use minutes_core::{Config, ContentType};
use std::fs;
use tempfile::TempDir;

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

    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    let result = minutes_core::process(&wav, ContentType::Meeting, Some("Test Meeting"), &config);
    assert!(result.is_ok());

    let result = result.unwrap();
    assert!(result.path.exists());
    assert!(result.path.to_str().unwrap().contains("test-meeting"));
    assert!(!result.path.to_str().unwrap().contains("memos"));

    // Verify file contents
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

    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    let result = minutes_core::process(&wav, ContentType::Memo, None, &config);
    assert!(result.is_ok());

    let result = result.unwrap();
    assert!(result.path.to_str().unwrap().contains("memos"));

    let content = fs::read_to_string(&result.path).unwrap();
    assert!(content.contains("type: memo"));
    assert!(content.contains("source: voice-memo"));
}

#[test]
fn pipeline_rejects_empty_audio() {
    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("empty.wav");
    fs::write(&wav, "").unwrap();

    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    let result = minutes_core::process(&wav, ContentType::Memo, None, &config);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("empty") || err.contains("zero"));
}

#[test]
fn pipeline_rejects_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    let result = minutes_core::process(
        std::path::Path::new("/nonexistent/file.wav"),
        ContentType::Memo,
        None,
        &config,
    );
    assert!(result.is_err());
}

#[test]
fn markdown_permissions_are_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("test.wav");
    create_test_wav(&wav, 1.0);

    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    let result = minutes_core::process(&wav, ContentType::Memo, None, &config).unwrap();
    let mode = fs::metadata(&result.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "output file must have 0600 permissions");
}

#[test]
fn filename_collision_appends_suffix() {
    let dir = TempDir::new().unwrap();
    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    // Process twice with the same title
    let wav1 = dir.path().join("test1.wav");
    create_test_wav(&wav1, 1.0);
    let result1 =
        minutes_core::process(&wav1, ContentType::Meeting, Some("Same Title"), &config).unwrap();

    let wav2 = dir.path().join("test2.wav");
    create_test_wav(&wav2, 1.0);
    let result2 =
        minutes_core::process(&wav2, ContentType::Meeting, Some("Same Title"), &config).unwrap();

    // Both files should exist with different names
    assert!(result1.path.exists());
    assert!(result2.path.exists());
    assert_ne!(result1.path, result2.path);

    // Second file should have -2 suffix
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
    let config = Config {
        output_dir: dir.path().join("output"),
        ..Config::default()
    };

    // Create a meeting and a memo
    let wav1 = dir.path().join("m1.wav");
    create_test_wav(&wav1, 1.0);
    minutes_core::process(&wav1, ContentType::Meeting, Some("Meeting One"), &config).unwrap();

    let wav2 = dir.path().join("m2.wav");
    create_test_wav(&wav2, 1.0);
    minutes_core::process(&wav2, ContentType::Memo, Some("Memo One"), &config).unwrap();

    // Search with type filter
    let filters = minutes_core::search::SearchFilters {
        content_type: Some("memo".into()),
        since: None,
        attendee: None,
    };
    let results = minutes_core::search::search("placeholder", &config, &filters).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content_type, "memo");
}

#[test]
fn output_dir_auto_created() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("deeply").join("nested").join("output");
    assert!(!output.exists());

    let config = Config {
        output_dir: output.clone(),
        ..Config::default()
    };

    let wav = dir.path().join("test.wav");
    create_test_wav(&wav, 1.0);
    let result = minutes_core::process(&wav, ContentType::Meeting, None, &config);
    assert!(result.is_ok());
    assert!(output.exists());
}
