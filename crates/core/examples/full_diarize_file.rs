//! Offline opt-in worker probe. No ASR, microphone, enrollment or configuration writes.
use minutes_core::{config::Config, diarize, voice};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This explicit offline probe reports only the host's allowlisted progress
    // marker. The production dispatcher never reflects arbitrary worker stderr.
    tracing_subscriber::fmt()
        .with_env_filter("minutes_core::diarize::full_worker=debug")
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|_| "could not initialize offline probe diagnostics")?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: full_diarize_file WORKER MODELS WAV".into());
    }
    let mut config = Config::default();
    config.diarization.engine = "speakrs".into();
    config.diarization.speaker_worker_path = Some(args[0].clone().into());
    config.diarization.model_path = args[1].clone().into();
    let audio = std::path::Path::new(&args[2]);
    let result = match diarize::diarize_with_context(
        audio,
        &config,
        diarize::DiarizationContext {
            purpose: diarize::DiarizationPurpose::Auxiliary,
            transcript_windows: None,
        },
    ) {
        diarize::DiarizationOutcome::Result(result) => result,
        diarize::DiarizationOutcome::NotConfigured { reason } => {
            if reason == diarize::DiarizationUnavailable::WorkerTimedOut {
                // The public dispatcher returns at its deadline; its detached
                // supervisor then has a five-second retirement budget. Keep
                // this offline probe alive briefly so that supervisor can
                // publish its sanitized final phase before process exit.
                // This grants no extra inference time or success outcome.
                std::thread::sleep(std::time::Duration::from_secs(6));
            }
            return Err(reason.detail().into());
        }
        diarize::DiarizationOutcome::Skipped { .. } => {
            return Err("source-aware capture was degraded".into())
        }
    };
    assert!(result.speaker_embeddings.is_empty());
    let segments: Vec<_> = result
        .segments
        .iter()
        .map(|s| serde_json::json!({"speaker":s.speaker,"start":s.start,"end":s.end}))
        .collect();
    println!(
        "{}",
        serde_json::json!({"segments":segments,"num_speakers":result.num_speakers,"profile_namespace":voice::model_version(&config)})
    );
    Ok(())
}
