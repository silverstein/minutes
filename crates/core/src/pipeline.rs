use crate::config::Config;
use crate::error::MinutesError;
use crate::markdown::{self, ContentType, Frontmatter, OutputStatus, WriteResult};
use crate::transcribe;
use chrono::Local;
use std::path::Path;

// ──────────────────────────────────────────────────────────────
// Pipeline orchestration:
//
//   Audio → Transcribe → [Diarize] → [Summarize] → Write Markdown
//                           ▲             ▲
//                           │             │
//                     config.diarization  config.summarization
//                     .engine != "none"   .engine != "none"
//
// Transcription uses whisper-rs (whisper.cpp) with symphonia for
// format conversion (m4a/mp3/ogg → 16kHz mono PCM).
// Phase 1b adds Diarize + Summarize with if-guards.
// ──────────────────────────────────────────────────────────────

/// Process an audio file through the full pipeline.
pub fn process(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
) -> Result<WriteResult, MinutesError> {
    let start = std::time::Instant::now();
    tracing::info!(
        file = %audio_path.display(),
        content_type = ?content_type,
        "starting pipeline"
    );

    // Verify file exists and is not empty
    let metadata = std::fs::metadata(audio_path)?;
    if metadata.len() == 0 {
        return Err(crate::error::TranscribeError::EmptyAudio.into());
    }

    // Step 1: Transcribe (always)
    tracing::info!(step = "transcribe", file = %audio_path.display(), "transcribing audio");
    let transcript = transcribe::transcribe(audio_path, config)?;

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        step = "transcribe",
        words = word_count,
        "transcription complete"
    );

    // Check minimum word threshold
    let status = if word_count < config.transcription.min_words {
        tracing::warn!(
            words = word_count,
            min = config.transcription.min_words,
            "below minimum word threshold — marking as no-speech"
        );
        Some(OutputStatus::NoSpeech)
    } else {
        Some(OutputStatus::TranscriptOnly) // Phase 1a: no summarization
    };

    // Step 2: Diarize (Phase 1b, optional — currently skipped)
    // if config.diarization.engine != "none" {
    //     tracing::info!(step = "diarize", "running speaker diarization");
    //     let speakers = diarize::run(audio_path, config)?;
    // }

    // Step 3: Summarize (Phase 1b, optional — currently skipped)
    let summary: Option<String> = None;
    // if config.summarization.engine != "none" {
    //     tracing::info!(step = "summarize", "generating summary");
    //     summary = Some(summarize::run(&transcript, config)?);
    // }

    // Step 4: Write markdown (always)
    let duration = estimate_duration(audio_path);
    let auto_title = title
        .map(String::from)
        .unwrap_or_else(|| generate_title(&transcript));

    let frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: Local::now(),
        duration,
        source: match content_type {
            ContentType::Memo => Some("voice-memo".into()),
            ContentType::Meeting => None,
        },
        status,
        tags: vec![],
        attendees: vec![],
        calendar_event: None,
        people: vec![],
    };

    tracing::info!(step = "write", "writing markdown");
    let result = markdown::write(&frontmatter, &transcript, summary.as_deref(), config)?;

    let elapsed = start.elapsed();
    tracing::info!(
        file = %result.path.display(),
        words = result.word_count,
        elapsed_ms = elapsed.as_millis() as u64,
        "pipeline complete"
    );

    Ok(result)
}

/// Estimate audio duration from file size (rough approximation).
/// 16kHz mono 16-bit WAV ≈ 32KB/sec.
fn estimate_duration(audio_path: &Path) -> String {
    let bytes = std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0);

    // WAV header is 44 bytes, then raw PCM at 32000 bytes/sec (16kHz 16-bit mono)
    let secs = if bytes > 44 { (bytes - 44) / 32_000 } else { 0 };

    let mins = secs / 60;
    let remaining_secs = secs % 60;
    if mins > 0 {
        format!("{}m {}s", mins, remaining_secs)
    } else {
        format!("{}s", remaining_secs)
    }
}

/// Generate a title from the first few words of the transcript.
fn generate_title(transcript: &str) -> String {
    let first_line = transcript
        .lines()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('[')
        })
        .unwrap_or("Untitled Recording");

    let words: Vec<&str> = first_line.split_whitespace().take(8).collect();
    let title = words.join(" ");

    if title.len() > 60 {
        format!("{}...", &title[..57])
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_title_takes_first_words() {
        let transcript = "We need to discuss the new pricing strategy for Q2";
        let title = generate_title(transcript);
        assert_eq!(title, "We need to discuss the new pricing strategy");
    }

    #[test]
    fn generate_title_skips_speaker_labels() {
        let transcript = "[0:00] We need to discuss pricing";
        // Lines starting with [ are skipped for title generation
        let title = generate_title(transcript);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn estimate_duration_formats_correctly() {
        // 32000 bytes/sec * 90 sec + 44 header = 2_880_044 bytes
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.wav");
        let data = vec![0u8; 2_880_044];
        std::fs::write(&path, &data).unwrap();

        let duration = estimate_duration(&path);
        assert_eq!(duration, "1m 30s");
    }
}
