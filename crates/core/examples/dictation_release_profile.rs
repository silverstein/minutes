//! Paced release/queue component profile on explicit public PCM16 audio.
//! No device, config, history, insertion, provider, or new model is accessed.

#[cfg(not(all(feature = "whisper", feature = "streaming")))]
fn main() {
    eprintln!("requires --no-default-features --features whisper,streaming");
    std::process::exit(2);
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
mod profile {
    use minutes_core::streaming::CHUNK_SAMPLES;
    use minutes_core::streaming_whisper::StreamingWhisper;
    use serde::Serialize;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Serialize)]
    struct Replay {
        audio_samples: usize,
        previews: bool,
        feed_us: u128,
        final_us: u128,
        text: Option<String>,
    }

    fn replay(samples: &[f32], ctx: &whisper_rs::WhisperContext, previews: bool) -> Replay {
        let mut session = StreamingWhisper::new(Some("en".into()));
        let started = Instant::now();
        for chunk in samples.chunks(CHUNK_SAMPLES) {
            if previews {
                session.feed(chunk, ctx);
            } else {
                session.accumulate(chunk);
            }
        }
        let feed_us = started.elapsed().as_micros();
        let started = Instant::now();
        let text = session.finalize(ctx).map(|r| r.text);
        Replay {
            audio_samples: samples.len(),
            previews,
            feed_us,
            final_us: started.elapsed().as_micros(),
            text,
        }
    }

    #[derive(Serialize)]
    struct Paced {
        requested_release_ms: u64,
        previews: bool,
        control_poll_ms: u64,
        release_to_stop_observed_us: u128,
        release_to_final_us: u128,
        final_decode_us: u128,
        feed_total_us: u128,
        feed_max_us: u128,
        released_during_feed: bool,
        max_chunk_age_us: u128,
        queued_at_stop: usize,
        oldest_queued_age_at_stop_us: Option<u128>,
        capture_send_drops: usize,
        samples_consumed: usize,
        samples_captured_before_release: usize,
        same_consumed_audio_reference_match: bool,
        text: Option<String>,
    }

    fn paced(
        audio: &[f32],
        ctx: &whisper_rs::WhisperContext,
        previews: bool,
        release_ms: u64,
    ) -> Paced {
        let epoch = Instant::now();
        let release_deadline = epoch + Duration::from_millis(release_ms);
        let stop = AtomicBool::new(false);
        let finished = AtomicBool::new(false);
        let released_at = Mutex::new(None);
        let captured_before_release = AtomicUsize::new(0);
        let send_drops = AtomicUsize::new(0);
        let (tx, rx) = crossbeam_channel::bounded(64);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut index = 0;
                while !finished.load(Ordering::Relaxed) {
                    let deadline = epoch + Duration::from_millis((index as u64 + 1) * 100);
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    if finished.load(Ordering::Relaxed) {
                        break;
                    }
                    let start = index * CHUNK_SAMPLES;
                    let mut samples = vec![0.0; CHUNK_SAMPLES];
                    if start < audio.len() {
                        let end = (start + CHUNK_SAMPLES).min(audio.len());
                        samples[..end - start].copy_from_slice(&audio[start..end]);
                    }
                    let captured = Instant::now();
                    if !stop.load(Ordering::Relaxed) {
                        captured_before_release.fetch_add(CHUNK_SAMPLES, Ordering::Relaxed);
                    }
                    if tx.try_send((captured, samples)).is_err() {
                        send_drops.fetch_add(1, Ordering::Relaxed);
                    }
                    index += 1;
                }
            });
            scope.spawn(|| {
                std::thread::sleep(release_deadline.saturating_duration_since(Instant::now()));
                *released_at.lock().unwrap() = Some(Instant::now());
                stop.store(true, Ordering::Relaxed);
            });
            let mut session = StreamingWhisper::new(Some("en".into()));
            let mut consumed = Vec::new();
            let mut feed_total_us = 0;
            let mut feed_max_us = 0;
            let mut max_chunk_age_us = 0;
            let mut feed_intervals = Vec::new();
            // Original desktop: previews and 100 ms control polling. Candidate:
            // no previews and the production candidate's 20 ms control polling.
            // Keep one controlled utterance active to isolate preview blocking;
            // this timed experiment intentionally excludes VAD segmentation.
            while !stop.load(Ordering::Relaxed) {
                let Ok((captured, chunk)) =
                    rx.recv_timeout(Duration::from_millis(if previews { 100 } else { 20 }))
                else {
                    continue;
                };
                max_chunk_age_us = max_chunk_age_us.max(captured.elapsed().as_micros());
                consumed.extend_from_slice(&chunk);
                let started = Instant::now();
                if previews {
                    session.feed(&chunk, ctx);
                } else {
                    session.accumulate(&chunk);
                }
                let ended = Instant::now();
                let elapsed = ended.duration_since(started).as_micros();
                feed_total_us += elapsed;
                feed_max_us = feed_max_us.max(elapsed);
                feed_intervals.push((started, ended));
            }
            let observed = Instant::now();
            let queued_at_stop = rx.len();
            // This diagnostic removes one already-abandoned queue entry, never
            // feeds it into recognition or changes the consumed-audio reference.
            let oldest_queued_age_at_stop_us = rx
                .try_recv()
                .ok()
                .map(|(captured, _)| observed.saturating_duration_since(captured).as_micros());
            // Production capture lives until loop teardown. Keep producer alive
            // during final decode too; stopping it here would hide queue growth.
            let started = Instant::now();
            let text = session.finalize(ctx).map(|r| r.text);
            let final_ready = Instant::now();
            finished.store(true, Ordering::Relaxed);
            let released = released_at.lock().unwrap().unwrap();
            // Compare each actual consumed buffer against an independent final
            // decode. Baseline and candidate can consume different audio while
            // release waits on a partial, so their texts are not paired here.
            let reference = replay(&consumed, ctx, false);
            Paced {
                requested_release_ms: release_ms,
                previews,
                control_poll_ms: if previews { 100 } else { 20 },
                release_to_stop_observed_us: observed.duration_since(released).as_micros(),
                release_to_final_us: final_ready.duration_since(released).as_micros(),
                final_decode_us: final_ready.duration_since(started).as_micros(),
                feed_total_us,
                feed_max_us,
                released_during_feed: feed_intervals
                    .iter()
                    .any(|(start, end)| *start <= released && released < *end),
                max_chunk_age_us,
                queued_at_stop,
                oldest_queued_age_at_stop_us,
                capture_send_drops: send_drops.load(Ordering::Relaxed),
                samples_consumed: consumed.len(),
                samples_captured_before_release: captured_before_release.load(Ordering::Relaxed),
                same_consumed_audio_reference_match: text == reference.text,
                text,
            }
        })
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if !(2..=3).contains(&args.len()) {
            return Err("usage: dictation_release_profile MODEL.bin PUBLIC.wav [--fresh-preview|--fresh-final]".into());
        }
        let mut reader = hound::WavReader::open(&args[1])?;
        let spec = reader.spec();
        if spec.sample_rate != 16000 || spec.channels != 1 || spec.bits_per_sample != 16 {
            return Err("fixture must be 16 kHz mono PCM16".into());
        }
        let audio = reader
            .samples::<i16>()
            .map(|s| s.map(|s| f32::from(s) / 32768.0))
            .collect::<Result<Vec<_>, _>>()?;
        if audio.len() < 5 * 16000 {
            return Err("fixture must contain at least five seconds".into());
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
        let load_started = Instant::now();
        let ctx = whisper_rs::WhisperContext::new_with_params(&args[0], params)?;
        let context_load_us = load_started.elapsed().as_micros();
        if let Some(mode) = args.get(2) {
            let previews = match mode.to_str() {
                Some("--fresh-preview") => true,
                Some("--fresh-final") => false,
                _ => return Err("unknown fresh-process mode".into()),
            };
            let result = replay(&audio, &ctx, previews);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "measurement": "fresh-process context, no warmup; unpaced same-input work and final cost; shared OS/backend caches not cleared",
                    "context_load_us": context_load_us,
                    "replay": result,
                }))?
            );
            return Ok(());
        }
        let _ = replay(&audio[..32000], &ctx, false);
        let mut same_audio = Vec::new();
        for end in [2 * 16000, 4 * 16000, audio.len()] {
            let baseline = replay(&audio[..end], &ctx, true);
            let candidate = replay(&audio[..end], &ctx, false);
            same_audio.push(serde_json::json!({
                "text_identical": baseline.text == candidate.text,
                "baseline": baseline, "candidate": candidate,
            }));
        }
        let mut paced_runs = Vec::new();
        for (index, release) in [2050, 2350, 3950, 4050].into_iter().enumerate() {
            for previews in if index % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                eprintln!("release={release}ms previews={previews}");
                paced_runs.push(paced(&audio, &ctx, previews, release));
            }
        }
        let matched = same_audio.iter().all(|p| p["text_identical"] == true)
            && paced_runs
                .iter()
                .all(|p| p.same_consumed_audio_reference_match);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
            "measurement": "paced single-utterance component replay; 100ms input, bounded(64), original previews/100ms control poll versus candidate accumulate/20ms control poll; excludes VAD, native devices, cleanup and insertion",
                "all_same_audio_references_match": matched,
                "same_audio": same_audio, "paced": paced_runs,
            }))?
        );
        if !matched {
            return Err("same-audio text mismatch; preserve raw evidence".into());
        }
        Ok(())
    }
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    profile::run()
}
