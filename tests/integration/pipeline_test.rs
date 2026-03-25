use minutes_core::{Config, ContentType};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Create a test config that works with or without the whisper feature.
/// Uses the tiny model (smallest/fastest) when whisper is enabled.
fn test_config(output_dir: PathBuf) -> Config {
    Config {
        output_dir,
        transcription: minutes_core::config::TranscriptionConfig {
            model: "tiny".into(),
            model_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".minutes")
                .join("models"),
            min_words: 10,
            language: Some("en".into()),
            vad_model: "silero-v6.2.0".into(),
            ..minutes_core::config::TranscriptionConfig::default()
        },
        ..Config::default()
    }
}

/// Helper to create a test WAV file with hound.
fn create_test_wav(path: &std::path::Path, duration_secs: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    let samples = (16000.0 * duration_secs) as usize;
    for i in 0..samples {
        let t = i as f32 / 16000.0;
        let sample = (10000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16;
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn full_pipeline_meeting() {
    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("test-meeting.wav");
    create_test_wav(&wav, 2.0);

    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(&wav, ContentType::Meeting, Some("Test Meeting"), &config);
    assert!(result.is_ok(), "pipeline failed: {:?}", result.err());

    let result = result.unwrap();
    assert!(result.path.exists());
    assert!(result.path.to_str().unwrap().contains("test-meeting"));
    assert!(!result.path.to_str().unwrap().contains("memos"));

    let content = fs::read_to_string(&result.path).unwrap();
    assert!(content.contains("type: meeting"));
    assert!(content.contains("title: Test Meeting"));
    assert!(content.contains("## Transcript"));
}

#[test]
fn full_pipeline_memo() {
    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("test-memo.wav");
    create_test_wav(&wav, 1.0);

    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(&wav, ContentType::Memo, None, &config);
    assert!(result.is_ok(), "pipeline failed: {:?}", result.err());

    let result = result.unwrap();
    assert!(result.path.to_str().unwrap().contains("memos"));

    let content = fs::read_to_string(&result.path).unwrap();
    assert!(content.contains("type: memo"));
    assert!(content.contains("source: voice-memos"));
}

#[test]
fn pipeline_rejects_empty_audio() {
    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("empty.wav");
    fs::write(&wav, "").unwrap();

    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(&wav, ContentType::Memo, None, &config);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("empty") || err.contains("zero"));
}

#[test]
fn pipeline_rejects_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(
        std::path::Path::new("/nonexistent/file.wav"),
        ContentType::Memo,
        None,
        &config,
    );
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn markdown_permissions_are_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("test.wav");
    create_test_wav(&wav, 1.0);

    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(&wav, ContentType::Memo, None, &config).unwrap();
    let mode = fs::metadata(&result.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "output file must have 0600 permissions");
}

#[test]
fn filename_collision_appends_suffix() {
    let dir = TempDir::new().unwrap();
    let config = test_config(dir.path().join("output"));

    let wav1 = dir.path().join("test1.wav");
    create_test_wav(&wav1, 1.0);
    let result1 =
        minutes_core::process(&wav1, ContentType::Meeting, Some("Same Title"), &config).unwrap();

    let wav2 = dir.path().join("test2.wav");
    create_test_wav(&wav2, 1.0);
    let result2 =
        minutes_core::process(&wav2, ContentType::Meeting, Some("Same Title"), &config).unwrap();

    assert!(result1.path.exists());
    assert!(result2.path.exists());
    assert_ne!(result1.path, result2.path);

    let name2 = result2.path.file_name().unwrap().to_str().unwrap();
    assert!(
        name2.contains("-2"),
        "collision should append -2, got: {}",
        name2
    );
}

#[test]
fn search_filters_by_content_type() {
    let dir = TempDir::new().unwrap();
    let config = test_config(dir.path().join("output"));

    let wav1 = dir.path().join("m1.wav");
    create_test_wav(&wav1, 1.0);
    minutes_core::process(&wav1, ContentType::Meeting, Some("Meeting One"), &config).unwrap();

    let wav2 = dir.path().join("m2.wav");
    create_test_wav(&wav2, 1.0);
    minutes_core::process(&wav2, ContentType::Memo, Some("Memo One"), &config).unwrap();

    let filters = minutes_core::search::SearchFilters {
        content_type: Some("memo".into()),
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };

    // Search for content that exists in the output (varies by whisper vs placeholder)
    let results = minutes_core::search::search("", &config, &filters).unwrap();
    // Should have at least the memo (might not match text search with empty query,
    // but empty query should return all files)
    let memo_results: Vec<_> = results
        .iter()
        .filter(|r| r.content_type == "memo")
        .collect();
    assert!(!memo_results.is_empty(), "should find the memo");
}

#[test]
fn output_dir_auto_created() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("deeply").join("nested").join("output");
    assert!(!output.exists());

    let config = test_config(output.clone());
    let wav = dir.path().join("test.wav");
    create_test_wav(&wav, 1.0);
    let result = minutes_core::process(&wav, ContentType::Meeting, None, &config);
    assert!(result.is_ok());
    assert!(output.exists());
}

/// Test real whisper transcription with the tiny model.
/// Only runs when the `whisper` feature is enabled AND the tiny model is downloaded.
#[test]
#[cfg(feature = "whisper")]
fn whisper_real_transcription() {
    let model_path = dirs::home_dir()
        .unwrap()
        .join(".minutes/models/ggml-tiny.bin");
    if !model_path.exists() {
        eprintln!("SKIPPED: whisper_real_transcription — tiny model not found");
        return;
    }

    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("speech.wav");
    create_test_wav(&wav, 2.0);

    let mut config = test_config(dir.path().join("output"));
    config.transcription.min_words = 1; // Low threshold for blank audio

    let result = minutes_core::process(&wav, ContentType::Memo, Some("Whisper Test"), &config);
    assert!(
        result.is_ok(),
        "pipeline should succeed: {:?}",
        result.err()
    );

    let result = result.unwrap();
    let content = fs::read_to_string(&result.path).unwrap();
    assert!(content.contains("## Transcript"));
    assert!(content.contains("title: Whisper Test"));
    assert!(
        !content.contains("whisper-rs not yet integrated")
            && !content.contains("whisper feature not enabled"),
        "should be real whisper output, not placeholder"
    );
}

/// Test no-speech detection with whisper on near-silent audio.
#[test]
#[cfg(feature = "whisper")]
fn whisper_no_speech_detection() {
    let model_path = dirs::home_dir()
        .unwrap()
        .join(".minutes/models/ggml-tiny.bin");
    if !model_path.exists() {
        return;
    }

    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("silence.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
    for _ in 0..16000 {
        writer.write_sample(10i16).unwrap();
    }
    writer.finalize().unwrap();

    let config = test_config(dir.path().join("output"));
    let result = minutes_core::process(&wav, ContentType::Memo, None, &config).unwrap();
    let content = fs::read_to_string(&result.path).unwrap();

    assert!(
        content.contains("status: no-speech") || content.contains("No speech detected"),
        "near-silent audio should trigger no-speech detection"
    );
}
