//! Replay public/synthetic 16 kHz mono i16 stems through the desktop's actual
//! checkpoint worker, queue the real job, and compare against a whole-file pass.
//! Requires fresh, isolated MINUTES_DATA_DIR and MINUTES_CONFIG_PATH. Args:
//! VOICE.wav SYSTEM.wav [speed, default 1]. Use >120 s inputs, at most 10 min.
//! Example build: --release --no-default-features --features whisper,metal.
#[cfg(feature = "whisper")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use minutes_core::{capture_checkpoint::CaptureCheckpoint, config::Config, jobs, pipeline};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();
    let data = PathBuf::from(
        std::env::var_os("MINUTES_DATA_DIR").ok_or("explicit MINUTES_DATA_DIR required")?,
    );
    std::fs::create_dir_all(&data)?;
    let data = data.canonicalize()?;
    if dirs::home_dir()
        .and_then(|h| h.join(".minutes").canonicalize().ok())
        .as_deref()
        == Some(data.as_path())
    {
        return Err("refusing the normal data directory".into());
    }
    if data.join("jobs").exists() {
        return Err("use a fresh isolated data directory".into());
    }
    let config_path = PathBuf::from(
        std::env::var_os("MINUTES_CONFIG_PATH").ok_or("explicit MINUTES_CONFIG_PATH required")?,
    );
    if !config_path.canonicalize()?.starts_with(&data) {
        return Err("config must be in the isolated data directory".into());
    }
    let mut config = Config::load_strict_from(&config_path)?;
    config.output_dir = data.join("output");
    config.calendar.enabled = false;
    config.summarization.engine = "none".into();
    config.diarization.engine = "none".into();
    config.knowledge.enabled = false;
    config.transcription.engine = "whisper".into();
    config.recording.transcript_checkpoints = true;
    config.ensure_dirs()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err("expected VOICE.wav SYSTEM.wav [speed]".into());
    }
    let speed: f64 = args.get(2).map_or(Ok(1.0), |s| s.parse())?;
    if !(0.5..=8.0).contains(&speed) {
        return Err("speed must be 0.5 to 8".into());
    }
    let mut audio = Vec::new();
    for path in &args[..2] {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.sample_rate != 16_000
            || spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
        {
            return Err("expected 16 kHz mono i16 WAV".into());
        }
        if reader.duration() <= 120 * 16_000 || reader.duration() > 600 * 16_000 {
            return Err("fixtures must be >120 s and <=10 min".into());
        }
        audio.push((
            spec,
            reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?,
        ));
    }
    if audio[0].1.len() != audio[1].1.len() {
        return Err("stems must have equal duration".into());
    }
    let title = "Checkpoint replay";
    let context = "Synthetic public speech fixture for native call checkpoint verification.";
    let anchor = data.join("replay.mov");
    let baseline = data.join("baseline.mov");
    std::fs::write(&anchor, b"minutes native-call stem anchor")?;
    std::fs::write(&baseline, b"minutes native-call stem anchor")?;
    for (input, role) in args[..2].iter().zip(["voice", "system"]) {
        std::fs::copy(input, baseline.with_extension(format!("{role}.wav")))?;
    }
    let whole = pipeline::transcribe_to_artifact(
        &baseline,
        minutes_core::markdown::ContentType::Meeting,
        Some(title),
        &config,
        &pipeline::BackgroundPipelineContext {
            pre_context: Some(context.into()),
            requested_title: Some(title.into()),
            ..Default::default()
        },
        None,
    )?;
    let mut writers = audio
        .iter()
        .zip(["voice", "system"])
        .map(|((spec, _), role)| {
            hound::WavWriter::create(anchor.with_extension(format!("{role}.wav")), *spec)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for writer in &mut writers {
        writer.flush()?;
    }
    let worker = CaptureCheckpoint::start(&anchor, &config, Some(title), Some(context))
        .ok_or("checkpoint worker did not start")?;
    let start = Instant::now();
    let step = 1_600;
    for offset in (0..audio[0].1.len()).step_by(step) {
        let end = (offset + step).min(audio[0].1.len());
        for (writer, (_, samples)) in writers.iter_mut().zip(&audio) {
            for &sample in &samples[offset..end] {
                writer.write_sample(sample)?;
            }
            writer.flush()?;
        }
        let target = Duration::from_secs_f64(end as f64 / 16_000.0 / speed);
        if let Some(wait) = target.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    let stop = Instant::now();
    let checkpoint = worker.finish();
    let snapshot_ms = stop.elapsed().as_millis();
    for writer in writers {
        writer.finalize()?;
    }
    if checkpoint.spans.is_empty() {
        return Err("no spans completed during replay; use slower replay or a faster model".into());
    }
    let job = jobs::queue_live_capture_with_transcript_checkpoint(
        minutes_core::CaptureMode::Meeting,
        Some(title.into()),
        &anchor,
        None,
        Some(context.into()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&checkpoint),
    )?;
    let saved: minutes_core::transcript_checkpoint::TranscriptCheckpoint =
        serde_json::from_slice(&std::fs::read(jobs::job_checkpoint_path(&job.id))?)?;
    assert_eq!(saved, checkpoint);
    let queued_start = Instant::now();
    jobs::process_pending_jobs(&config, |_| {})?;
    let job_ms = queued_start.elapsed().as_millis();
    let finished = jobs::load_job(&job.id).ok_or("queued job disappeared")?;
    if finished.state != jobs::JobState::Complete {
        return Err(format!("job ended {:?}: {:?}", finished.state, finished.error).into());
    }
    let markdown = std::fs::read_to_string(finished.output_path.ok_or("no output")?)?;
    let actual = markdown
        .split_once("## Transcript\n\n")
        .ok_or("missing transcript")?
        .1;
    let words = |text: &str| {
        text.lines()
            .filter_map(|l| l.split_once(']').map(|(_, t)| t))
            .flat_map(str::split_whitespace)
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    let matches = words(actual) == words(&whole.transcript);
    println!(
        "{}",
        serde_json::json!({"spans":checkpoint.spans.len(), "snapshot_ms":snapshot_ms,
        "queued_job_ms":job_ms, "whole_file_words":words(&whole.transcript).len(), "checkpoint_words":words(actual).len(),
        "words_match":matches, "checkpoint_removed":!jobs::job_checkpoint_path(&job.id).exists(), "speed":speed})
    );
    if !matches {
        return Err("word comparison differs; inspect isolated artifacts".into());
    }
    Ok(())
}
#[cfg(not(feature = "whisper"))]
fn main() {
    eprintln!("requires whisper");
    std::process::exit(2);
}
