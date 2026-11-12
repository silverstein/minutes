use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

type CapturedRequest = (String, Vec<u8>);
type ServerHandle = thread::JoinHandle<Vec<CapturedRequest>>;

const OK: &str =
    r#"{"language":"en","segments":[{"start":0.125,"end":0.75,"text":"hello from server"}]}"#;

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let n = stream.read(&mut buffer).unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return (headers, bytes[end + 4..end + 4 + length].to_vec());
            }
        }
    }
}

fn server(
    replies: Vec<(u16, &'static str)>,
    mut after_request: impl FnMut(usize) + Send + 'static,
) -> (String, ServerHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (index, (status, body)) in replies.into_iter().enumerate() {
            let start = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            start.elapsed() < Duration::from_secs(10),
                            "no request received"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            requests.push(read_request(&mut stream));
            after_request(index);
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        // A failure must not trigger an HTTP retry or follow a redirect.
        thread::sleep(Duration::from_millis(100));
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        requests
    });
    (base, handle)
}

fn fixture(root: &Path, seconds: usize) -> (Config, PathBuf) {
    let mut config = Config::default();
    config.transcription.engine = "openai-compatible".into();
    config.output_dir = root.join("meetings");
    std::fs::create_dir_all(&config.output_dir).unwrap();
    let audio = config.output_dir.join("test-recording.wav");
    super::super::write_wav_16k_mono_to_writer(
        std::fs::File::create(&audio).unwrap(),
        &vec![0.2; seconds * 16000],
    )
    .unwrap();
    std::fs::write(audio.with_extension("md"), "---\ntitle: Test recording\ntype: meeting\ndate: 2026-01-01T12:00:00Z\nsensitivity: normal\n---\n").unwrap();
    (config, audio)
}

#[test]
fn multipart_success_keeps_precise_segments_and_anonymous_filename() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        let (base, handle) = server(vec![(200, OK)], |_| {});
        config.transcription.openai_compatible_base_url = base;
        let result = super::super::transcribe_meeting(&audio, &config).unwrap();
        assert_eq!(result.segments[0].start, 0.125);
        assert_eq!(result.segments[0].end, 0.75);
        assert_eq!(result.detected_language.as_deref(), Some("en"));
        assert_eq!(result.text, "[0:00] hello from server\n");
        let requests = handle.join().unwrap();
        assert!(requests[0]
            .0
            .starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        assert!(!requests[0].0.to_ascii_lowercase().contains("authorization"));
        let body = String::from_utf8_lossy(&requests[0].1);
        assert!(body.contains("name=\"response_format\"\r\n\r\nverbose_json"));
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1"));
        assert!(body.contains("filename=\"audio.wav\""));
        assert!(!body.contains("test-recording"));
        assert!(requests[0].1.windows(4).any(|w| w == b"RIFF"));
    });
}

#[test]
fn chunk_limit_includes_multipart_and_offsets_use_source_samples() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 2);
        let (prefix, suffix, _) = multipart_parts(&config).unwrap();
        config.transcription.openai_compatible_max_upload_bytes =
            prefix.len() + suffix.len() + 44 + 32000;
        let (base, handle) = server(vec![(200, OK), (200, OK)], |_| {});
        config.transcription.openai_compatible_base_url = base;
        let result = super::super::transcribe_meeting(&audio, &config).unwrap();
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[1].start, 1.125);
        assert_eq!(result.segments[1].end, 1.75);
        assert_eq!(result.stats.audio_duration_secs, 2.0);
        for (_, body) in handle.join().unwrap() {
            assert_eq!(
                body.len(),
                config.transcription.openai_compatible_max_upload_bytes
            );
        }
    });
}

#[test]
fn missing_key_refuses_before_connect_and_does_not_echo_env_name() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        config.transcription.openai_compatible_base_url =
            format!("http://{}/v1", listener.local_addr().unwrap());
        config.transcription.openai_compatible_api_key_env =
            "MINUTES_TEST_TRANSCRIPTION_UNSET_KEY_1355".into();
        let error = super::super::transcribe(&audio, &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("environment variable is missing"));
        assert!(!error.contains("MINUTES_TEST_TRANSCRIPTION_UNSET_KEY_1355"));
        assert!(listener.accept().is_err());
    });
}

#[test]
fn http_failure_marks_job_failed_and_preserves_recording_and_both_stems() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        let voice = audio.with_extension("voice.wav");
        let system = audio.with_extension("system.wav");
        let original = std::fs::read(&audio).unwrap();
        std::fs::write(&voice, &original).unwrap();
        std::fs::write(&system, &original).unwrap();
        let (base, handle) = server(
            vec![(503, "provider error content must not reach logs")],
            |_| {},
        );
        config.transcription.openai_compatible_base_url = base;
        let job = crate::jobs::enqueue_capture_job(
            crate::CaptureMode::Meeting,
            None,
            audio.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        crate::jobs::process_pending_jobs(&config, |_| {}).unwrap();
        let failed = crate::jobs::load_job(&job.id).unwrap();
        assert_eq!(failed.state, crate::jobs::JobState::Failed);
        assert_eq!(failed.retry_count, crate::jobs::MAX_AUTO_RETRIES);
        assert!(failed.error.as_deref().unwrap().contains("HTTP 503"));
        assert!(!failed
            .error
            .as_deref()
            .unwrap()
            .contains("provider error content"));
        for path in [&audio, &voice, &system] {
            assert_eq!(std::fs::read(path).unwrap(), original);
        }
        assert_eq!(handle.join().unwrap().len(), 1);
    });
}

#[test]
fn restriction_or_unknown_policy_refuses_without_connecting() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        config.transcription.openai_compatible_base_url =
            format!("http://{}/v1", listener.local_addr().unwrap());
        let policy = audio.with_extension("md");
        let normal = std::fs::read_to_string(&policy).unwrap();
        for sensitivity in ["restricted", "future", "null", "~"] {
            std::fs::write(&policy, normal.replace("normal", sensitivity)).unwrap();
            let error = super::super::transcribe(&audio, &config)
                .unwrap_err()
                .to_string();
            assert!(error.contains("egress policy refused"));
        }
        std::fs::write(
            &policy,
            normal.replace("sensitivity: normal", "capture: none"),
        )
        .unwrap();
        assert!(super::super::transcribe(&audio, &config)
            .unwrap_err()
            .to_string()
            .contains("capture disabled"));
        std::fs::remove_file(&policy).unwrap();
        assert!(super::super::transcribe(&audio, &config).is_err());
        assert!(listener.accept().is_err());
        assert!(audio.exists());
    });
}

#[test]
fn policy_is_rechecked_between_chunks() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 2);
        let (prefix, suffix, _) = multipart_parts(&config).unwrap();
        config.transcription.openai_compatible_max_upload_bytes =
            prefix.len() + suffix.len() + 44 + 32000;
        let path = audio.with_extension("md");
        let (base, handle) = server(vec![(200, OK)], move |_| {
            let content = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, content.replace("normal", "restricted")).unwrap();
        });
        config.transcription.openai_compatible_base_url = base;
        let error = super::super::transcribe(&audio, &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("egress policy changed"));
        assert_eq!(handle.join().unwrap().len(), 1);
    });
}

#[test]
fn auto_never_selects_remote_and_live_stays_local() {
    let mut config = Config::default();
    config.transcription.openai_compatible_base_url = "https://provider.invalid/v1".into();
    for compiled in [false, true] {
        for apple in [false, true] {
            for plugin in [false, true] {
                for model in [false, true] {
                    let resolution = super::super::resolve_auto_transcription_engine(
                        compiled, apple, plugin, model,
                    );
                    assert_ne!(resolution.engine, "openai-compatible");
                }
            }
        }
    }
    assert_ne!(
        super::super::effective_batch_engine(&config),
        "openai-compatible"
    );
    config.transcription.engine = "openai-compatible".into();
    assert_eq!(config.effective_live_transcript_backend(), "whisper");
    assert_eq!(
        super::super::effective_batch_engine(&config),
        "openai-compatible"
    );
}

#[test]
fn remote_health_and_disclosure_name_only_the_host() {
    let mut config = Config::default();
    config.transcription.engine = "openai-compatible".into();
    config.transcription.openai_compatible_base_url = "https://provider.invalid/v1".into();
    let item = crate::health::model_status(&config);
    assert_eq!(item.state, "attention");
    assert!(item.detail.contains("remote"));
    assert!(item.detail.contains("provider.invalid"));
    assert!(config.recording_disclosure().contains("provider.invalid"));
    assert!(!config
        .recording_disclosure()
        .contains("locally on my device"));
    config.transcription.openai_compatible_base_url =
        "https://secret@provider.invalid/v1?key=secret".into();
    assert!(!remote_transcription_notice(&config).contains("secret"));
    assert!(remote_transcription_status(&config).is_err());
}

#[test]
fn redirect_and_invalid_segments_fail_without_retry() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        for (status, response) in [
            (307, OK),
            (
                200,
                r#"{"segments":[{"start":0,"end":99,"text":"invalid"}]}"#,
            ),
            (200, "invalid JSON"),
        ] {
            let (base, handle) = server(vec![(status, response)], |_| {});
            config.transcription.openai_compatible_base_url = base;
            assert!(super::super::transcribe(&audio, &config).is_err());
            assert_eq!(handle.join().unwrap().len(), 1);
            assert!(audio.exists());
        }
    });
}

#[test]
fn bearer_key_is_read_only_from_named_env_and_never_persisted_or_echoed() {
    crate::test_support::with_temp_home(|root| {
        const NAME: &str = "MINUTES_TEST_TRANSCRIPTION_KEY_1355";
        const KEY: &str = "synthetic-test-key-not-a-credential";
        struct RestoreEnv(Option<std::ffi::OsString>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    std::env::set_var(NAME, value);
                } else {
                    std::env::remove_var(NAME);
                }
            }
        }
        let _restore = RestoreEnv(std::env::var_os(NAME));
        std::env::set_var(NAME, KEY);
        let (mut config, audio) = fixture(root, 1);
        config.transcription.openai_compatible_api_key_env = NAME.into();
        let (base, handle) = server(vec![(401, KEY)], |_| {});
        config.transcription.openai_compatible_base_url = base;
        let job = crate::jobs::enqueue_capture_job(
            crate::CaptureMode::Meeting,
            None,
            audio.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        crate::jobs::process_pending_jobs(&config, |_| {}).unwrap();
        let failed = crate::jobs::load_job(&job.id).unwrap();
        assert_eq!(failed.state, crate::jobs::JobState::Failed);
        assert_eq!(failed.retry_count, crate::jobs::MAX_AUTO_RETRIES);
        assert!(failed.error.as_deref().unwrap().contains("HTTP 401"));
        assert!(!serde_json::to_string(&failed).unwrap().contains(KEY));
        assert!(!toml::to_string(&config).unwrap().contains(KEY));
        let requests = handle.join().unwrap();
        assert!(requests[0]
            .0
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {KEY}")));
        assert!(!String::from_utf8_lossy(&requests[0].1).contains(KEY));
    });
}

#[test]
fn crashed_remote_job_is_not_automatically_reuploaded() {
    crate::test_support::with_temp_home(|root| {
        let (mut config, audio) = fixture(root, 1);
        let job = crate::jobs::enqueue_capture_job(
            crate::CaptureMode::Meeting,
            None,
            audio.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        // Simulate a dead upload worker with the budget persisted before POST.
        crate::jobs::update_job_state(&job.id, |job| {
            job.state = crate::jobs::JobState::Transcribing;
            job.owner_pid = Some(99_999_999);
            job.retry_count = crate::jobs::MAX_AUTO_RETRIES;
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        config.transcription.openai_compatible_base_url =
            format!("http://{}/v1", listener.local_addr().unwrap());
        crate::jobs::process_pending_jobs(&config, |_| {}).unwrap();
        assert_eq!(
            crate::jobs::load_job(&job.id).unwrap().state,
            crate::jobs::JobState::Failed
        );
        assert!(listener.accept().is_err());
        assert!(audio.exists());
    });
}

#[test]
fn config_round_trips_opt_in_settings_and_defaults_stay_local() {
    let mut config = Config::default();
    assert_eq!(config.transcription.engine, "auto");
    assert_eq!(
        config.transcription.openai_compatible_max_upload_bytes,
        25_000_000
    );
    config.transcription.engine = "openai-compatible".into();
    config.transcription.openai_compatible_base_url = "https://provider.invalid/v1".into();
    config.transcription.openai_compatible_model = "test-model".into();
    config.transcription.openai_compatible_api_key_env = "TRANSCRIPTION_API_KEY".into();
    config.transcription.openai_compatible_max_upload_bytes = 12_000_000;
    let loaded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(loaded.transcription.engine, "openai-compatible");
    assert_eq!(loaded.transcription.openai_compatible_model, "test-model");
    assert_eq!(
        loaded.transcription.openai_compatible_api_key_env,
        "TRANSCRIPTION_API_KEY"
    );
    assert_eq!(
        loaded.transcription.openai_compatible_max_upload_bytes,
        12_000_000
    );
}
