use anyhow::{Context, Result};
use clap::Subcommand;
use minutes_core::voice::{self, TrustClass, VoiceError, VoiceSampleInput};
use minutes_core::Config;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_ENROLL_SECONDS: u64 = 25;
const TEST_SECONDS: u64 = 8;
const MAX_CAPTURE_SECONDS: u64 = 300;

#[derive(Subcommand)]
pub(crate) enum VoiceAction {
    /// Teach Minutes your voice from a quality-gated microphone sample
    Enroll {
        /// Number of seconds to record
        #[arg(long, default_value_t = DEFAULT_ENROLL_SECONDS)]
        seconds: u64,
    },
    /// Show whether you and other people have active voice enrollments
    Status,
    /// List enrolled people, sample counts, and embedding models
    List,
    /// Record a short probe and compare it with active voice profiles
    Test,
    /// Revoke every active sample for a person slug
    Remove { slug: String },
    /// Permanently delete every local voiceprint and meeting embedding sidecar
    DeleteAll,
}

struct CapturedClip {
    path: PathBuf,
}

impl CapturedClip {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("minutes-voice-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        Ok(Self {
            path: directory.join("voice.wav"),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CapturedClip {
    fn drop(&mut self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

fn capture_clip(seconds: u64, config: &Config) -> Result<CapturedClip> {
    if !(1..=MAX_CAPTURE_SECONDS).contains(&seconds) {
        anyhow::bail!("capture duration must be between 1 and {MAX_CAPTURE_SECONDS} seconds");
    }
    let clip = CapturedClip::new()?;
    let stop = Arc::new(AtomicBool::new(false));
    let timer_stop = Arc::clone(&stop);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        timer_stop.store(true, Ordering::Relaxed);
    });
    minutes_core::capture::record_to_wav(clip.path(), stop, config)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(clip)
}

fn open_voice_db() -> Result<voice::VoiceConnection> {
    let conn = voice::open_db().map_err(|error| anyhow::anyhow!(error))?;
    voice::migrate_legacy_profiles(&conn).map_err(|error| anyhow::anyhow!(error))?;
    Ok(conn)
}

fn identity(config: &Config) -> Result<(&str, String)> {
    let name = config
        .identity
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .context("set [identity] name in ~/.config/minutes/config.toml before enrolling")?;
    Ok((name, voice::person_slug(name)))
}

fn enroll(seconds: u64, config: &Config) -> Result<()> {
    let (name, slug) = identity(config)?;
    eprintln!(
        "Voice enrollment for {name}: speak naturally for {seconds} seconds. The voiceprint stays on this device."
    );
    let clip = capture_clip(seconds, config)?;
    eprintln!("Analyzing overlapping speech windows...");
    let solo = match voice::embed_solo_clip(clip.path(), config) {
        Ok(solo) => solo,
        Err(VoiceError::LowQuality { reason }) => {
            eprintln!("Enrollment rejected: {reason}");
            return Err(anyhow::anyhow!(
                "voice enrollment did not pass the quality gate"
            ));
        }
        Err(error) => return Err(anyhow::anyhow!(error)),
    };
    let quality_json = serde_json::to_string(&solo.quality)?;
    let conn = open_voice_db()?;
    voice::insert_voice_sample(
        &conn,
        &VoiceSampleInput {
            person_slug: slug.clone(),
            name: name.to_string(),
            embedding: solo.embedding,
            model_id: solo.model_id.clone(),
            trust_class: TrustClass::Manual,
            meeting_path: None,
            sidecar_speaker: None,
            capture_source: Some("cli-manual-enrollment".to_string()),
            speech_seconds: solo.speech_seconds,
            segment_count: solo.segment_count,
            quality_json: Some(quality_json),
            similarity: None,
            top2_margin: None,
            threshold_version: None,
            sensitivity: "normal".to_string(),
            created_at: None,
        },
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    let summary = voice::list_voice_enrollments(&conn)
        .map_err(|error| anyhow::anyhow!(error))?
        .into_iter()
        .find(|summary| summary.person_slug == slug && summary.model_id == solo.model_id)
        .context("enrollment was stored but its active profile could not be read")?;
    println!("Enrolled: {} ({})", summary.name, summary.person_slug);
    println!("Samples: {}", summary.sample_count);
    println!("Model: {}", summary.model_id);
    println!(
        "Quality: {:.1} dB SNR, {:.2}% clipping, {:.3} window consistency",
        solo.quality.snr,
        solo.quality.clipping * 100.0,
        solo.quality.window_consistency
    );
    Ok(())
}

fn status(config: &Config) -> Result<()> {
    let conn = open_voice_db()?;
    let summaries = voice::list_voice_enrollments(&conn).map_err(|error| anyhow::anyhow!(error))?;
    let self_slug = config.identity.name.as_deref().map(voice::person_slug);
    let self_rows = summaries
        .iter()
        .filter(|summary| Some(&summary.person_slug) == self_slug.as_ref())
        .collect::<Vec<_>>();
    if let Some(name) = config.identity.name.as_deref() {
        if self_rows.is_empty() {
            println!("You ({name}) are not enrolled.");
        } else {
            println!("You ({name}) are enrolled.");
            for summary in &self_rows {
                print_summary(summary);
            }
        }
    } else {
        println!("Self enrollment: unknown ([identity] name is not configured).");
    }
    let others = summaries.len().saturating_sub(self_rows.len());
    println!("Other enrolled people/models: {others}");
    if summaries.is_empty() {
        println!("No active voice profiles. Run `minutes voice enroll` to create one.");
    }
    Ok(())
}

fn print_summary(summary: &voice::VoiceEnrollmentSummary) {
    println!(
        "  {} ({}) — {} sample(s), model {}",
        summary.name, summary.person_slug, summary.sample_count, summary.model_id
    );
    match (summary.last_match_similarity, summary.last_match_margin) {
        (Some(similarity), Some(margin)) => {
            println!("    Last match: {similarity:.3}, top-two margin {margin:.3}");
        }
        (Some(similarity), None) => println!("    Last match: {similarity:.3}"),
        (None, _) => println!("    Last match: no stored match evidence"),
    }
}

fn list() -> Result<()> {
    let conn = open_voice_db()?;
    let summaries = voice::list_voice_enrollments(&conn).map_err(|error| anyhow::anyhow!(error))?;
    if summaries.is_empty() {
        println!("No active voice profiles.");
        return Ok(());
    }
    for summary in &summaries {
        print_summary(summary);
    }
    Ok(())
}

fn test_voice(config: &Config) -> Result<()> {
    let model_id = voice::model_version(config);
    let conn = open_voice_db()?;
    let profiles =
        voice::list_active_profiles(&conn, model_id).map_err(|error| anyhow::anyhow!(error))?;
    if profiles.is_empty() {
        anyhow::bail!(
            "no active profiles use model {model_id}; enroll first with `minutes voice enroll`"
        );
    }
    eprintln!("Speak naturally for {TEST_SECONDS} seconds to test voice matching.");
    let clip = capture_clip(TEST_SECONDS, config)?;
    let probe = voice::embed_solo_clip(clip.path(), config).map_err(|error| {
        if let VoiceError::LowQuality { reason } = &error {
            eprintln!("Voice test rejected: {reason}");
        }
        anyhow::anyhow!(error)
    })?;
    let evidence = voice::match_active_profiles(
        &probe.embedding,
        &probe.model_id,
        &profiles,
        config.voice.match_threshold,
    );
    if evidence.accepted {
        println!(
            "Match: {} ({})",
            evidence.winner_name.as_deref().unwrap_or("unknown"),
            evidence.winner_slug.as_deref().unwrap_or("unknown")
        );
    } else {
        println!("No confident match.");
    }
    if let Some(similarity) = evidence.similarity {
        println!(
            "Similarity: {similarity:.3} (threshold {:.3})",
            evidence.threshold
        );
    }
    if let Some(runner_up) = evidence.runner_up_similarity {
        println!("Runner-up: {runner_up:.3}");
    }
    if let Some(margin) = evidence.margin {
        println!("Top-two margin: {margin:.3}");
    }
    println!("Evidence: {} [{}]", evidence.reason, evidence.model_id);
    Ok(())
}

fn remove(slug: &str) -> Result<()> {
    let normalized = slug.trim();
    if normalized.is_empty() {
        anyhow::bail!("person slug cannot be empty");
    }
    let conn = open_voice_db()?;
    let changed =
        voice::revoke_voice_person(&conn, normalized).map_err(|error| anyhow::anyhow!(error))?;
    if changed == 0 {
        anyhow::bail!("no active voice samples found for slug `{normalized}`");
    }
    println!("Revoked {changed} voice sample/profile row(s) for `{normalized}`.");
    Ok(())
}

fn delete_all(config: &Config) -> Result<()> {
    eprintln!(
        "This permanently deletes all voice profiles, samples, caches, and .embeddings sidecars."
    );
    eprint!("Type DELETE to continue: ");
    std::io::stderr().flush()?;
    let mut confirmation = String::new();
    std::io::stdin().read_line(&mut confirmation)?;
    if confirmation.trim() != "DELETE" {
        anyhow::bail!("voice-data deletion cancelled");
    }
    let report = voice::delete_all_voice_data(config).map_err(|error| anyhow::anyhow!(error))?;
    println!(
        "Deleted {} profile row(s), {} sample(s), {} active cache row(s), {} sidecar(s), and {} SQLite WAL/SHM file(s).",
        report.profiles_deleted,
        report.samples_deleted,
        report.active_profiles_deleted,
        report.sidecars_deleted,
        report.sqlite_aux_files_deleted
    );
    Ok(())
}

pub(crate) fn run(action: VoiceAction, config: &Config) -> Result<()> {
    match action {
        VoiceAction::Enroll { seconds } => enroll(seconds, config),
        VoiceAction::Status => status(config),
        VoiceAction::List => list(),
        VoiceAction::Test => test_voice(config),
        VoiceAction::Remove { slug } => remove(&slug),
        VoiceAction::DeleteAll => delete_all(config),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn voice_cli_parses_every_subcommand() {
        for args in [
            vec!["minutes", "voice", "enroll", "--seconds", "30"],
            vec!["minutes", "voice", "status"],
            vec!["minutes", "voice", "list"],
            vec!["minutes", "voice", "test"],
            vec!["minutes", "voice", "remove", "mat"],
            vec!["minutes", "voice", "delete-all"],
        ] {
            assert!(crate::Cli::try_parse_from(args).is_ok());
        }
    }

    #[test]
    fn voice_cli_enroll_defaults_to_twenty_five_seconds() {
        let cli = crate::Cli::try_parse_from(["minutes", "voice", "enroll"]).unwrap();
        let crate::Commands::Voice {
            action: VoiceAction::Enroll { seconds },
        } = cli.command
        else {
            panic!("expected voice enroll");
        };
        assert_eq!(seconds, DEFAULT_ENROLL_SECONDS);
    }
}
