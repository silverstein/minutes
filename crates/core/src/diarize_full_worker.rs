//! Explicit post-processing engine. Capture and live ASR never call this worker.
use super::{Config, DiarizationCancellation, DiarizationResult, SpeakerSegment};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use zeroize::Zeroizing;

#[allow(dead_code)] // Shared protocol includes worker-only constants and model verification.
#[path = "diarize_full_protocol.rs"]
mod protocol;

struct AudioInput {
    samples: Zeroizing<Vec<f32>>,
    offset: usize,
}

fn last_worker_marker(stderr: &[u8], prefix: &[u8], allowed: &[&'static str]) -> &'static str {
    stderr
        .split(|byte| *byte == b'\n')
        .rev()
        .find_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let value = line.strip_prefix(prefix)?;
            allowed
                .iter()
                .copied()
                .find(|value_allowed| value == value_allowed.as_bytes())
        })
        .unwrap_or("unreported")
}

fn last_worker_phase(stderr: &[u8]) -> &'static str {
    last_worker_marker(
        stderr,
        b"MINUTES_SPEAKER_PHASE ",
        &[
            "handles_started",
            "handles_closed",
            "awaiting_pcm",
            "pcm_loaded",
            "pipeline_ready",
            "inference_complete",
        ],
    )
}

fn last_worker_failure(stderr: &[u8]) -> &'static str {
    last_worker_marker(
        stderr,
        b"MINUTES_SPEAKER_FAILURE ",
        protocol::FAILURE_CLASSES,
    )
}

impl Read for AudioInput {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = output.len().min(self.samples.len() * 4 - self.offset);
        for (index, byte) in output[..count].iter_mut().enumerate() {
            let position = self.offset + index;
            *byte = self.samples[position / 4].to_le_bytes()[position % 4];
        }
        self.offset += count;
        Ok(count)
    }
}

fn validate_response(
    response: protocol::Response,
    sample_count: usize,
    digest: &str,
) -> Result<DiarizationResult, String> {
    if response.protocol != protocol::PROTOCOL
        || response.engine != protocol::ENGINE
        || response.model_revision != protocol::MODEL_REVISION
        || response.sample_count != sample_count
        || response.audio_sha256 != digest
        || response.segments.len() > protocol::MAX_SEGMENTS
    {
        return Err("full speaker worker response does not match this input/protocol".into());
    }
    let duration = sample_count as f64 / 16_000.0;
    let mut identities = HashSet::new();
    let mut segments = Vec::with_capacity(response.segments.len());
    for segment in response.segments {
        let label = segment
            .speaker
            .strip_prefix("SPEAKER_")
            .and_then(|label| label.parse::<u8>().ok());
        let valid_label = matches!(label, Some(1..=64))
            && segment.speaker == format!("SPEAKER_{}", label.unwrap_or(0));
        let valid_interval = segment.start.is_finite()
            && segment.end.is_finite()
            && 0.0 <= segment.start
            && segment.start < segment.end
            && segment.end <= duration;
        if !valid_label || !valid_interval {
            return Err("full speaker worker returned invalid anonymous activity".into());
        }
        identities.insert(segment.speaker.clone());
        segments.push(SpeakerSegment {
            speaker: segment.speaker,
            start: segment.start,
            end: segment.end,
        });
    }
    segments.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    Ok(DiarizationResult {
        segments,
        num_speakers: identities.len(),
        ..Default::default()
    })
}

pub(super) fn models_installed(config: &Config) -> bool {
    config
        .diarization
        .speaker_worker_path
        .as_ref()
        .is_some_and(|path| path.is_absolute() && path.is_file())
        && protocol::verify_models(&config.diarization.model_path).is_ok()
}

pub(super) fn diarize(
    audio: &Path,
    config: &Config,
    cancellation: &DiarizationCancellation,
) -> Result<DiarizationResult, Box<dyn std::error::Error>> {
    cancellation.check()?;
    // Anonymous/private process-audio capabilities cannot cross an ordinary
    // post-exec pipe boundary. Never weaken their existing in-process policy.
    if crate::pipeline::authorized_audio_stdin(audio)?.is_some() {
        return Err("full speaker worker is unavailable for anonymous private audio".into());
    }
    let executable = config
        .diarization
        .speaker_worker_path
        .as_ref()
        .filter(|path| path.is_absolute())
        .ok_or("set an absolute diarization.speaker_worker_path for this opt-in engine")?;
    let authority = crate::bounded_child::BoundExecutable::bind(executable)?;
    // No download or fallback to the ONNX/CAM++ model directory.
    protocol::verify_models(&config.diarization.model_path)?;
    cancellation.check()?;
    // Admit the declared duration before allocating canonical audio. The
    // existing loader separately bounds actual frames and decoded dimensions.
    let wav = hound::WavReader::open(audio)?;
    let spec = wav.spec();
    if spec.channels == 0
        || spec.sample_rate == 0
        || wav.duration() as u64 > spec.sample_rate as u64 * 3600
    {
        return Err("full speaker worker currently accepts up to one hour of WAV audio".into());
    }
    drop(wav);
    let (samples, _) = super::load_audio(audio, cancellation)?;
    let samples = Zeroizing::new(samples);
    if samples.is_empty() || samples.len() > protocol::MAX_SAMPLES {
        return Err("full speaker worker currently accepts up to one hour of WAV audio".into());
    }
    let sample_count = samples.len();
    let mut hasher = Sha256::new();
    for sample in samples.iter() {
        hasher.update(sample.to_le_bytes());
    }
    let digest = format!("{:x}", hasher.finalize());
    let mut command = crate::bounded_child::BoundedCommand::from_bound_executable(authority)?;
    command
        .arg(&config.diarization.model_path)
        .env_clear()
        .env("MINUTES_FULL_SPEAKER_WORKER_V1", "1")
        .env(
            "MINUTES_FULL_SPEAKER_PARENT_PID",
            std::process::id().to_string(),
        )
        .env("RAYON_NUM_THREADS", "2")
        .env("OMP_NUM_THREADS", "2")
        .env("OPENBLAS_NUM_THREADS", "1")
        .env("VECLIB_MAXIMUM_THREADS", "1")
        // Speakrs already parallelizes independent CPU jobs. Do not multiply
        // that by matrixmultiply's separate, otherwise automatic thread pool.
        .env("MATMUL_NUM_THREADS", "1")
        .single_process()
        .close_extra_descriptors();
    #[cfg(not(target_os = "macos"))]
    command.address_space_limit(protocol::MEMORY_BYTES);
    let run = crate::bounded_child::run_cancellable(
        &mut command,
        Some(Box::new(AudioInput { samples, offset: 0 })),
        crate::bounded_child::StdoutTarget::Capture {
            max_bytes: protocol::OUTPUT_LIMIT,
        },
        crate::bounded_child::ChildBudget {
            wall_clock: cancellation.remaining(),
            stderr_tail: 4096,
        },
        cancellation.cancelled.clone(),
    )?;
    tracing::debug!(
        worker_phase = last_worker_phase(&run.output.stderr),
        worker_failure = last_worker_failure(&run.output.stderr),
        worker_exit_code = ?run.output.status.code(),
        timed_out = run.timed_out,
        "full speaker worker completion"
    );
    cancellation.check()?;
    if run.timed_out || !run.output.status.success() {
        // Do not reflect arbitrary worker stderr/audio/path content into saved meetings.
        return Err(
            "full speaker worker failed or exceeded its deadline; speaker labels unavailable"
                .into(),
        );
    }
    let response = serde_json::from_slice(&run.output.stdout)
        .map_err(|_| "full speaker worker returned malformed JSON")?;
    Ok(validate_response(response, sample_count, &digest)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the explicitly built native helper and public pinned models"]
    fn real_worker_short_silence_preserves_protocol_and_completes() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("minutes_core::diarize::full_worker=debug")
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .try_init();
        let mut config = Config::default();
        config.diarization.engine = "speakrs".into();
        config.diarization.speaker_worker_path = Some(
            std::env::var_os("MINUTES_SPEAKER_SMOKE_WORKER")
                .expect("provide the actual compiled helper")
                .into(),
        );
        config.diarization.model_path = std::env::var_os("MINUTES_SPEAKER_SMOKE_MODELS")
            .expect("provide the public pinned model directory")
            .into();
        let temp = tempfile::tempdir().unwrap();
        let wav = temp.path().join("synthetic-silence.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..48_000 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let before = std::fs::read(&wav).unwrap();
        // A small synthetic protocol smoke, separate from the unchanged
        // complete-recording 900-second deadline and frozen quality gates.
        let cancellation = DiarizationCancellation::new(std::time::Duration::from_secs(60));
        let result = diarize(&wav, &config, &cancellation).expect("native protocol smoke failed");
        assert!(result.segments.is_empty());
        assert_eq!(result.num_speakers, 0);
        assert!(result.speaker_embeddings.is_empty());
        assert_eq!(std::fs::read(&wav).unwrap(), before);
    }

    #[test]
    fn progress_diagnostics_never_reflect_arbitrary_stderr() {
        assert_eq!(
            last_worker_phase(b"MINUTES_SPEAKER_PHASE awaiting_pcm\r\nprivate audio content\nMINUTES_SPEAKER_PHASE pipeline_ready\n"),
            "pipeline_ready"
        );
        for untrusted in [
            b"MINUTES_SPEAKER_PHASE secret-path\n".as_slice(),
            b"prefix MINUTES_SPEAKER_PHASE pipeline_ready\n".as_slice(),
            b"MINUTES_SPEAKER_PHASE pcm_loaded private content\n".as_slice(),
            b"\xff\xfeprivate content".as_slice(),
        ] {
            assert_eq!(last_worker_phase(untrusted), "unreported");
        }
    }

    #[test]
    fn failure_diagnostics_accept_only_exact_closed_classes() {
        assert_eq!(
            last_worker_failure(b"MINUTES_SPEAKER_FAILURE backend\r\nprivate payload"),
            "backend"
        );
        for input in [
            b"MINUTES_SPEAKER_FAILURE private/path\n".as_slice(),
            b"prefix MINUTES_SPEAKER_FAILURE backend\n".as_slice(),
            b"MINUTES_SPEAKER_FAILURE backend extra payload\n".as_slice(),
            b"\xff\xfeprivate content".as_slice(),
        ] {
            assert_eq!(last_worker_failure(input), "unreported");
        }
    }

    fn response() -> protocol::Response {
        protocol::Response {
            protocol: protocol::PROTOCOL,
            engine: protocol::ENGINE.into(),
            model_revision: protocol::MODEL_REVISION.into(),
            sample_count: 32_000,
            audio_sha256: "input".into(),
            segments: vec![protocol::Segment {
                speaker: "SPEAKER_1".into(),
                start: 0.0,
                end: 1.0,
            }],
        }
    }
    #[test]
    fn full_engine_namespace_cannot_reuse_existing_profiles() {
        let mut config = Config::default();
        let legacy = crate::voice::model_version(&config);
        config.diarization.model = "community-1".into();
        let community = crate::voice::model_version(&config);
        config.diarization.engine = "speakrs".into();
        assert_eq!(
            crate::voice::model_version(&config),
            protocol::PROFILE_NAMESPACE
        );
        assert_ne!(crate::voice::model_version(&config), legacy);
        assert_ne!(crate::voice::model_version(&config), community);
        assert!(super::super::extract_speaker_embeddings(&[], &config)
            .unwrap_err()
            .to_string()
            .contains("not yet qualified"));
    }
    #[test]
    fn accepts_anonymous_activity_without_enrollment_evidence() {
        let result = validate_response(response(), 32_000, "input").unwrap();
        assert_eq!(result.num_speakers, 1);
        assert!(result.speaker_embeddings.is_empty());
        assert!(result.speaker_embedding_segments.is_empty());
        assert!(!result.source_aware);
    }
    #[test]
    fn rejects_wrong_input_names_and_invalid_activity() {
        for bad in 0..6 {
            let mut r = response();
            match bad {
                0 => r.audio_sha256 = "different recording".into(),
                1 => r.model_revision = "different model".into(),
                2 => r.segments[0].speaker = "Mat".into(),
                3 => r.segments[0].end = 3.0,
                4 => r.segments[0].start = f64::NAN,
                _ => r.segments[0].speaker = "SPEAKER_01".into(),
            }
            assert!(validate_response(r, 32_000, "input").is_err());
        }
    }
    #[test]
    fn cancellation_retires_a_running_worker() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("ready");
        let token = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let child_token = token.clone();
        let ready = marker.clone();
        let signal = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !ready.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            child_token.store(true, std::sync::atomic::Ordering::Release);
        });
        let mut command =
            crate::bounded_child::BoundedCommand::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "diarize::full_worker::tests::cancel_fixture",
                "--nocapture",
            ])
            .env("MINUTES_SPEAKER_TEST_READY", &marker);
        let error = crate::bounded_child::run_cancellable(
            &mut command,
            None,
            crate::bounded_child::StdoutTarget::Capture { max_bytes: 4096 },
            crate::bounded_child::ChildBudget {
                wall_clock: std::time::Duration::from_secs(15),
                stderr_tail: 4096,
            },
            token,
        )
        .err()
        .expect("cancel must not return successful labels");
        signal.join().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        let pid: u32 = std::fs::read_to_string(marker).unwrap().parse().unwrap();
        assert!(
            !crate::pid::is_process_alive(pid),
            "cancelled worker remained alive"
        );
    }
    #[test]
    fn cancel_fixture() {
        if let Some(path) = std::env::var_os("MINUTES_SPEAKER_TEST_READY") {
            std::fs::write(path, std::process::id().to_string()).unwrap();
            loop {
                std::thread::park();
            }
        }
    }

    #[test]
    fn private_audio_capability_is_refused_before_worker_resolution() {
        let mut audio =
            crate::pipeline::PrivateAudioTempFile::new("minutes-full-worker-test-", ".wav")
                .unwrap();
        crate::transcribe::write_wav_16k_mono_to_writer(
            audio.prepare_for_write().unwrap(),
            &[0.25; 320],
        )
        .unwrap();
        audio.finish_write().unwrap();
        let cancellation = DiarizationCancellation::new(std::time::Duration::from_secs(1));
        let error = diarize(audio.as_path(), &Config::default(), &cancellation).unwrap_err();
        assert!(error
            .to_string()
            .contains("unavailable for anonymous private audio"));
    }

    #[test]
    fn missing_worker_is_explicit_and_no_fallback_occurs() {
        let config = Config::default();
        let cancellation = DiarizationCancellation::new(std::time::Duration::from_secs(1));
        let err = diarize(Path::new("missing.wav"), &config, &cancellation).unwrap_err();
        assert!(err.to_string().contains("speaker_worker_path"));
    }
}
