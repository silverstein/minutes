//! Explicit, model-backed qualification using repository synthetic speech only.
//! No microphone, playback, network provider or user's meeting data is used.
#![cfg(feature = "whisper")]

use minutes_core::{config::Config, markdown::ContentType, pipeline};
use std::path::Path;
use std::process::Command;

// These manual qualifications change process-wide profile paths.
static QUALIFICATION_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ffmpeg(args: &[&std::ffi::OsStr]) {
    // Integration tests cannot use the crate-private engine process boundary.
    // This subprocess only renders repository fixture audio for the test.
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-threads", "1"])
        .args(args)
        .output()
        .expect("ffmpeg is required for this manual qualification");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_source(input: &Path, output: &Path, filter: &str) {
    ffmpeg(&[
        "-i".as_ref(),
        input.as_os_str(),
        "-af".as_ref(),
        filter.as_ref(),
        "-ac".as_ref(),
        "1".as_ref(),
        "-ar".as_ref(),
        "16000".as_ref(),
        output.as_os_str(),
    ]);
}

#[test]
#[ignore = "requires an installed Whisper small model; set MINUTES_ASR_QUALIFICATION_MODELS and MINUTES_ASR_EVIDENCE_DIR"]
fn quieter_overlapping_source_survives_real_whisper_with_vad_disabled() {
    let _environment = QUALIFICATION_ENV.lock().unwrap();
    let models = std::env::var_os("MINUTES_ASR_QUALIFICATION_MODELS")
        .expect("explicit model directory required");
    assert!(Path::new(&models).join("ggml-small.bin").is_file());
    let evidence = std::env::var_os("MINUTES_ASR_EVIDENCE_DIR")
        .expect("explicit synthetic evidence directory required");
    std::fs::create_dir_all(&evidence).unwrap();
    let temp = tempfile::tempdir().unwrap();
    // Isolate all notes,
    // vocabulary, logs and calendar configuration even when a contributor
    // invokes it without setting a separate Minutes profile themselves.
    let profile = temp.path().join("profile");
    let config_path = profile.join("config.toml");
    std::env::set_var("MINUTES_DATA_DIR", &profile);
    std::env::set_var("MINUTES_CONFIG_PATH", &config_path);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let voice = temp.path().join("overlap.voice.wav");
    let system = temp.path().join("overlap.system.wav");
    make_source(
        &repo.join("tests/eval/fixtures/audio/apple-speech-dictation.wav"),
        &voice,
        "adelay=4000:all=1,volume=0.12",
    );
    make_source(
        &repo.join("tests/eval/fixtures/audio/apple-speech-meeting.wav"),
        &system,
        "adelay=2000:all=1,volume=0.8",
    );
    let anchor = temp.path().join("overlap.wav");
    ffmpeg(&[
        "-i".as_ref(),
        voice.as_os_str(),
        "-i".as_ref(),
        system.as_os_str(),
        "-filter_complex".as_ref(),
        "[0:a][1:a]amix=inputs=2".as_ref(),
        anchor.as_os_str(),
    ]);
    let mut config = Config {
        output_dir: temp.path().join("outputs"),
        ..Config::default()
    };
    config.transcription.model = "small".into();
    config.transcription.model_path = models.into();
    config.transcription.engine = "whisper".into();
    config.transcription.language = Some("en".into());
    config.transcription.vad_model.clear();
    config.diarization.engine = "none".into();
    config.summarization.engine = "none".into();
    config.calendar.enabled = false;
    config.save_to(&config_path).unwrap();

    // The baseline has identical mixed bytes, without sibling sources.
    let baseline = temp.path().join("baseline.wav");
    std::fs::copy(&anchor, &baseline).unwrap();
    let baseline_result = pipeline::process(
        &baseline,
        ContentType::Meeting,
        Some("Synthetic mixed baseline"),
        &config,
    )
    .unwrap();
    let baseline_text = std::fs::read_to_string(baseline_result.path).unwrap();
    std::fs::write(
        Path::new(&evidence).join("mixed-baseline.md"),
        baseline_text,
    )
    .unwrap();

    let result = pipeline::process(
        &anchor,
        ContentType::Meeting,
        Some("Synthetic independent sources"),
        &config,
    )
    .unwrap();
    let content = std::fs::read_to_string(result.path).unwrap();
    std::fs::write(
        Path::new(&evidence).join("independent-sources.md"),
        &content,
    )
    .unwrap();
    let voice_text = content
        .lines()
        .filter(|line| line.starts_with("[MICROPHONE "))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let system_text = content
        .lines()
        .filter(|line| line.starts_with("[SYSTEM "))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    assert!(
        voice_text.contains("dictation"),
        "quieter microphone words missing: {voice_text}"
    );
    assert!(
        system_text.contains("review") && system_text.contains("product"),
        "system words missing: {system_text}"
    );
    let first_start = |source: &str| {
        let prefix = format!("[{source} ");
        let line = content
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap();
        let timestamp = line
            .strip_prefix(&prefix)
            .unwrap()
            .split(']')
            .next()
            .unwrap();
        let (minutes, seconds) = timestamp.split_once(':').unwrap();
        minutes.parse::<f64>().unwrap() * 60.0 + seconds.parse::<f64>().unwrap()
    };
    assert!(
        (2.5..=5.5).contains(&first_start("MICROPHONE")),
        "microphone onset must remain near the original 4s clock: {content}"
    );
    assert!(
        (0.5..=3.5).contains(&first_start("SYSTEM")),
        "system onset must remain near the original 2s clock: {content}"
    );
}

#[test]
#[ignore = "requires an installed Whisper small model and ffmpeg; set MINUTES_ASR_QUALIFICATION_MODELS and MINUTES_ASR_EVIDENCE_DIR"]
fn identical_synthetic_stems_suppress_echo_only_when_enabled() {
    let _environment = QUALIFICATION_ENV.lock().unwrap();
    let models = std::env::var_os("MINUTES_ASR_QUALIFICATION_MODELS")
        .expect("explicit model directory required");
    assert!(Path::new(&models).join("ggml-small.bin").is_file());
    let evidence = std::env::var_os("MINUTES_ASR_EVIDENCE_DIR")
        .expect("explicit synthetic evidence directory required");
    std::fs::create_dir_all(&evidence).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    let config_path = profile.join("config.toml");
    std::env::set_var("MINUTES_DATA_DIR", &profile);
    std::env::set_var("MINUTES_CONFIG_PATH", &config_path);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let anchor = temp.path().join("echo.wav");
    make_source(
        &repo.join("tests/eval/fixtures/audio/apple-speech-meeting.wav"),
        &anchor,
        "adelay=2000:all=1",
    );
    // Identical synthetic bytes give a deterministic echo case without live I/O.
    std::fs::copy(&anchor, anchor.with_extension("voice.wav")).unwrap();
    std::fs::copy(&anchor, anchor.with_extension("system.wav")).unwrap();
    let mut config = Config {
        output_dir: temp.path().join("outputs"),
        ..Config::default()
    };
    config.transcription.model = "small".into();
    config.transcription.model_path = models.into();
    config.transcription.engine = "whisper".into();
    config.transcription.language = Some("en".into());
    config.transcription.vad_model.clear();
    config.diarization.engine = "none".into();
    config.summarization.engine = "none".into();
    config.calendar.enabled = false;

    let mut transcripts = Vec::new();
    for enabled in [false, true] {
        config.transcription.cross_source_echo_suppression = enabled;
        config.save_to(&config_path).unwrap();
        let result = pipeline::process(
            &anchor,
            ContentType::Meeting,
            Some(if enabled {
                "Synthetic echo enabled"
            } else {
                "Synthetic echo disabled"
            }),
            &config,
        )
        .unwrap();
        let content = std::fs::read_to_string(result.path).unwrap();
        std::fs::write(
            Path::new(&evidence).join(format!("echo-suppression-{enabled}.md")),
            &content,
        )
        .unwrap();
        transcripts.push(content);
    }
    let source_lines = |text: &str, source: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with(source))
            .map(str::to_string)
            .collect()
    };
    let system_before = source_lines(&transcripts[0], "[SYSTEM ");
    assert!(!system_before.is_empty());
    assert_eq!(system_before, source_lines(&transcripts[1], "[SYSTEM "));
    let microphone_before = source_lines(&transcripts[0], "[MICROPHONE ");
    let microphone_after = source_lines(&transcripts[1], "[MICROPHONE ");
    assert!(!microphone_before.is_empty());
    assert!(microphone_after.len() < microphone_before.len());
}
