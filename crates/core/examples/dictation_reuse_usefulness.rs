//! Public-WAV reuse sweep using production chunking, VAD and Whisper sessions.
//! This is component replay, not `dictation::run`, paced capture or native latency.

#[cfg(not(all(feature = "whisper", feature = "streaming")))]
fn main() {
    eprintln!("requires --no-default-features --features whisper,streaming");
    std::process::exit(2);
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
mod benchmark {
    use minutes_core::streaming::{ChunkAccumulator, CHUNK_SAMPLES};
    use minutes_core::streaming_whisper::{StreamingWhisper, StreamingWhisperSession};
    use minutes_core::vad::Vad;
    use serde::Serialize;
    use std::time::Instant;

    #[derive(Serialize)]
    struct Utterance {
        chunks: Vec<Vec<f32>>,
        final_at_ms: usize,
        reason: &'static str,
    }

    #[derive(Serialize)]
    struct Segment {
        samples: usize,
        final_at_ms: usize,
        reason: &'static str,
        alignment_eligible: bool,
        feed_us: u128,
        final_us: u128,
        reused: bool,
        text: Option<String>,
    }

    #[derive(Serialize)]
    struct Pair {
        cutoff_ms: usize,
        stop_mode: &'static str,
        iteration: usize,
        all_text_identical: bool,
        baseline: Vec<Segment>,
        candidate: Vec<Segment>,
    }

    // Match dictation::run: feed every VAD-speaking chunk (including hangover),
    // finalize at the first non-speaking chunk, reset, and flush on explicit stop.
    // Device/PID/recovery/insertion paths are deliberately outside this replay.
    fn segment(samples: &[f32], cutoff: usize, silence_end: bool) -> Vec<Utterance> {
        let mut audio = samples[..cutoff].to_vec();
        if silence_end {
            audio.resize(audio.len() + 16000, 0.0);
        }
        let mut accumulator = ChunkAccumulator::new();
        let mut vad = Vad::new();
        let mut pending = Vec::new();
        let mut utterances = Vec::new();
        let mut end_ms = 0;
        // Vary driver callback size from the consumer's fixed 100 ms frames.
        for callback in audio.chunks(171) {
            accumulator.push(callback, |index, chunk| {
                end_ms = (index as usize + 1) * 100;
                let energy = chunk.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>();
                let rms = (energy / chunk.len() as f64).sqrt() as f32;
                if vad.process(rms).speaking {
                    pending.push(chunk);
                } else if !pending.is_empty() {
                    utterances.push(Utterance {
                        chunks: std::mem::take(&mut pending),
                        final_at_ms: end_ms,
                        reason: "vad_boundary",
                    });
                }
            });
        }
        if !pending.is_empty() {
            utterances.push(Utterance {
                chunks: pending,
                final_at_ms: end_ms,
                reason: "explicit_stop",
            });
        }
        utterances
    }

    fn recognize(
        utterances: &[Utterance],
        context: &whisper_rs::WhisperContext,
        reuse: bool,
    ) -> Vec<Segment> {
        utterances
            .iter()
            .map(|utterance| {
                let mut session = StreamingWhisperSession::new(
                    StreamingWhisper::with_partial_max_secs(Some("en".into()), 30),
                    context,
                )
                .with_final_reuse(reuse);
                let started = Instant::now();
                for chunk in &utterance.chunks {
                    session.feed(chunk);
                }
                let feed_us = started.elapsed().as_micros();
                let started = Instant::now();
                let text = session.finalize().map(|r| r.text);
                let final_us = started.elapsed().as_micros();
                let samples = utterance.chunks.len() * CHUNK_SAMPLES;
                Segment {
                    samples,
                    final_at_ms: utterance.final_at_ms,
                    reason: utterance.reason,
                    // Upper bound: a matching decode may still be absent after
                    // empty/error/duplicate preview suppression. Not a cache hit.
                    alignment_eligible: (32000..=30 * 16000).contains(&samples)
                        && samples.is_multiple_of(32000),
                    feed_us,
                    final_us,
                    reused: session.final_reused(),
                    text,
                }
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn vad_hangover_can_invalidate_a_release_aligned_partial() {
            let speech = vec![0.1; 4 * 16000];
            let released = segment(&speech, speech.len(), false);
            assert_eq!(released.len(), 1);
            assert_eq!(released[0].chunks.len(), 40);
            assert_eq!(released[0].reason, "explicit_stop");
            let ended = segment(&speech, speech.len(), true);
            assert_eq!(ended.len(), 1);
            assert_eq!(ended[0].chunks.len(), 45);
            assert_eq!(ended[0].final_at_ms, 4600);
            assert_eq!(ended[0].reason, "vad_boundary");
        }

        #[test]
        fn subchunk_release_does_not_fabricate_an_extra_capture_frame() {
            let speech = vec![0.1; 4 * 16000 + 800];
            let released = segment(&speech, speech.len(), false);
            assert_eq!(released[0].chunks.len(), 40);
            assert_eq!(released[0].final_at_ms, 4000);
        }

        #[test]
        fn internal_pause_finalizes_before_the_next_utterance() {
            let mut audio = vec![0.1; 2 * 16000];
            audio.resize(3 * 16000, 0.0);
            audio.extend(vec![0.1; 2 * 16000]);
            let segments = segment(&audio, audio.len(), true);
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0].chunks.len(), 25);
            assert_eq!(segments[1].chunks.len(), 25);
            assert!(segments.iter().all(|s| s.reason == "vad_boundary"));
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if !(2..=3).contains(&args.len()) {
            return Err(
                "usage: dictation_reuse_usefulness MODEL.bin PUBLIC.wav [ITERATIONS]".into(),
            );
        }
        let iterations: usize = args
            .get(2)
            .map(|s| {
                s.to_str()
                    .ok_or("invalid iterations")?
                    .parse::<usize>()
                    .map_err(|_| "invalid iterations")
            })
            .transpose()?
            .unwrap_or(1);
        if !(1..=3).contains(&iterations) {
            return Err("iterations must be 1..=3 (bounded experiment)".into());
        }
        let mut reader = hound::WavReader::open(&args[1])?;
        let spec = reader.spec();
        if spec.sample_rate != 16000
            || spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
        {
            return Err("fixture must be 16 kHz mono PCM16".into());
        }
        let samples = reader
            .samples::<i16>()
            .map(|s| s.map(|s| f32::from(s) / 32768.0))
            .collect::<Result<Vec<_>, _>>()?;
        if samples.len() < 4 * 16000 {
            return Err("fixture must contain at least four seconds".into());
        }
        minutes_core::install_whisper_logging_hooks();
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
        let context = whisper_rs::WhisperContext::new_with_params(&args[0], params)?;
        let _ = recognize(&segment(&samples, 32000, false), &context, false);
        let start_chunk = if samples.len() >= 6 * 16000 { 40 } else { 20 };
        let mut cutoffs: Vec<usize> = (start_chunk..start_chunk + 20)
            .map(|n| n * CHUNK_SAMPLES)
            .collect();
        cutoffs.push(samples.len());
        let mut pairs = Vec::new();
        for (case_index, cutoff) in cutoffs.into_iter().enumerate() {
            for silence_end in [false, true] {
                let utterances = segment(&samples, cutoff, silence_end);
                for iteration in 0..iterations {
                    eprintln!(
                        "cutoff={}ms silence_end={silence_end} iteration={iteration}",
                        cutoff / 16
                    );
                    let (baseline, candidate) = if (case_index + iteration) % 2 == 0 {
                        (
                            recognize(&utterances, &context, false),
                            recognize(&utterances, &context, true),
                        )
                    } else {
                        let candidate = recognize(&utterances, &context, true);
                        (recognize(&utterances, &context, false), candidate)
                    };
                    let matched = baseline.len() == candidate.len()
                        && baseline
                            .iter()
                            .zip(&candidate)
                            .all(|(a, b)| a.text == b.text);
                    pairs.push(Pair {
                        cutoff_ms: cutoff / 16,
                        stop_mode: if silence_end { "vad_end" } else { "release" },
                        iteration,
                        all_text_identical: matched,
                        baseline,
                        candidate,
                    });
                }
            }
        }
        let matched = pairs.iter().all(|p| p.all_text_identical);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "measurement": "unpaced component replay: production ChunkAccumulator, energy Vad, Whisper session; source-matched dictation routing, not dictation::run or native capture",
                "input": args[1],
                "all_text_identical": matched,
                "pairs": pairs,
            }))?
        );
        if !matched {
            return Err("baseline/candidate text mismatch; preserve raw evidence".into());
        }
        Ok(())
    }
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    benchmark::run()
}
