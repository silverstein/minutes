//! File-only smoke test for the tracked native-stem sidecar, without a microphone.
//! Requires explicit isolated MINUTES_DATA_DIR and MINUTES_CONFIG_PATH. Use public
//! 16 kHz mono i16 WAV input; the harness appends silence to finalize speech.
//! Run with --no-default-features --features streaming,whisper [--features metal].
//! Args: INPUT.wav [--coverage]. Output contains counts and a text hash, not text.
#[cfg(all(feature = "streaming", feature = "whisper"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::sync::{atomic::AtomicBool, Arc};
    let data = PathBuf::from(
        std::env::var_os("MINUTES_DATA_DIR")
            .ok_or("explicit isolated MINUTES_DATA_DIR required")?,
    );
    std::fs::create_dir_all(&data)?;
    let data = data.canonicalize()?;
    let normal = dirs::home_dir().map(|h| h.join(".minutes"));
    if normal.and_then(|p| p.canonicalize().ok()).as_deref() == Some(data.as_path()) {
        return Err("refusing the normal data directory (including aliases)".into());
    }
    let config_path = PathBuf::from(
        std::env::var_os("MINUTES_CONFIG_PATH").ok_or("explicit MINUTES_CONFIG_PATH required")?,
    );
    let mut config = minutes_core::config::Config::load_strict_from(&config_path)?;
    config.output_dir = data.join("output");
    config.live_transcript.coverage_shadow = std::env::args().any(|a| a == "--coverage");
    config.live_transcript.recording_sidecar = true;
    config.ensure_dirs()?;
    let source = PathBuf::from(std::env::args().nth(1).ok_or("expected input WAV")?);
    let mut reader = hound::WavReader::open(&source)?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("expected 16 kHz mono i16 WAV".into());
    }
    if reader.duration() > 16_000 * 60 {
        return Err("smoke fixture must be at most one minute".into());
    }
    let input = data.join("replay.voice.wav");
    let mut writer = hound::WavWriter::create(&input, spec)?;
    for sample in reader.samples::<i16>() {
        writer.write_sample(sample?)?;
    }
    for _ in 0..32_000 {
        writer.write_sample(0_i16)?;
    }
    writer.finalize()?;
    let stop = Arc::new(AtomicBool::new(false));
    let handle = minutes_core::stem_tail::spawn_live_transcription_from_stems(
        input,
        None,
        &config,
        Arc::clone(&stop),
        None,
    )
    .ok_or("sidecar did not start")?;
    // Small fixtures finish on the in-process worker before the deliberately
    // bounded Stop. This is neither capture-drain nor realtime latency evidence.
    std::thread::sleep(std::time::Duration::from_secs(15));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    handle.join().map_err(|_| "sidecar panicked")?;
    let text = std::fs::read_to_string(data.join("live-transcript.jsonl"))?;
    let lines = text
        .lines()
        .map(serde_json::from_str::<minutes_core::live_transcript::TranscriptLine>)
        .collect::<Result<Vec<_>, _>>()?;
    if lines.is_empty() {
        return Err("no finalized speech; inspect the isolated profile logs".into());
    }
    let speech = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut result = serde_json::json!({"lines":lines.len(),"words":speech.split_whitespace().count(),"text_sha256":format!("{:x}",Sha256::digest(speech.as_bytes())),"coverage_enabled":config.live_transcript.coverage_shadow});
    if config.live_transcript.coverage_shadow {
        let text = std::fs::read_to_string(data.join("live-transcript.coverage.jsonl"))?;
        let rows = text
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        result["finalized_spans"] = rows
            .iter()
            .filter(|r| r["event"] == "finalized")
            .count()
            .into();
        result["complete"] = rows.last().unwrap()["complete"].clone();
        result["raw_text_in_coverage"] = text.contains(&speech).into();
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(not(all(feature = "streaming", feature = "whisper")))]
fn main() {
    eprintln!("requires streaming and whisper");
    std::process::exit(2);
}
