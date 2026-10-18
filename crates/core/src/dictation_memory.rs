use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::Read;
#[cfg(any(all(feature = "streaming", feature = "whisper"), test))]
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;
pub const MAX_HISTORY_RECORDS: usize = 100;
const MAX_PREVIEW_AUDIO_BYTES: u64 = 16 * 1024 * 1024;
static HISTORY_WRITE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DictationMemoryRecord {
    /// The first transcript before a user accepted retranscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_transcript: Option<OriginalDictationTranscript>,
    pub schema_version: u32,
    pub id: String,
    pub captured_at: DateTime<Local>,
    pub raw_text: String,
    pub cleaned_text: String,
    /// Cleaned text before explicit voice commands changed the output.
    #[serde(default)]
    pub pre_command_text: Option<String>,
    /// Local, content-minimal provenance for applied voice edits.
    #[serde(default)]
    pub commands_applied: Vec<crate::dictation_commands::DictationCommandProvenance>,
    pub duration_secs: f64,
    pub engine_id: String,
    pub engine_descriptor_version: Option<String>,
    pub vocabulary_mode: Option<String>,
    pub vocabulary_used: Vec<String>,
    pub destination: String,
    pub insertion: DictationInsertionMemory,
    pub target_context: Option<DictationTargetContext>,
    pub file_path: Option<PathBuf>,
    pub daily_note_appended: bool,
    /// Private captured audio retained only when delivery or transcription
    /// needs recovery. Successful routine dictation retires this file.
    #[serde(default)]
    pub recovery_audio_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OriginalDictationTranscript {
    pub raw_text: String,
    pub cleaned_text: String,
    pub engine_id: String,
    pub insertion: DictationInsertionMemory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DictationInsertionMemory {
    pub outcome: String,
    pub method: String,
    pub verified: bool,
    pub clipboard_restored: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DictationTargetContext {
    pub platform: String,
    pub app_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DictationMemoryInput {
    pub raw_text: String,
    pub cleaned_text: String,
    pub pre_command_text: Option<String>,
    pub commands_applied: Vec<crate::dictation_commands::DictationCommandProvenance>,
    pub duration_secs: f64,
    pub engine_id: String,
    pub engine_descriptor_version: Option<String>,
    pub vocabulary_mode: Option<String>,
    pub vocabulary_used: Vec<String>,
    pub destination: String,
    pub insertion: DictationInsertionMemory,
    pub target_context: Option<DictationTargetContext>,
    pub file_path: Option<PathBuf>,
    pub daily_note_appended: bool,
    pub recovery_audio_path: Option<PathBuf>,
}

impl DictationMemoryRecord {
    pub fn preserve_original_from(&mut self, before: &Self) {
        self.original_transcript = before.original_transcript.clone().or_else(|| {
            Some(OriginalDictationTranscript {
                raw_text: before.raw_text.clone(),
                cleaned_text: before.cleaned_text.clone(),
                engine_id: before.engine_id.clone(),
                insertion: before.insertion.clone(),
            })
        });
    }

    pub fn new(input: DictationMemoryInput) -> Self {
        let captured_at = Local::now();
        Self::from_parts(captured_at, input)
    }

    fn from_parts(captured_at: DateTime<Local>, input: DictationMemoryInput) -> Self {
        let id = record_id(
            &captured_at,
            &input.cleaned_text,
            input.duration_secs,
            &input.engine_id,
        );
        Self {
            original_transcript: None,
            schema_version: SCHEMA_VERSION,
            id,
            captured_at,
            raw_text: input.raw_text,
            cleaned_text: input.cleaned_text,
            pre_command_text: input.pre_command_text,
            commands_applied: input.commands_applied,
            duration_secs: input.duration_secs,
            engine_id: input.engine_id,
            engine_descriptor_version: input.engine_descriptor_version,
            vocabulary_mode: input.vocabulary_mode,
            vocabulary_used: input.vocabulary_used,
            destination: input.destination,
            insertion: input.insertion,
            target_context: input.target_context,
            file_path: input.file_path,
            daily_note_appended: input.daily_note_appended,
            recovery_audio_path: input.recovery_audio_path,
        }
    }
}

pub fn recovery_audio_root() -> PathBuf {
    crate::config::Config::minutes_dir().join("dictation-recovery")
}

/// Incremental crash-recovery WAV. The RIFF and data lengths are refreshed
/// after every chunk, so an application crash leaves a readable file rather
/// than an unfinalized placeholder header.
#[cfg(any(all(feature = "streaming", feature = "whisper"), test))]
pub(crate) struct DictationRecoveryCapture {
    path: PathBuf,
    file: Option<fs::File>,
    data_bytes: u64,
    settled: bool,
}

#[cfg(any(all(feature = "streaming", feature = "whisper"), test))]
impl DictationRecoveryCapture {
    #[cfg(all(feature = "streaming", feature = "whisper"))]
    pub(crate) fn create() -> io::Result<Self> {
        Self::create_in(&recovery_audio_root())
    }

    fn create_in(root: &Path) -> io::Result<Self> {
        crate::policy_fs::ensure_owner_only_directory(root)?;
        let temp = tempfile::Builder::new()
            .prefix("dictation-")
            .suffix(".wav")
            .tempfile_in(root)?;
        let (file, path) = temp.keep().map_err(|error| error.error)?;
        crate::policy_fs::ensure_owner_only_file(&path)?;
        let mut capture = Self {
            path,
            file: Some(file),
            data_bytes: 0,
            settled: false,
        };
        capture.write_header_lengths()?;
        capture.file_mut()?.flush()?;
        Ok(capture)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn append_samples(&mut self, samples: &[f32]) -> io::Result<()> {
        let added_bytes = samples
            .len()
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("dictation recovery WAV size overflowed"))?;
        let next_data_bytes = self
            .data_bytes
            .checked_add(added_bytes as u64)
            .filter(|bytes| *bytes <= u32::MAX as u64 - 36)
            .ok_or_else(|| io::Error::other("dictation recovery WAV exceeds RIFF limits"))?;

        let mut encoded = Vec::with_capacity(added_bytes);
        for &sample in samples {
            let value = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            encoded.extend_from_slice(&value.to_le_bytes());
        }
        self.file_mut()?.seek(SeekFrom::End(0))?;
        self.file_mut()?.write_all(&encoded)?;
        self.data_bytes = next_data_bytes;
        self.write_header_lengths()?;
        self.file_mut()?.flush()?;
        Ok(())
    }

    fn file_mut(&mut self) -> io::Result<&mut fs::File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("dictation recovery file is already closed"))
    }

    fn write_header_lengths(&mut self) -> io::Result<()> {
        let data_bytes = u32::try_from(self.data_bytes)
            .map_err(|_| io::Error::other("dictation recovery WAV exceeds RIFF limits"))?;
        let riff_bytes = 36_u32
            .checked_add(data_bytes)
            .ok_or_else(|| io::Error::other("dictation recovery WAV size overflowed"))?;
        if self.file_mut()?.metadata()?.len() < 44 {
            self.file_mut()?.seek(SeekFrom::Start(0))?;
            self.file_mut()?.write_all(b"RIFF")?;
            self.file_mut()?.write_all(&riff_bytes.to_le_bytes())?;
            self.file_mut()?.write_all(b"WAVEfmt ")?;
            self.file_mut()?.write_all(&16_u32.to_le_bytes())?;
            self.file_mut()?.write_all(&1_u16.to_le_bytes())?;
            self.file_mut()?.write_all(&1_u16.to_le_bytes())?;
            self.file_mut()?.write_all(&16_000_u32.to_le_bytes())?;
            self.file_mut()?.write_all(&32_000_u32.to_le_bytes())?;
            self.file_mut()?.write_all(&2_u16.to_le_bytes())?;
            self.file_mut()?.write_all(&16_u16.to_le_bytes())?;
            self.file_mut()?.write_all(b"data")?;
            self.file_mut()?.write_all(&data_bytes.to_le_bytes())?;
        } else {
            self.file_mut()?.seek(SeekFrom::Start(4))?;
            self.file_mut()?.write_all(&riff_bytes.to_le_bytes())?;
            self.file_mut()?.seek(SeekFrom::Start(40))?;
            self.file_mut()?.write_all(&data_bytes.to_le_bytes())?;
        }
        self.file_mut()?.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<PathBuf> {
        self.write_header_lengths()?;
        self.file_mut()?.sync_data()?;
        self.settled = true;
        Ok(self.path.clone())
    }

    pub(crate) fn discard(mut self) -> io::Result<()> {
        self.settled = true;
        let path = self.path.clone();
        drop(self.file.take());
        fs::remove_file(path)
    }
}

#[cfg(any(all(feature = "streaming", feature = "whisper"), test))]
impl Drop for DictationRecoveryCapture {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let _ = self.write_header_lengths();
        if let Some(file) = self.file.as_ref() {
            let _ = file.sync_data();
        }
    }
}

/// Resolve an owned recovery WAV without following a planted symlink or
/// accepting an arbitrary path supplied by a caller.
pub fn validate_recovery_audio_path(path: &Path) -> io::Result<PathBuf> {
    validate_recovery_audio_path_in(&recovery_audio_root(), path)
}

fn validate_recovery_audio_path_in(root: &Path, path: &Path) -> io::Result<PathBuf> {
    crate::policy_fs::ensure_owner_only_directory(root)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "dictation recovery audio is not a regular file",
        ));
    }
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "dictation recovery audio is outside the private recovery folder",
        ));
    }
    crate::policy_fs::ensure_owner_only_file(&canonical_path)?;
    Ok(canonical_path)
}

pub fn retire_recovery_audio(path: &Path) -> io::Result<()> {
    retire_recovery_audio_from(&recovery_audio_root(), path)
}

fn retire_recovery_audio_from(root: &Path, path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let safe_path = validate_recovery_audio_path_in(root, path)?;
    fs::remove_file(safe_path)
}

pub fn recovery_audio_duration_secs(path: &Path) -> io::Result<f64> {
    let safe_path = validate_recovery_audio_path(path)?;
    let reader = hound::WavReader::open(safe_path).map_err(io::Error::other)?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != 16_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "dictation recovery audio is not 16 kHz mono WAV",
        ));
    }
    Ok(reader.duration() as f64 / f64::from(spec.sample_rate))
}

pub fn history_path() -> PathBuf {
    crate::config::Config::minutes_dir().join("dictation-history.json")
}

pub fn load_recent(limit: usize) -> io::Result<Vec<DictationMemoryRecord>> {
    load_recent_from(&history_path(), limit)
}

pub fn find_record(id: &str) -> io::Result<Option<DictationMemoryRecord>> {
    Ok(load_recent(MAX_HISTORY_RECORDS)?
        .into_iter()
        .find(|record| record.id == id))
}

pub fn append_record(record: DictationMemoryRecord) -> io::Result<()> {
    let _guard = HISTORY_WRITE
        .lock()
        .map_err(|_| io::Error::other("history write unavailable"))?;
    append_record_managed(
        &history_path(),
        &recovery_audio_root(),
        record,
        MAX_HISTORY_RECORDS,
    )
}

/// Save an explicitly accepted preview only if its source is still current.
/// Serialize with capture/history writes; model work never holds this lock.
pub fn replace_record_if_unchanged(
    expected: &DictationMemoryRecord,
    record: DictationMemoryRecord,
) -> io::Result<()> {
    let _guard = HISTORY_WRITE
        .lock()
        .map_err(|_| io::Error::other("history write unavailable"))?;
    replace_record_at(&history_path(), &recovery_audio_root(), expected, record)
}
fn replace_record_at(
    path: &Path,
    recovery_root: &Path,
    expected: &DictationMemoryRecord,
    record: DictationMemoryRecord,
) -> io::Result<()> {
    let current = load_recent_from(path, MAX_HISTORY_RECORDS)?
        .into_iter()
        .find(|item| item.id == expected.id);
    if record.id != expected.id || current.as_ref() != Some(expected) {
        return Err(io::Error::other(
            "This dictation changed. Refresh it and preview recovery again.",
        ));
    }
    append_record_managed(path, recovery_root, record, MAX_HISTORY_RECORDS)
}

/// Read only an owned, bounded recovery WAV. No general asset/file access is enabled.
pub fn read_recovery_audio_preview(path: &Path) -> io::Result<Vec<u8>> {
    read_recovery_audio_preview_in(&recovery_audio_root(), path)
}
fn read_recovery_audio_preview_in(recovery_root: &Path, path: &Path) -> io::Result<Vec<u8>> {
    let safe_path = validate_recovery_audio_path_in(recovery_root, path)?;
    let root = cap_std::fs::Dir::open_ambient_dir(recovery_root, cap_std::ambient_authority())?;
    let name = safe_path
        .file_name()
        .ok_or_else(|| io::Error::other("Invalid recovery file"))?;
    if !root.symlink_metadata(name)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Recovery audio changed.",
        ));
    }
    // Capability-relative open prevents a swapped symlink from escaping the recovery root.
    let file = root.open(name)?;
    if file.metadata()?.len() > MAX_PREVIEW_AUDIO_BYTES {
        return Err(io::Error::other("This recording is too large for inline playback. Its audio is still saved for recovery."));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PREVIEW_AUDIO_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PREVIEW_AUDIO_BYTES {
        return Err(io::Error::other(
            "Recovery audio grew beyond the playback limit.",
        ));
    }
    let reader = hound::WavReader::new(std::io::Cursor::new(&bytes)).map_err(io::Error::other)?;
    if reader.spec().channels != 1 || reader.spec().sample_rate != 16_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Recovery audio is not16 kHz mono WAV.",
        ));
    }
    Ok(bytes)
}

fn append_record_managed(
    path: &Path,
    recovery_root: &Path,
    record: DictationMemoryRecord,
    max_records: usize,
) -> io::Result<()> {
    let previous = load_recent_from(path, max_records).unwrap_or_default();
    append_record_to(path, record, max_records)?;

    // The history is the ownership manifest for retained recovery audio. Once
    // a record is replaced or ages out, retire only paths that no remaining
    // record owns. The history write succeeds first, so a failed cleanup can
    // never lose the user's only recovery pointer.
    let retained: HashSet<PathBuf> = load_recent_from(path, max_records)?
        .into_iter()
        .filter_map(|record| record.recovery_audio_path)
        .collect();
    for stale_path in previous
        .into_iter()
        .filter_map(|record| record.recovery_audio_path)
        .filter(|path| !retained.contains(path))
    {
        if let Err(error) = retire_recovery_audio_from(recovery_root, &stale_path) {
            eprintln!(
                "[dictation-memory] could not retire unreferenced recovery audio {}: {error}",
                stale_path.display()
            );
        }
    }
    Ok(())
}

/// Recovery WAVs are written before transcription begins. If Minutes exits
/// unexpectedly, they can exist without a history record. Return only valid,
/// private files not already owned by the history manifest so the desktop can
/// adopt them into its recoverable UI on the next launch.
pub fn orphaned_recovery_audio() -> io::Result<Vec<PathBuf>> {
    orphaned_recovery_audio_from(&recovery_audio_root(), &history_path())
}

fn orphaned_recovery_audio_from(root: &Path, history: &Path) -> io::Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    crate::policy_fs::ensure_owner_only_directory(root)?;
    let owned: HashSet<PathBuf> = load_recent_from(history, MAX_HISTORY_RECORDS)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|record| record.recovery_audio_path)
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    let canonical_root = root.canonicalize()?;
    let mut orphaned = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("wav")
        {
            continue;
        }
        crate::policy_fs::ensure_owner_only_file(&path)?;
        let canonical = path.canonicalize()?;
        if canonical.starts_with(&canonical_root) && !owned.contains(&canonical) {
            orphaned.push(canonical);
        }
    }
    orphaned.sort();
    Ok(orphaned)
}

fn load_recent_from(path: &Path, limit: usize) -> io::Result<Vec<DictationMemoryRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let data = fs::read_to_string(path)?;
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut records: Vec<DictationMemoryRecord> = serde_json::from_str(&data)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    records.sort_by_key(|r| std::cmp::Reverse(r.captured_at));
    if limit > 0 && records.len() > limit {
        records.truncate(limit);
    }
    Ok(records)
}

fn append_record_to(
    path: &Path,
    record: DictationMemoryRecord,
    max_records: usize,
) -> io::Result<()> {
    let mut records = load_recent_from(path, max_records.max(1)).unwrap_or_default();
    records.retain(|existing| existing.id != record.id);
    records.insert(0, record);
    records.sort_by_key(|r| std::cmp::Reverse(r.captured_at));
    if records.len() > max_records {
        records.truncate(max_records);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut tmp, &records).map_err(io::Error::other)?;
        tmp.persist(path).map_err(|error| error.error)?;
    }
    Ok(())
}

fn record_id(
    captured_at: &DateTime<Local>,
    cleaned_text: &str,
    duration_secs: f64,
    engine_id: &str,
) -> String {
    let mut hasher = DefaultHasher::new();
    captured_at
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .hash(&mut hasher);
    cleaned_text.hash(&mut hasher);
    duration_secs.to_bits().hash(&mut hasher);
    engine_id.hash(&mut hasher);
    format!(
        "dict-{}-{:016x}",
        captured_at.format("%Y%m%d%H%M%S"),
        hasher.finish()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn sample_record(offset: i64, text: &str) -> DictationMemoryRecord {
        let captured_at = Local.timestamp_opt(1_700_000_000 + offset, 0).unwrap();
        DictationMemoryRecord::from_parts(
            captured_at,
            DictationMemoryInput {
                raw_text: text.into(),
                cleaned_text: text.into(),
                pre_command_text: None,
                commands_applied: Vec::new(),
                duration_secs: 1.5,
                engine_id: "whisper:base".into(),
                engine_descriptor_version: Some("base".into()),
                vocabulary_mode: None,
                vocabulary_used: Vec::new(),
                destination: "clipboard".into(),
                insertion: DictationInsertionMemory {
                    outcome: "copied".into(),
                    method: "clipboard_only".into(),
                    verified: true,
                    clipboard_restored: false,
                    message: "Copied dictation to the clipboard.".into(),
                },
                target_context: Some(DictationTargetContext {
                    platform: "macos".into(),
                    app_name: Some("Notes".into()),
                }),
                file_path: None,
                daily_note_appended: false,
                recovery_audio_path: None,
            },
        )
    }

    #[test]
    fn append_record_keeps_newest_first_and_truncates() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");

        append_record_to(&path, sample_record(0, "old"), 2).unwrap();
        append_record_to(&path, sample_record(2, "new"), 2).unwrap();
        append_record_to(&path, sample_record(1, "middle"), 2).unwrap();

        let records = load_recent_from(&path, 10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].cleaned_text, "new");
        assert_eq!(records[1].cleaned_text, "middle");
    }

    #[test]
    fn legacy_record_without_voice_edit_fields_still_deserializes() {
        let record = sample_record(0, "hello");
        let mut value = serde_json::to_value(record).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("preCommandText");
        object.remove("commandsApplied");

        let decoded: DictationMemoryRecord = serde_json::from_value(value).unwrap();
        assert!(decoded.pre_command_text.is_none());
        assert!(decoded.commands_applied.is_empty());
    }

    #[test]
    fn append_record_replaces_duplicate_id() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        let mut record = sample_record(0, "first");
        let id = record.id.clone();

        append_record_to(&path, record.clone(), 10).unwrap();
        record.cleaned_text = "updated".into();
        append_record_to(&path, record, 10).unwrap();

        let records = load_recent_from(&path, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].cleaned_text, "updated");
    }

    #[test]
    fn recovery_capture_is_readable_after_each_incremental_chunk() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("recovery");
        let mut capture = DictationRecoveryCapture::create_in(&root).unwrap();
        capture.append_samples(&[0.25; 1600]).unwrap();
        let path = capture.path().to_path_buf();

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.duration(), 1600);
        drop(reader);

        capture.append_samples(&[-0.25; 800]).unwrap();
        drop(capture); // Simulate an ordinary unwind before explicit finish.
        assert_eq!(hound::WavReader::open(path).unwrap().duration(), 2400);
    }

    #[test]
    fn explicit_discard_removes_recovery_audio() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("recovery");
        let mut capture = DictationRecoveryCapture::create_in(&root).unwrap();
        capture.append_samples(&[0.1; 160]).unwrap();
        let path = capture.path().to_path_buf();
        capture.discard().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn crash_recovery_scan_returns_only_unclaimed_private_wavs() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("recovery");
        let history = dir.path().join("history.json");

        let mut claimed_capture = DictationRecoveryCapture::create_in(&root).unwrap();
        claimed_capture.append_samples(&[0.2; 320]).unwrap();
        let claimed_path = claimed_capture.finish().unwrap();
        let mut claimed = sample_record(0, "claimed");
        claimed.recovery_audio_path = Some(claimed_path);
        append_record_to(&history, claimed, 10).unwrap();

        let mut orphan_capture = DictationRecoveryCapture::create_in(&root).unwrap();
        orphan_capture.append_samples(&[0.3; 640]).unwrap();
        let orphan_path = orphan_capture.path().canonicalize().unwrap();
        drop(orphan_capture); // Simulate a process exiting before history commit.

        assert_eq!(
            orphaned_recovery_audio_from(&root, &history).unwrap(),
            vec![orphan_path]
        );
    }

    #[test]
    fn history_manifest_retires_audio_only_after_record_is_replaced() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("recovery");
        let history = dir.path().join("history.json");
        let mut capture = DictationRecoveryCapture::create_in(&root).unwrap();
        capture.append_samples(&[0.2; 320]).unwrap();
        let recovery_path = capture.finish().unwrap();

        let mut recoverable = sample_record(0, "");
        recoverable.recovery_audio_path = Some(recovery_path.clone());
        append_record_managed(&history, &root, recoverable.clone(), 10).unwrap();
        assert!(recovery_path.exists());

        recoverable.cleaned_text = "recovered".into();
        recoverable.recovery_audio_path = None;
        append_record_managed(&history, &root, recoverable, 10).unwrap();
        assert!(!recovery_path.exists());
        assert_eq!(load_recent_from(&history, 10).unwrap().len(), 1);
    }
    #[test]
    fn all_retained_history_and_legacy_original_are_available() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        for n in 0..100 {
            append_record_to(
                &path,
                sample_record(n, &format!("entry{n}")),
                MAX_HISTORY_RECORDS,
            )
            .unwrap();
        }
        let records = load_recent_from(&path, MAX_HISTORY_RECORDS).unwrap();
        assert_eq!(records.len(), 100);
        assert_eq!(records.last().unwrap().cleaned_text, "entry0");
        let legacy: DictationMemoryRecord =
            serde_json::from_value(serde_json::to_value(&records[0]).unwrap()).unwrap();
        assert!(legacy.original_transcript.is_none());
    }
    #[test]
    fn repeated_recovery_preserves_the_first_transcript_and_delivery() {
        let first = sample_record(0, "first");
        let mut second = first.clone();
        second.cleaned_text = "second".into();
        second.preserve_original_from(&first);
        let mut third = second.clone();
        third.cleaned_text = "third".into();
        third.preserve_original_from(&second);
        assert_eq!(third.original_transcript.unwrap().cleaned_text, "first");
    }
    #[test]
    fn stale_preview_cannot_replace_newer_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        let root = dir.path().join("audio");
        let before = sample_record(0, "first");
        append_record_managed(&path, &root, before.clone(), 100).unwrap();
        let mut newer = before.clone();
        newer.cleaned_text = "newer".into();
        append_record_managed(&path, &root, newer.clone(), 100).unwrap();
        let mut candidate = before.clone();
        candidate.cleaned_text = "candidate".into();
        assert!(replace_record_at(&path, &root, &before, candidate).is_err());
        assert_eq!(load_recent_from(&path, 100).unwrap(), vec![newer]);
    }
    #[test]
    fn accepted_preview_keeps_id_original_and_other_records() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.json");
        let root = dir.path().join("audio");
        let before = sample_record(0, "first");
        append_record_managed(&path, &root, before.clone(), 100).unwrap();
        append_record_managed(&path, &root, sample_record(1, "other"), 100).unwrap();
        let mut candidate = before.clone();
        candidate.cleaned_text = "candidate".into();
        candidate.preserve_original_from(&before);
        replace_record_at(&path, &root, &before, candidate).unwrap();
        let records = load_recent_from(&path, 100).unwrap();
        assert_eq!(records.len(), 2);
        let accepted = records.iter().find(|item| item.id == before.id).unwrap();
        assert_eq!(accepted.cleaned_text, "candidate");
        assert_eq!(
            accepted.original_transcript.as_ref().unwrap().cleaned_text,
            "first"
        );
    }
    #[test]
    fn audio_preview_reads_owned_wav_without_deleting_it() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("audio");
        let mut capture = DictationRecoveryCapture::create_in(&root).unwrap();
        capture.append_samples(&[0.25; 1600]).unwrap();
        let path = capture.finish().unwrap();
        let bytes = read_recovery_audio_preview_in(&root, &path).unwrap();
        assert!(bytes.starts_with(b"RIFF"));
        assert!(path.exists());
    }
    #[test]
    fn audio_preview_rejects_oversize_files_before_reading() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("audio");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("large.wav");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_PREVIEW_AUDIO_BYTES + 1)
            .unwrap();
        assert!(read_recovery_audio_preview_in(&root, &path).is_err());
        assert!(path.exists());
    }
    #[cfg(unix)]
    #[test]
    fn audio_preview_rejects_outside_and_symlink_without_changing_outside_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("audio");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside.wav");
        fs::write(&outside, b"private other data").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_recovery_audio_preview_in(&root, &outside).is_err());
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let link = root.join("link.wav");
        symlink(&outside, &link).unwrap();
        assert!(read_recovery_audio_preview_in(&root, &link).is_err());
    }
}
