//! Replay a 16 kHz mono WAV through the production recording sidecar.
//!
//! This is the harness that found the whisper-rs abort-callback regression
//! (#951) and measured the drop behaviour behind #967. It drives
//! `run_sidecar_mpsc`, the exact code path the desktop recorder uses, from a
//! WAV instead of the microphone, under an isolated HOME that reuses the real
//! models and config, with a tracing subscriber so every warning the desktop
//! app would swallow is printed.
//!
//! ```text
//! cargo run --release -p minutes-core --example replay_sidecar \
//!     --features "whisper streaming" -- --wav call.wav --realtime --drafts
//! ```
//!
//! - `--realtime` paces chunks at 100 ms and uses the production
//!   `sync_channel(200)` + `try_send`, so feed drops are visible. Without it
//!   the audio is pushed as fast as the sidecar accepts it.
//! - `--drafts` wires a live-partial publisher like the desktop recorder does,
//!   so the worker also runs current-speech drafts.
//! - `--speed N` scales the real-time pacing (2 = twice real time).
//!
//! A WAV snapshotted mid-recording (unfinalized header) is read as raw
//! `pcm_s16le`. Add `--features metal` to match the shipped macOS build.
#![cfg_attr(
    not(all(feature = "whisper", feature = "streaming")),
    allow(dead_code, unused_imports)
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    if bytes.len() > 44 && &bytes[0..4] == b"RIFF" {
        let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
        if data_len == 0 || data_len > bytes.len() - 44 {
            eprintln!("wav: unfinalized header, reading raw pcm_s16le mono 16 kHz");
            return bytes[44..]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
                .collect();
        }
    }
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "expected a 16 kHz wav");
    let raw: Vec<f32> = reader
        .samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / i16::MAX as f32)
        .collect();
    if spec.channels == 1 {
        raw
    } else {
        raw.chunks(spec.channels as usize)
            .map(|f| f.iter().copied().sum::<f32>() / f.len() as f32)
            .collect()
    }
}

#[cfg(all(feature = "whisper", feature = "streaming"))]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut wav: Option<PathBuf> = None;
    let mut drafts = false;
    let mut realtime = false;
    let mut speed: f64 = 1.0;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--wav" => wav = args.next().map(PathBuf::from),
            "--drafts" => drafts = true,
            "--realtime" => realtime = true,
            "--speed" => speed = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    let wav = wav.expect("usage: replay_sidecar --wav PATH [--realtime] [--drafts] [--speed N]");

    // Isolated HOME: the sidecar writes live-transcript.jsonl, its status file,
    // and minutes.log under ~/.minutes, and must never touch the real ones.
    let real_home = dirs::home_dir().expect("HOME");
    let temp = tempfile::Builder::new()
        .prefix("replay-sidecar-")
        .tempdir()
        .expect("tempdir");
    let th = temp.path().to_path_buf();
    std::fs::create_dir_all(th.join(".minutes")).unwrap();
    std::fs::create_dir_all(th.join(".config")).unwrap();
    std::os::unix::fs::symlink(
        real_home.join(".minutes/models"),
        th.join(".minutes/models"),
    )
    .expect("symlink models");
    std::os::unix::fs::symlink(
        real_home.join(".config/minutes"),
        th.join(".config/minutes"),
    )
    .expect("symlink config");
    std::env::set_var("HOME", &th);
    std::env::remove_var("XDG_CONFIG_HOME");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,whisper_rs=warn,ggml=warn")
            }),
        )
        .with_writer(std::io::stderr)
        .init();
    minutes_core::install_whisper_logging_hooks();

    let config = minutes_core::config::Config::load();
    eprintln!(
        "config: live.model={:?} dictation.model={} partial_max_secs={} vad_engine={} effective_live_backend={}",
        config.live_transcript.model,
        config.dictation.model,
        config.transcription.partial_max_secs,
        config.transcription.vad_engine,
        config.effective_live_transcript_backend(),
    );

    let samples = read_wav(&wav);
    let audio_secs = samples.len() as f64 / 16000.0;
    eprintln!("wav: {audio_secs:.1}s drafts={drafts} realtime={realtime} speed={speed}");

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (publisher, subscriber) = if drafts {
        let (p, s) = minutes_core::live_partials::channel_with_source(
            1,
            minutes_core::live_partials::DEFAULT_PARTIAL_CHANNEL_CAPACITY,
            "recording-sidecar",
        );
        (Some(p), Some(s))
    } else {
        (None, None)
    };

    // Drain drafts like the capture relay would, counting them.
    let draft_events = Arc::new(AtomicU64::new(0));
    let drain_stop = Arc::new(AtomicBool::new(false));
    let drain = subscriber.map(|mut sub| {
        let seen = Arc::clone(&draft_events);
        let stop = Arc::clone(&drain_stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                while sub.try_recv().is_some() {
                    seen.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    });

    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(200);
    let cfg = config.clone();
    let sf = Arc::clone(&stop_flag);
    let sidecar = std::thread::Builder::new()
        .name("live-sidecar".into())
        .spawn(move || minutes_core::live_transcript::run_sidecar_mpsc(rx, sf, &cfg, publisher))
        .unwrap();

    let started = Instant::now();
    let mut sent = 0u64;
    let mut dropped = 0u64;
    let chunk_dur = Duration::from_secs_f64(0.1 / speed);
    for (i, chunk) in samples.chunks(1600).enumerate() {
        if realtime {
            let target = started + chunk_dur.mul_f64(i as f64);
            let now = Instant::now();
            if target > now {
                std::thread::sleep(target - now);
            }
            match tx.try_send(chunk.to_vec()) {
                Ok(()) => sent += 1,
                Err(std::sync::mpsc::TrySendError::Full(_)) => dropped += 1,
                Err(_) => break,
            }
        } else {
            if tx.send(chunk.to_vec()).is_err() {
                break;
            }
            sent += 1;
        }
        if i % 600 == 599 {
            eprintln!(
                "[feeder] audio_t={:.0}s wall={:.0}s sent={sent} dropped={dropped} draft_events={}",
                (i + 1) as f64 / 10.0,
                started.elapsed().as_secs_f64(),
                draft_events.load(Ordering::Relaxed)
            );
        }
    }
    drop(tx);
    sidecar.join().expect("sidecar panicked");
    drain_stop.store(true, Ordering::Relaxed);
    if let Some(d) = drain {
        d.join().ok();
    }
    let wall = started.elapsed().as_secs_f64();

    let content =
        std::fs::read_to_string(th.join(".minutes/live-transcript.jsonl")).unwrap_or_default();
    let lines: Vec<minutes_core::live_transcript::TranscriptLine> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let words: usize = lines
        .iter()
        .map(|l| l.text.split_whitespace().count())
        .sum();
    let covered: f64 = lines.iter().map(|l| l.duration_ms as f64 / 1000.0).sum();
    println!(
        "RESULT drafts={drafts} realtime={realtime} speed={speed} audio_s={audio_secs:.0} wall_s={wall:.0} chunks_sent={sent} chunks_dropped={dropped} draft_events={} final_lines={} final_words={words} covered_audio_s={covered:.0}",
        draft_events.load(Ordering::Relaxed),
        lines.len(),
    );
    for l in &lines {
        println!(
            "LINE {} off={}s dur={:.1}s {}",
            l.line,
            l.offset_ms / 1000,
            l.duration_ms as f64 / 1000.0,
            l.text.chars().take(90).collect::<String>()
        );
    }
    // The sidecar's own summary (live_sidecar_ended) lands in the temp HOME's
    // minutes.log; surface it so drops and whisper failures are in the report.
    if let Ok(log) = std::fs::read_to_string(th.join(".minutes/logs/minutes.log")) {
        for entry in log.lines().filter(|l| l.contains("live_sidecar_ended")) {
            println!("SUMMARY {entry}");
        }
    }
    let _ = stop_flag;
}

#[cfg(not(all(feature = "whisper", feature = "streaming")))]
fn main() {
    eprintln!("build with --features \"whisper streaming\"");
}
