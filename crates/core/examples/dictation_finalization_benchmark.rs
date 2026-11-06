//! Compare ordinary finalization with opt-in exact-audio reuse on a public WAV.
//!
//! This is an unpaced engine replay, not a microphone, hotkey, or insertion test.
//! No configuration, history, network, or additional model downloads are used.

#[cfg(not(all(feature = "whisper", feature = "streaming")))]
fn main() {
    eprintln!("requires --no-default-features --features whisper,streaming");
    std::process::exit(2);
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use minutes_core::streaming_whisper::{StreamingWhisper, StreamingWhisperSession};
    use serde::Serialize;
    use std::path::PathBuf;
    use std::time::Instant;

    #[derive(Serialize)]
    struct Pass {
        feed_us: u128,
        final_us: u128,
        reused: bool,
        text: Option<String>,
    }

    #[derive(Serialize)]
    struct Pair {
        case: &'static str,
        iteration: usize,
        audio_samples: usize,
        partial_max_secs: u32,
        exact_text_match: bool,
        baseline: Pass,
        candidate: Pass,
    }

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err(
            "usage: dictation_finalization_benchmark MODEL.bin PUBLIC.wav [ITERATIONS]".into(),
        );
    }
    let iterations = args
        .get(2)
        .map(|s| {
            s.to_str()
                .ok_or("iterations must be UTF-8")?
                .parse::<usize>()
                .map_err(|_| "invalid iterations")
        })
        .transpose()?
        .unwrap_or(3);
    if !(1..=100).contains(&iterations) {
        return Err("iterations must be between 1 and 100".into());
    }
    let model = PathBuf::from(&args[0]);
    let audio = PathBuf::from(&args[1]);
    let mut reader = hound::WavReader::open(&audio)?;
    let spec = reader.spec();
    if spec.sample_rate != 16000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("fixture must be 16 kHz mono PCM16".into());
    }
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| sample.map(|sample| sample as f32 / 32768.0))
        .collect::<Result<_, _>>()?;
    if samples.len() < 5 * 16000 {
        return Err("fixture must contain at least five seconds".into());
    }
    minutes_core::install_whisper_logging_hooks();
    // Match the production context's explicit compiled-backend selection.
    let params = whisper_rs::WhisperContextParameters {
        use_gpu: cfg!(any(
            feature = "coreml",
            feature = "cuda",
            feature = "hipblas",
            feature = "metal",
            feature = "vulkan"
        )),
        ..Default::default()
    };
    let context = whisper_rs::WhisperContext::new_with_params(model, params)?;
    let run = |samples: &[f32], cap, reuse| {
        let mut session = StreamingWhisperSession::new(
            StreamingWhisper::with_partial_max_secs(Some("en".into()), cap),
            &context,
        )
        .with_final_reuse(reuse);
        let started = Instant::now();
        for chunk in samples.chunks(1600) {
            session.feed(chunk);
        }
        let feed_us = started.elapsed().as_micros();
        let started = Instant::now();
        let text = session.finalize().map(|result| result.text);
        Pass {
            feed_us,
            final_us: started.elapsed().as_micros(),
            reused: session.final_reused(),
            text,
        }
    };
    // Exclude model initialization and one warm recognition from reported pairs.
    let _ = run(&samples[..2 * 16000], 30, false);
    let silence = vec![0.0; 4 * 16000];
    let cases: Vec<(&str, &[f32], u32)> = vec![
        ("aligned_4s", &samples[..4 * 16000], 30),
        ("new_audio_100ms", &samples[..4 * 16000 + 1600], 30),
        ("new_audio_500ms", &samples[..4 * 16000 + 8000], 30),
        ("whole_fixture", &samples, 30),
        ("bounded_suffix", &samples[..4 * 16000], 1),
        ("too_short", &samples[..8000], 30),
        ("silence", &silence, 30),
    ];
    let mut pairs = Vec::new();
    for (case, audio, cap) in cases {
        for iteration in 0..iterations {
            // Alternate order so the candidate does not always run second.
            let (baseline, candidate) = if iteration % 2 == 0 {
                (run(audio, cap, false), run(audio, cap, true))
            } else {
                let candidate = run(audio, cap, true);
                (run(audio, cap, false), candidate)
            };
            pairs.push(Pair {
                case,
                iteration,
                audio_samples: audio.len(),
                partial_max_secs: cap,
                exact_text_match: baseline.text == candidate.text,
                baseline,
                candidate,
            });
        }
    }
    let matched = pairs.iter().all(|pair| pair.exact_text_match);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "measurement": "unpaced 100ms-chunk engine replay; excludes capture, release delivery and insertion",
            "all_text_identical": matched,
            "pairs": pairs,
        }))?
    );
    if !matched {
        std::process::exit(1);
    }
    Ok(())
}
