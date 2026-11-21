//! Exercises the real loader, meeting chunker, dispatch, cleanup, and RAII lifetime.
//! A separate test executable isolates the plugin's process-wide ABI cache.
#![cfg(all(feature = "engine-sherpa", unix))]

use minutes_core::{config::Config, sherpa_engine, transcribe};
use std::{path::Path, process::Command};

fn write_audio(path: &Path, amplitudes: &[i16]) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for amplitude in amplitudes {
        for _ in 0..32_000 {
            writer.write_sample(*amplitude).unwrap();
        }
        for _ in 0..32_000 {
            writer.write_sample(0_i16).unwrap();
        }
    }
    writer.finalize().unwrap();
}

#[test]
fn meeting_reuses_model_preserves_text_and_timestamps_and_releases_on_failure() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.transcription.engine = "sherpa".into();
    config.transcription.model_path = temp.path().join("models");
    config.transcription.sherpa_model_dir = temp.path().join("fixture-model").display().to_string();
    config.transcription.vad_model = temp.path().join("absent-vad.bin").display().to_string();
    config.transcription.min_words = 1;
    config.transcription.noise_reduction = false;
    let model = sherpa_engine::model_dir(&config);
    std::fs::create_dir_all(&model).unwrap();
    for (name, size) in sherpa_engine::MODEL_FILES {
        std::fs::File::create(model.join(name))
            .unwrap()
            .set_len(size)
            .unwrap();
    }
    let lib_dir = sherpa_engine::installs_root(&config).join("lib");
    std::fs::create_dir_all(&lib_dir).unwrap();
    let library_path = lib_dir.join(if cfg!(target_os = "macos") {
        "libminutes_sherpa.dylib"
    } else {
        "libminutes_sherpa.so"
    });
    // Integration tests cannot use the crate-private engine process boundary.
    // This subprocess only compiles the checked-in fixture, never handles audio.
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sherpa_reuse/plugin.c"
        ))
        .arg("-o")
        .arg(&library_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // This test executable is dedicated to the fixture; no real plugin may win discovery.
    std::env::set_var("MINUTES_SHERPA_PLUGIN", &library_path);
    let library = unsafe { libloading::Library::new(&library_path).unwrap() };
    let reset = unsafe {
        library
            .get::<unsafe extern "C" fn(i32)>(b"fixture_reset")
            .unwrap()
    };
    let count = unsafe {
        library
            .get::<unsafe extern "C" fn(i32) -> i32>(b"fixture_count")
            .unwrap()
    };
    let counts = || (0..5).map(|i| unsafe { count(i) }).collect::<Vec<_>>();
    let meeting = temp.path().join("meeting.wav");
    write_audio(&meeting, &[9830, 16384, 26214]);

    unsafe { reset(0) };
    let result = transcribe::transcribe_meeting(&meeting, &config).unwrap();
    assert_eq!(
        counts(),
        [1, 1, 3, 0, 1],
        "one model, three chunks, released at return"
    );
    assert_eq!(result.text, "[0:00] Bonjour les amis de Montréal.\n[0:04] Zażółć gęślą jaźń podczas spotkania.\n[0:08] We will review the project tomorrow.\n");
    assert_eq!(result.stats.raw_segments, 3);
    assert_eq!(result.stats.audio_duration_secs, 12.0);
    assert!(!result.stats.sherpa_fell_back_to_whisper);
    assert!(result.stats.samples_after_silence_strip > 0);
    assert!(!result.stats.diagnosis().contains("removed ALL audio"));

    // A separate meeting must not inherit the previous model/session.
    let again = transcribe::transcribe_meeting(&meeting, &config).unwrap();
    assert_eq!(again.text, result.text);
    assert_eq!(counts(), [2, 2, 6, 0, 1]);

    // Standalone dispatch still gets a fresh model for each operation.
    unsafe { reset(0) };
    for amplitude in [9830, 16384, 26214] {
        let clip = temp.path().join("clip.wav");
        write_audio(&clip, &[amplitude]);
        let standalone = transcribe::transcribe(&clip, &config).unwrap();
        assert!(result.text.contains(standalone.segments[0].text.as_str()));
    }
    assert_eq!(counts(), [3, 3, 3, 0, 1]);

    for (mode, expected) in [
        (1, vec![1, 0, 0, 0, 0]),
        (2, vec![1, 1, 2, 0, 1]),
        (3, vec![1, 1, 2, 0, 1]),
    ] {
        unsafe { reset(mode) };
        let error = transcribe::transcribe_meeting(&meeting, &config).unwrap_err();
        // No Whisper model is supplied. Its model error proves fallback was attempted,
        // instead of leaking the Sherpa error or returning a partial meeting as complete.
        assert!(
            matches!(
                error,
                minutes_core::error::TranscribeError::ModelNotFound(_)
            ),
            "{error:?}"
        );
        assert_eq!(counts(), expected, "failure mode {mode}");
    }
    unsafe { reset(0) };
    assert_eq!(
        transcribe::transcribe_meeting(&meeting, &config)
            .unwrap()
            .text,
        result.text
    );
    assert_eq!(counts(), [1, 1, 3, 0, 1]);
}
