//! Read-only, non-authorizing validation of observed native PCM prefixes.
//!
//! Callers supply already-authorized descriptors and their canonical-path
//! hashes. No pathname is opened here, no transcript is accepted, and no Stop
//! or processing path invokes this automatically. Hash checkpoints share one
//! incremental scan per source; bytes beyond the last observed prefix are not
//! reread. Producer completion and transcript/timeline quality remain separate.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

const MAX_DIAGNOSTICS: u64 = 16 * 1024 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_PREFIXES: usize = 65_536;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum SourceSlot {
    Voice,
    System,
}

pub struct SourceCandidate<'a> {
    pub slot: SourceSlot,
    pub file: &'a mut File,
    /// Computed by the caller from its authorized descriptor/path binding,
    /// using the observer's canonical UTF-8 path SHA-256 convention.
    pub canonical_path_sha256: &'a str,
}

pub struct ValidationBudget {
    pub max_source_bytes: u64,
    pub max_duration: Duration,
}

impl Default for ValidationBudget {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024 * 1024,
            max_duration: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceIssue {
    MissingObservations,
    MissingCandidate,
    AliasedInputs,
    InvalidatedObserver,
    MultipleEpochs,
    InconsistentObservations,
    LocatorMismatch,
    EmptyPrefix,
    UnreadableSource,
    UnfinalizedOrUnsupportedWav,
    FormatMismatch,
    PrefixBeyondData,
    PrefixHashMismatch,
    SourceChanged,
    BudgetExceeded,
}

#[derive(Debug, Serialize)]
pub struct SourceValidation {
    pub slot: SourceSlot,
    pub observations: usize,
    pub matched_prefixes: usize,
    pub source_bytes_scanned: u64,
    pub file_pcm_bytes: Option<u64>,
    pub unobserved_suffix_bytes: Option<u64>,
    pub issues: Vec<SourceIssue>,
}

#[derive(Debug, Serialize)]
pub struct SourceValidationReport {
    pub version: u32,
    pub session_bound: bool,
    pub range_geometry_valid: bool,
    pub unmapped_forwarded_samples: u64,
    pub sources: Vec<SourceValidation>,
    pub all_observed_prefixes_match: bool,
    pub reuse_authorized: bool,
    pub capture_completion_verified: bool,
    pub timeline_verified: bool,
    pub transcript_quality_verified: bool,
    pub remaining_requirements: Vec<&'static str>,
}

#[derive(Clone, Deserialize)]
struct Prefix {
    epoch: u64,
    role: SourceSlot,
    locator_sha256: Option<String>,
    sample_rate: u32,
    channels: u16,
    encoding: String,
    data_offset: u64,
    pcm_bytes: u64,
    pcm_sha256: String,
    decoded_samples: u64,
    invalidated: Option<String>,
}

#[derive(Deserialize)]
struct InputRange {
    epoch: u64,
    role: SourceSlot,
    source_start_sample: u64,
    output_start_sample: u64,
    sample_count: u64,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or unsupported live source diagnostics",
    )
}

fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn same_metadata(before: &Metadata, file: &File) -> bool {
    file.metadata().is_ok_and(|after| {
        before.len() == after.len()
            && before
                .modified()
                .ok()
                .zip(after.modified().ok())
                .is_some_and(|(a, b)| a == b)
    })
}

fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "source validation exceeded its deadline",
        ))
    } else {
        Ok(())
    }
}

fn parse_observations(
    bytes: &[u8],
    expected_session: Option<&str>,
    deadline: Instant,
) -> io::Result<(BTreeMap<SourceSlot, Vec<Prefix>>, bool, u64)> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    if text.is_empty() || !text.ends_with('\n') {
        return Err(invalid());
    }
    let mut groups: BTreeMap<SourceSlot, Vec<Prefix>> = BTreeMap::new();
    let mut session_bound = false;
    let mut last_end = BTreeMap::<u64, u64>::new();
    let mut gaps = 0_u64;
    let mut count = 0;
    let mut source_ends = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        check_deadline(deadline)?;
        if line.len() > MAX_LINE {
            return Err(invalid());
        }
        let row: serde_json::Value = serde_json::from_str(line).map_err(|_| invalid())?;
        if index == 0 {
            if row["event"] != "manifest"
                || row["version"] != 1
                || row["sample_rate"] != 16000
                || row["reuse_authorized"] != false
                || row["source_file_binding"] != "unverified"
                || row["timeline"] != "forwarded-stream"
            {
                return Err(invalid());
            }
            if let Some(expected) = expected_session {
                if expected.is_empty() || row["session_id"].as_str() != Some(expected) {
                    return Err(invalid());
                }
                session_bound = true;
            }
            continue;
        }
        if row["event"] == "manifest" {
            return Err(invalid());
        }
        if row["event"] != "source_inputs" {
            continue;
        }
        if row["version"] != 1
            || row["reuse_authorized"] != false
            || row["epoch_scope"] != "this-manifest"
            || row["source_roles"] != "caller-file-slots"
            || row["resampling"] != "channel-average-nearest-source-v1"
            || row["mixing"] != "sum-clamp-v1"
            || row["locator_format"] != "canonical-path-utf8-sha256"
            || row["binding"] != "observed-pcm-prefix"
            || row["final_file_validation"] != "not-performed"
            || row["capture_end"] != "unknown"
        {
            return Err(invalid());
        }
        let stream = row["span"]["stream"].as_u64().ok_or_else(invalid)?;
        let start = row["span"]["start_sample"].as_u64().ok_or_else(invalid)?;
        let len = row["span"]["sample_count"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(invalid)?;
        let end = start.checked_add(len).ok_or_else(invalid)?;
        let previous = last_end.insert(stream, end).unwrap_or(0);
        if stream == 0 || previous > start {
            return Err(invalid());
        }
        gaps = gaps.checked_add(start - previous).ok_or_else(invalid)?;
        let prefixes: Vec<Prefix> =
            serde_json::from_value(row["sources"]["prefixes"].clone()).map_err(|_| invalid())?;
        let ranges: Vec<InputRange> =
            serde_json::from_value(row["sources"]["ranges"].clone()).map_err(|_| invalid())?;
        if prefixes.is_empty() || prefixes.len() > 2 || ranges.is_empty() || ranges.len() > 4 {
            return Err(invalid());
        }
        let mut roles = BTreeSet::new();
        for p in &prefixes {
            let width = match p.encoding.as_str() {
                "i16-le" => 2,
                "f32-le" => 4,
                _ => return Err(invalid()),
            };
            let frame = u64::from(p.channels) * width;
            if !roles.insert(p.role)
                || p.epoch == 0
                || p.channels == 0
                || p.channels > 32
                || !(16000..=384000).contains(&p.sample_rate)
                || !is_digest(&p.pcm_sha256)
                || p.locator_sha256.as_deref().is_some_and(|h| !is_digest(h))
                || p.data_offset < 44
                || p.pcm_bytes % frame != 0
                || p.decoded_samples > p.pcm_bytes / frame
            {
                return Err(invalid());
            }
        }
        let mut output_ends = BTreeMap::new();
        for r in ranges {
            let p = prefixes
                .iter()
                .find(|p| p.role == r.role && p.epoch == r.epoch)
                .ok_or_else(invalid)?;
            let input_end = r
                .source_start_sample
                .checked_add(r.sample_count)
                .ok_or_else(invalid)?;
            let output_end = r
                .output_start_sample
                .checked_add(r.sample_count)
                .ok_or_else(invalid)?;
            if r.sample_count == 0
                || input_end > p.decoded_samples
                || output_end > len
                || output_ends
                    .get(&r.role)
                    .is_some_and(|previous| *previous > r.output_start_sample)
            {
                return Err(invalid());
            }
            if source_ends
                .get(&(r.role, r.epoch))
                .is_some_and(|previous| *previous > r.source_start_sample)
            {
                return Err(invalid());
            }
            source_ends.insert((r.role, r.epoch), input_end);
            output_ends.insert(r.role, output_end);
        }
        for p in prefixes {
            count += 1;
            if count > MAX_PREFIXES {
                return Err(invalid());
            }
            groups.entry(p.role).or_default().push(p);
        }
    }
    Ok((groups, session_bound, gaps))
}

/// Check prefix observations against caller-authorized source descriptors.
///
/// A successful hash match proves only the bytes observed during this scan.
/// It never grants capture authority, proves a complete timeline, approves
/// transcript quality, or permits skipping final recognition. Callers must
/// attest their own descriptor/locator binding and producer completion. This
/// offline scan is not wired into Stop or the normal processing pipeline.
pub fn validate_source_prefixes(
    diagnostics: &mut File,
    candidates: &mut [SourceCandidate<'_>],
    expected_session: Option<&str>,
    budget: ValidationBudget,
) -> io::Result<SourceValidationReport> {
    if budget.max_source_bytes == 0
        || budget.max_source_bytes > 8 * 1024 * 1024 * 1024
        || budget.max_duration.is_zero()
        || budget.max_duration > Duration::from_secs(60)
        || !crate::policy_fs::opened_regular_file_is_safe(diagnostics)
    {
        return Err(invalid());
    }
    let deadline = Instant::now()
        .checked_add(budget.max_duration)
        .ok_or_else(invalid)?;
    let before = diagnostics.metadata()?;
    let bytes =
        crate::policy_fs::read_bound_file_twice_bounded(diagnostics, MAX_DIAGNOSTICS, deadline)?;
    if !same_metadata(&before, diagnostics) {
        return Err(invalid());
    }
    let (groups, session_bound, gaps) = parse_observations(&bytes, expected_session, deadline)?;
    let mut slots = BTreeSet::new();
    for candidate in candidates.iter() {
        if !slots.insert(candidate.slot) || !is_digest(candidate.canonical_path_sha256) {
            return Err(invalid());
        }
    }
    let aliases = candidates.len() == 2
        && crate::policy_fs::open_file_identity_matches(candidates[0].file, candidates[1].file);
    let mut remaining = budget.max_source_bytes;
    let mut results = Vec::new();
    for slot in [SourceSlot::Voice, SourceSlot::System] {
        let prefixes = groups.get(&slot).map(Vec::as_slice).unwrap_or(&[]);
        let candidate = candidates.iter_mut().find(|c| c.slot == slot);
        if prefixes.is_empty() && candidate.is_none() {
            continue;
        }
        let mut result = SourceValidation {
            slot,
            observations: prefixes.len(),
            matched_prefixes: 0,
            source_bytes_scanned: 0,
            file_pcm_bytes: None,
            unobserved_suffix_bytes: None,
            issues: Vec::new(),
        };
        if prefixes.is_empty() {
            result.issues.push(SourceIssue::MissingObservations);
        }
        if aliases {
            result.issues.push(SourceIssue::AliasedInputs);
        }
        if let Some(candidate) = candidate {
            verify_source(
                prefixes,
                candidate,
                &mut remaining,
                deadline,
                &mut result,
                || {},
            );
        } else {
            result.issues.push(SourceIssue::MissingCandidate);
        }
        results.push(result);
    }
    let all_match = !results.is_empty()
        && results
            .iter()
            .all(|r| r.issues.is_empty() && r.matched_prefixes == r.observations);
    let mut requirements = vec![
        "trusted-capture-completion",
        "trusted-source-role-binding",
        "timeline-and-coverage-validation",
        "matched-transcript-quality",
    ];
    if !session_bound {
        requirements.push("session-binding");
    }
    if results.is_empty() {
        requirements.push("source-observations");
    }
    Ok(SourceValidationReport {
        version: 1,
        session_bound,
        range_geometry_valid: true,
        unmapped_forwarded_samples: gaps,
        sources: results,
        all_observed_prefixes_match: all_match,
        reuse_authorized: false,
        capture_completion_verified: false,
        timeline_verified: false,
        transcript_quality_verified: false,
        remaining_requirements: requirements,
    })
}

fn verify_source(
    prefixes: &[Prefix],
    candidate: &mut SourceCandidate<'_>,
    remaining: &mut u64,
    deadline: Instant,
    result: &mut SourceValidation,
    after_read: impl FnOnce(),
) {
    let file = &mut *candidate.file;
    if !crate::policy_fs::opened_regular_file_is_safe(file) {
        result.issues.push(SourceIssue::UnreadableSource);
        return;
    }
    let Ok(before) = file.metadata() else {
        result.issues.push(SourceIssue::UnreadableSource);
        return;
    };
    let Ok(layout) = crate::stem_probe::final_wav_layout(file) else {
        result.issues.push(SourceIssue::UnfinalizedOrUnsupportedWav);
        return;
    };
    result.file_pcm_bytes = Some(layout.data_bytes);
    let Some(first) = prefixes.first() else {
        return;
    };
    if prefixes.iter().any(|p| p.invalidated.is_some()) {
        result.issues.push(SourceIssue::InvalidatedObserver);
    }
    if prefixes.iter().any(|p| p.epoch != first.epoch) {
        result.issues.push(SourceIssue::MultipleEpochs);
    }
    if prefixes
        .iter()
        .any(|p| p.locator_sha256.as_deref() != Some(candidate.canonical_path_sha256))
    {
        result.issues.push(SourceIssue::LocatorMismatch);
    }
    if prefixes.iter().any(|p| {
        p.sample_rate != layout.sample_rate
            || p.channels != layout.channels
            || p.encoding != layout.encoding
            || p.data_offset != layout.data_offset
    }) || layout.data_bytes / layout.frame_bytes
        > u64::from(layout.sample_rate) * crate::audio_budget::MAX_AUDIO_SECONDS
    {
        result.issues.push(SourceIssue::FormatMismatch);
    }
    let mut points = BTreeMap::<u64, (String, u64, usize)>::new();
    for p in prefixes {
        match points.entry(p.pcm_bytes) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert((p.pcm_sha256.to_ascii_lowercase(), p.decoded_samples, 1));
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                let entry = e.get_mut();
                if entry.0 != p.pcm_sha256.to_ascii_lowercase() || entry.1 != p.decoded_samples {
                    result.issues.push(SourceIssue::InconsistentObservations);
                    return;
                }
                entry.2 += 1;
            }
        }
    }
    let max = points.last_key_value().map(|(n, _)| *n).unwrap_or(0);
    if max == 0 {
        result.issues.push(SourceIssue::EmptyPrefix);
    }
    if max > layout.data_bytes {
        result.issues.push(SourceIssue::PrefixBeyondData);
    }
    result.unobserved_suffix_bytes = layout.data_bytes.checked_sub(max);
    if max > *remaining {
        result.issues.push(SourceIssue::BudgetExceeded);
    }
    if !result.issues.is_empty() {
        return;
    }
    if file.seek(SeekFrom::Start(layout.data_offset)).is_err() {
        result.issues.push(SourceIssue::UnreadableSource);
        return;
    }
    let mut hash = Sha256::new();
    let mut at = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    for (end, (expected, _, duplicates)) in points {
        while at < end {
            if check_deadline(deadline).is_err() {
                result.issues.push(SourceIssue::BudgetExceeded);
                return;
            }
            let want = (end - at).min(buffer.len() as u64) as usize;
            if file.read_exact(&mut buffer[..want]).is_err() {
                result.issues.push(SourceIssue::UnreadableSource);
                return;
            }
            hash.update(&buffer[..want]);
            at += want as u64;
            *remaining -= want as u64;
            result.source_bytes_scanned += want as u64;
        }
        if format!("{:x}", hash.clone().finalize()) != expected {
            result.issues.push(SourceIssue::PrefixHashMismatch);
            return;
        }
        result.matched_prefixes += duplicates;
    }
    after_read();
    if !same_metadata(&before, file) {
        result.issues.push(SourceIssue::SourceChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_audio_coverage::{
        AudioSource, AudioSpan, CoverageLedger, PacketSources, SourcePrefix, SourceRange, StemRole,
    };
    use std::io::Write;
    use std::path::PathBuf;

    struct Fixture {
        _dir: tempfile::TempDir,
        source: PathBuf,
        diag: PathBuf,
        locator: String,
    }

    impl Fixture {
        fn new(checkpoints: &[u64]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("private-source-name.wav");
            let mut wav = hound::WavWriter::create(
                &source,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for n in 0..300 {
                wav.write_sample(n as i16).unwrap();
            }
            wav.finalize().unwrap();
            let locator = format!(
                "{:x}",
                Sha256::digest(source.canonicalize().unwrap().to_str().unwrap().as_bytes())
            );
            let pcm: Vec<u8> = (0..300).flat_map(|n| (n as i16).to_le_bytes()).collect();
            let diag = dir.path().join("coverage.jsonl");
            let mut ledger = CoverageLedger::open(&diag, Some("fixture-session")).unwrap();
            for (index, frames) in checkpoints.iter().enumerate() {
                let prefix = SourcePrefix {
                    epoch: 7,
                    role: StemRole::Voice,
                    locator_sha256: Some(locator.clone()),
                    sample_rate: 16000,
                    channels: 1,
                    encoding: "i16-le",
                    data_offset: 44,
                    pcm_bytes: frames * 2,
                    pcm_sha256: format!("{:x}", Sha256::digest(&pcm[..(*frames * 2) as usize])),
                    decoded_samples: *frames,
                    invalidated: None,
                };
                let span = AudioSpan {
                    stream: 1,
                    source: AudioSource::Mixed,
                    start_sample: index as u64 * 100,
                    sample_count: 100,
                };
                let sources = PacketSources {
                    prefixes: vec![prefix],
                    ranges: vec![SourceRange {
                        epoch: 7,
                        role: StemRole::Voice,
                        source_start_sample: index as u64 * 100,
                        output_start_sample: 0,
                        sample_count: 100,
                    }],
                };
                ledger.observe(Some(&span));
                ledger.observe_sources(Some(&span), Some(&sources));
            }
            ledger.finish();
            drop(ledger);
            Self {
                _dir: dir,
                source,
                diag,
                locator,
            }
        }
        fn validate(
            &self,
            session: Option<&str>,
            budget: ValidationBudget,
        ) -> io::Result<SourceValidationReport> {
            let mut diag = File::open(&self.diag)?;
            let mut file = File::open(&self.source)?;
            validate_source_prefixes(
                &mut diag,
                &mut [SourceCandidate {
                    slot: SourceSlot::Voice,
                    file: &mut file,
                    canonical_path_sha256: &self.locator,
                }],
                session,
                budget,
            )
        }
        fn edit(&self, mut change: impl FnMut(&mut serde_json::Value)) {
            let text = std::fs::read_to_string(&self.diag).unwrap();
            let mut out = String::new();
            for line in text.lines() {
                let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                change(&mut row);
                out.push_str(&serde_json::to_string(&row).unwrap());
                out.push('\n');
            }
            std::fs::write(&self.diag, out).unwrap();
        }
    }

    #[test]
    fn verifies_all_prefixes_in_one_scan_and_reports_unobserved_tail_without_reuse() {
        let f = Fixture::new(&[100, 200]);
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert!(r.all_observed_prefixes_match && r.session_bound && r.range_geometry_valid);
        assert_eq!(r.sources[0].observations, 2);
        assert_eq!(r.sources[0].matched_prefixes, 2);
        assert_eq!(r.sources[0].source_bytes_scanned, 400);
        assert_eq!(r.sources[0].file_pcm_bytes, Some(600));
        assert_eq!(r.sources[0].unobserved_suffix_bytes, Some(200));
        assert!(
            !r.reuse_authorized
                && !r.capture_completion_verified
                && !r.timeline_verified
                && !r.transcript_quality_verified
        );
        assert!(!serde_json::to_string(&r)
            .unwrap()
            .contains("private-source-name"));
    }

    #[test]
    fn repeated_read_ahead_prefix_checkpoints_are_hashed_once() {
        let f = Fixture::new(&[200, 200]);
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert!(r.all_observed_prefixes_match);
        assert_eq!(r.sources[0].matched_prefixes, 2);
        assert_eq!(r.sources[0].source_bytes_scanned, 400);
    }

    #[test]
    fn changed_pcm_rejects_a_later_prefix_even_after_an_earlier_prefix_matches() {
        let f = Fixture::new(&[100, 200]);
        let mut writer = File::options().write(true).open(&f.source).unwrap();
        writer.seek(SeekFrom::Start(44 + 150 * 2)).unwrap();
        writer.write_all(&999_i16.to_le_bytes()).unwrap();
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert!(!r.all_observed_prefixes_match);
        assert_eq!(r.sources[0].matched_prefixes, 1);
        assert_eq!(r.sources[0].issues, vec![SourceIssue::PrefixHashMismatch]);
    }

    #[test]
    fn inconsistent_final_header_is_not_repaired_or_accepted() {
        let f = Fixture::new(&[100, 200]);
        let mut writer = File::options().write(true).open(&f.source).unwrap();
        writer.seek(SeekFrom::Start(4)).unwrap();
        writer.write_all(&0_u32.to_le_bytes()).unwrap();
        let before = std::fs::read(&f.source).unwrap();
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert_eq!(
            r.sources[0].issues,
            vec![SourceIssue::UnfinalizedOrUnsupportedWav]
        );
        assert_eq!(std::fs::read(&f.source).unwrap(), before);
        assert_eq!(r.sources[0].source_bytes_scanned, 0);
    }

    #[test]
    fn changed_source_format_rejects_matching_pcm_bytes() {
        let f = Fixture::new(&[100, 200]);
        let mut writer = File::options().write(true).open(&f.source).unwrap();
        writer.seek(SeekFrom::Start(24)).unwrap();
        writer.write_all(&24000_u32.to_le_bytes()).unwrap();
        writer.write_all(&48000_u32.to_le_bytes()).unwrap();
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert_eq!(r.sources[0].issues, vec![SourceIssue::FormatMismatch]);
        assert_eq!(r.sources[0].source_bytes_scanned, 0);
    }

    #[test]
    fn wrong_session_is_rejected_and_unknown_session_never_becomes_bound() {
        let f = Fixture::new(&[100, 200]);
        assert!(f
            .validate(Some("another-session"), ValidationBudget::default())
            .is_err());
        let r = f.validate(None, ValidationBudget::default()).unwrap();
        assert!(r.all_observed_prefixes_match);
        assert!(!r.session_bound && !r.reuse_authorized);
        assert!(r.remaining_requirements.contains(&"session-binding"));
    }

    #[test]
    fn source_hash_locators_and_observer_invalidations_are_enforced() {
        for issue in ["locator", "invalidated", "epoch", "length"] {
            let f = Fixture::new(&[100, 200]);
            f.edit(|row| {
                if row["event"] == "source_inputs" {
                    let prefix = &mut row["sources"]["prefixes"][0];
                    match issue {
                        "locator" => prefix["locator_sha256"] = "b".repeat(64).into(),
                        "invalidated" => prefix["invalidated"] = "file-truncated".into(),
                        "epoch" => {
                            if prefix["pcm_bytes"] == 400 {
                                prefix["epoch"] = 8.into();
                            }
                        }
                        "length" => {
                            prefix["pcm_bytes"] = 800.into();
                            prefix["decoded_samples"] = 400.into();
                            prefix["pcm_sha256"] = "c".repeat(64).into();
                        }
                        _ => unreachable!(),
                    }
                    if issue == "epoch" && row["sources"]["prefixes"][0]["epoch"] == 8 {
                        row["sources"]["ranges"][0]["epoch"] = 8.into();
                    }
                }
            });
            let r = f
                .validate(Some("fixture-session"), ValidationBudget::default())
                .unwrap();
            assert!(!r.all_observed_prefixes_match, "{issue}");
            assert_eq!(r.sources[0].source_bytes_scanned, 0, "{issue}");
        }
    }

    #[test]
    fn duplicate_length_hash_conflicts_and_invalid_range_geometry_are_rejected() {
        let f = Fixture::new(&[200, 200]);
        let mut n = 0;
        f.edit(|row| {
            if row["event"] == "source_inputs" {
                n += 1;
                if n == 2 {
                    row["sources"]["prefixes"][0]["pcm_sha256"] = "f".repeat(64).into();
                }
            }
        });
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert_eq!(
            r.sources[0].issues,
            vec![SourceIssue::InconsistentObservations]
        );
        for mode in ["output", "input", "repeat", "overflow", "role", "version"] {
            let f = Fixture::new(&[100, 200]);
            f.edit(|row| {
                if row["event"] == "source_inputs" {
                    match mode {
                        "output" => row["sources"]["ranges"][0]["output_start_sample"] = 1.into(),
                        "input" => {
                            row["sources"]["ranges"][0]["source_start_sample"] = u64::MAX.into()
                        }
                        "repeat" => row["sources"]["ranges"][0]["source_start_sample"] = 0.into(),
                        "overflow" => row["span"]["start_sample"] = u64::MAX.into(),
                        "role" => row["sources"]["ranges"][0]["role"] = "unverified-person".into(),
                        "version" => row["version"] = 99.into(),
                        _ => unreachable!(),
                    }
                }
            });
            assert!(
                f.validate(Some("fixture-session"), ValidationBudget::default())
                    .is_err(),
                "{mode}"
            );
        }
    }

    #[test]
    fn missing_candidates_observations_and_aliased_inputs_are_not_accepted() {
        let f = Fixture::new(&[100, 200]);
        let mut diag = File::open(&f.diag).unwrap();
        let r = validate_source_prefixes(
            &mut diag,
            &mut [],
            Some("fixture-session"),
            ValidationBudget::default(),
        )
        .unwrap();
        assert_eq!(r.sources[0].issues, vec![SourceIssue::MissingCandidate]);
        let mut a = File::open(&f.source).unwrap();
        let mut b = a.try_clone().unwrap();
        let r = validate_source_prefixes(
            &mut diag,
            &mut [
                SourceCandidate {
                    slot: SourceSlot::Voice,
                    file: &mut a,
                    canonical_path_sha256: &f.locator,
                },
                SourceCandidate {
                    slot: SourceSlot::System,
                    file: &mut b,
                    canonical_path_sha256: &f.locator,
                },
            ],
            Some("fixture-session"),
            ValidationBudget::default(),
        )
        .unwrap();
        assert!(!r.all_observed_prefixes_match);
        assert!(r
            .sources
            .iter()
            .all(|s| s.issues.contains(&SourceIssue::AliasedInputs)));
        let f = Fixture::new(&[]);
        let r = f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .unwrap();
        assert_eq!(r.sources[0].issues, vec![SourceIssue::MissingObservations]);
    }

    #[test]
    fn byte_time_text_and_record_budgets_fail_closed() {
        let f = Fixture::new(&[100, 200]);
        let r = f
            .validate(
                Some("fixture-session"),
                ValidationBudget {
                    max_source_bytes: 399,
                    ..ValidationBudget::default()
                },
            )
            .unwrap();
        assert_eq!(r.sources[0].issues, vec![SourceIssue::BudgetExceeded]);
        assert_eq!(r.sources[0].source_bytes_scanned, 0);
        assert!(f
            .validate(
                Some("fixture-session"),
                ValidationBudget {
                    max_duration: Duration::ZERO,
                    ..ValidationBudget::default()
                }
            )
            .is_err());
        File::options()
            .write(true)
            .open(&f.diag)
            .unwrap()
            .set_len(MAX_DIAGNOSTICS + 1)
            .unwrap();
        assert!(f
            .validate(Some("fixture-session"), ValidationBudget::default())
            .is_err());
        std::fs::write(&f.diag, "x".repeat(MAX_LINE + 1) + "\n").unwrap();
        assert!(f.validate(None, ValidationBudget::default()).is_err());
        std::fs::write(&f.diag, "{\"event\":\"manifest\"}").unwrap();
        assert!(f.validate(None, ValidationBudget::default()).is_err());
    }

    #[test]
    fn source_changes_during_validation_invalidate_an_otherwise_matching_scan() {
        let f = Fixture::new(&[100, 200]);
        let rows = std::fs::read(&f.diag).unwrap();
        let (groups, _, _) = parse_observations(
            &rows,
            Some("fixture-session"),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        let mut file = File::open(&f.source).unwrap();
        let mut candidate = SourceCandidate {
            slot: SourceSlot::Voice,
            file: &mut file,
            canonical_path_sha256: &f.locator,
        };
        let mut result = SourceValidation {
            slot: SourceSlot::Voice,
            observations: 2,
            matched_prefixes: 0,
            source_bytes_scanned: 0,
            file_pcm_bytes: None,
            unobserved_suffix_bytes: None,
            issues: Vec::new(),
        };
        verify_source(
            &groups[&SourceSlot::Voice],
            &mut candidate,
            &mut 1024,
            Instant::now() + Duration::from_secs(5),
            &mut result,
            || {
                File::options()
                    .append(true)
                    .open(&f.source)
                    .unwrap()
                    .write_all(&0_i16.to_le_bytes())
                    .unwrap();
            },
        );
        assert_eq!(result.matched_prefixes, 2);
        assert_eq!(result.issues, vec![SourceIssue::SourceChanged]);
    }
}
