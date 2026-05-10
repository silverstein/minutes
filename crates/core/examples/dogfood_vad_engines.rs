// ──────────────────────────────────────────────────────────────
// Dogfood harness: pump real meeting WAVs through the production
// streaming-sidecar pipeline twice (once per VAD engine) and report
// the empirical diff between the two JSONL transcripts.
//
//   cargo run --release -p minutes-core --example dogfood_vad_engines
//     --features "whisper streaming vad-ort"
//     [-- --wav PATH] [--count N] [--keep]
//
// Input:
//   --wav PATH       single WAV (defaults to scanning ~/meetings/)
//   --count N        when scanning, take N most-recent WAVs (default 5)
//   --keep           retain the temp HOME dir on exit (default: cleanup)
//
// Output:
//   - Per-engine JSONL files inside an isolated tempdir
//   - Stdout report: counts, durations, exact/near/different buckets,
//     same-engine variance baseline, all-silence flag
//
// Why this exists: PLAN-vad-refactor.md commit 3 flips the default
// from "whisper-silero" to "ort-silero". The flip needs empirical
// evidence beyond the unit tests and parity fixtures — actual runs
// of the production sidecar on real meeting audio. Each engine
// produces its own JSONL via the SAME code path that runs during
// live recording (run_sidecar_mpsc), just fed from a WAV reader
// instead of cpal.
//
// Isolation strategy (codex plan-review #1): override $HOME to a
// tempdir before any minutes-core code runs, then symlink the real
// ~/.minutes/models/ into the temp HOME. Multiple subsystems
// (pid.rs, voice.rs, graph.rs, overlays.rs) resolve paths via
// `home_dir()` which honors $HOME, so the redirect is process-wide
// and consistent. The harness never touches the real ~/.minutes/
// recording-sidecar artifacts.
//
// Determinism note (codex plan-review #3): StreamingWhisper uses
// Greedy(best_of=1) with temperature_inc=0.0 (streaming_whisper.rs:181),
// not the batch path's best_of=5. Run-to-run variance comes from
// greedy decoding non-determinism, not sampling. The same-engine
// variance baseline below establishes that floor so cross-engine
// signal is interpretable above the noise.
// ──────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Parse args ─────────────────────────────────────────────
    let mut args = std::env::args().skip(1);
    let mut single_wav: Option<PathBuf> = None;
    let mut count: usize = 5;
    let mut keep_tempdir = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--wav" => single_wav = args.next().map(PathBuf::from),
            "--count" => count = args.next().and_then(|s| s.parse().ok()).unwrap_or(5),
            "--keep" => keep_tempdir = true,
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("unknown arg: {}", other);
                print_help();
                std::process::exit(2);
            }
        }
    }

    // ── Pick WAVs ──────────────────────────────────────────────
    let wavs = if let Some(p) = single_wav {
        vec![p]
    } else {
        find_recent_meeting_wavs(count)?
    };
    if wavs.is_empty() {
        eprintln!("no WAVs to dogfood; pass --wav or put WAVs in ~/meetings/");
        std::process::exit(2);
    }

    // ── Set up isolated HOME ───────────────────────────────────
    let temp_home = tempfile::tempdir()?;
    let temp_models = temp_home.path().join(".minutes/models");
    std::fs::create_dir_all(&temp_models)?;
    let real_home = dirs::home_dir().expect("real HOME must resolve");
    let real_models = real_home.join(".minutes/models");
    if !real_models.exists() {
        // Tempdir already created above; use Err so its Drop runs.
        return Err(format!(
            "real models dir missing at {} — run `minutes setup` first",
            real_models.display()
        )
        .into());
    }
    for entry in std::fs::read_dir(&real_models)? {
        let entry = entry?;
        let dest = temp_models.join(entry.file_name());
        symlink_path(&entry.path(), &dest)?;
    }
    // Critical: set HOME BEFORE any minutes-core call resolves a
    // path. Setting it later means subsystems that captured HOME
    // earlier see the real one and the redirect is partial.
    // SAFETY: this is the entry point of an example binary; no
    // other thread is reading env at this point.
    unsafe {
        std::env::set_var("HOME", temp_home.path());
    }

    // ── Verify required models are reachable via the new HOME ──
    // Fail with returned errors, not std::process::exit, so the
    // tempdir Drop runs on the way out. Otherwise every misconfig
    // leaks a temp HOME under /tmp.
    let onnx_link = temp_models.join("silero-vad-v6.2.0.onnx");
    let ggml_silero_link = temp_models.join("ggml-silero-v6.2.0.bin");
    let ggml_whisper_link = temp_models.join("ggml-small.bin");
    for (label, path) in [
        ("silero ONNX", &onnx_link),
        ("silero ggml", &ggml_silero_link),
        ("whisper ggml-small", &ggml_whisper_link),
    ] {
        if !path.exists() {
            return Err(format!(
                "required model missing under temp HOME: {label} ({path:?}); \
                 real symlink target is {real_models:?}; run `minutes setup` if absent"
            )
            .into());
        }
    }

    println!("DOGFOOD — VAD engines comparison");
    println!("================================");
    println!("temp HOME:      {}", temp_home.path().display());
    println!("real models:    {}", real_models.display());
    println!("wavs:           {}", wavs.len());
    for (i, w) in wavs.iter().enumerate() {
        println!("  [{}] {}", i, w.display());
    }
    println!();

    // ── Run each WAV through both engines + a same-engine baseline ─
    let jsonl_dir = temp_home.path().join("dogfood-jsonl");
    std::fs::create_dir_all(&jsonl_dir)?;
    let live_jsonl = minutes_core::pid::live_transcript_jsonl_path();
    let live_pid = minutes_core::pid::live_transcript_pid_path();

    if live_pid.exists() {
        return Err(format!(
            "stale live-transcript PID at {} — likely a previous failed run; aborting",
            live_pid.display()
        )
        .into());
    }

    let mut per_wav_reports: Vec<WavReport> = Vec::new();
    for (i, wav_path) in wavs.iter().enumerate() {
        let label = wav_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wav");
        println!("[{}/{}] {}", i + 1, wavs.len(), wav_path.display());
        let samples_for_run = read_wav_to_16k_mono(wav_path);
        if samples_for_run.is_empty() {
            println!("  (empty wav, skipping)");
            continue;
        }

        // Three runs: ort-silero, whisper-silero, and a second
        // whisper-silero pass for the variance baseline. The order
        // matters only for output naming, not for engine state
        // (each run constructs a fresh sidecar).
        let dst_ort = jsonl_dir.join(format!("{}__ort-silero.jsonl", label));
        let dst_whisper_a = jsonl_dir.join(format!("{}__whisper-silero-a.jsonl", label));
        let dst_whisper_b = jsonl_dir.join(format!("{}__whisper-silero-b.jsonl", label));

        let ort = run_one_engine("ort-silero", &samples_for_run, &live_jsonl, &dst_ort)?;
        let whisper_a = run_one_engine(
            "whisper-silero",
            &samples_for_run,
            &live_jsonl,
            &dst_whisper_a,
        )?;
        let whisper_b = run_one_engine(
            "whisper-silero",
            &samples_for_run,
            &live_jsonl,
            &dst_whisper_b,
        )?;

        let lines_ort = read_jsonl(&dst_ort);
        let lines_whisper_a = read_jsonl(&dst_whisper_a);
        let lines_whisper_b = read_jsonl(&dst_whisper_b);

        let report = WavReport {
            wav: wav_path.clone(),
            ort,
            whisper_a,
            whisper_b,
            ort_lines: lines_ort.len(),
            whisper_a_lines: lines_whisper_a.len(),
            whisper_b_lines: lines_whisper_b.len(),
            ort_words: total_words(&lines_ort),
            whisper_a_words: total_words(&lines_whisper_a),
            whisper_b_words: total_words(&lines_whisper_b),
            cross_diff: diff_buckets(&lines_whisper_a, &lines_ort),
            same_diff: diff_buckets(&lines_whisper_a, &lines_whisper_b),
            cross_set: set_alignment_buckets(&lines_whisper_a, &lines_ort),
            same_set: set_alignment_buckets(&lines_whisper_a, &lines_whisper_b),
            jsonl_paths: [dst_ort, dst_whisper_a, dst_whisper_b],
        };
        report.print();
        per_wav_reports.push(report);
    }

    // ── Aggregate ─────────────────────────────────────────────
    println!();
    println!("AGGREGATE ACROSS {} WAVS", per_wav_reports.len());
    println!("==========================");
    let agg_cross_idx = aggregate_buckets(per_wav_reports.iter().map(|r| &r.cross_diff));
    let agg_same_idx = aggregate_buckets(per_wav_reports.iter().map(|r| &r.same_diff));
    let agg_cross_set = aggregate_buckets(per_wav_reports.iter().map(|r| &r.cross_set));
    let agg_same_set = aggregate_buckets(per_wav_reports.iter().map(|r| &r.same_set));
    println!(
        "INDEX-ALIGNED  cross: identical={} near={} short_changed={} different={} (n={})",
        agg_cross_idx.identical,
        agg_cross_idx.near,
        agg_cross_idx.short_changed,
        agg_cross_idx.different,
        agg_cross_idx.total
    );
    println!(
        "INDEX-ALIGNED  same:  identical={} near={} short_changed={} different={} (n={})",
        agg_same_idx.identical,
        agg_same_idx.near,
        agg_same_idx.short_changed,
        agg_same_idx.different,
        agg_same_idx.total
    );
    println!(
        "SET-ALIGNED    cross: identical={} near={} short_changed={} different={} (n={})",
        agg_cross_set.identical,
        agg_cross_set.near,
        agg_cross_set.short_changed,
        agg_cross_set.different,
        agg_cross_set.total
    );
    println!(
        "SET-ALIGNED    same:  identical={} near={} short_changed={} different={} (n={})",
        agg_same_set.identical,
        agg_same_set.near,
        agg_same_set.short_changed,
        agg_same_set.different,
        agg_same_set.total
    );
    println!();
    println!(
        "Δdifferent (cross - same): index-aligned {:+}, set-aligned {:+}",
        agg_cross_idx.different as i64 - agg_same_idx.different as i64,
        agg_cross_set.different as i64 - agg_same_set.different as i64
    );
    println!();
    println!("Index-aligned compares (i, i) pairs and cascades on segment merge/split.");
    println!("Set-aligned matches each utterance to any utterance in the other run; robust");
    println!("to splits/merges. Same-engine baseline is the greedy-decoding noise floor;");
    println!("cross-engine \"different\" close to or below same-engine \"different\" means");
    println!("the engines disagree no more than greedy decoding disagrees with itself.");

    // ── Cleanup ───────────────────────────────────────────────
    if keep_tempdir {
        let kept = temp_home.keep();
        println!();
        println!("kept tempdir at {}", kept.display());
        println!(
            "[privacy] retained tempdir contains transcripts of every WAV processed AND \
             symlinks to your real ~/.minutes/models/. Delete manually when done: rm -rf {}",
            kept.display()
        );
    } else {
        // Default: drop the TempDir, which cleans up on scope exit.
        // Print a hint in case someone wanted to inspect.
        println!();
        println!("(temp HOME cleaned; pass --keep to retain JSONLs for inspection)");
    }
    Ok(())
}

#[cfg(not(all(feature = "whisper", feature = "streaming", feature = "vad-ort")))]
fn main() {
    eprintln!(
        "this example requires --features \"whisper streaming vad-ort\". \
         Re-run with: cargo run --release -p minutes-core --example dogfood_vad_engines \
         --features \"whisper streaming vad-ort\""
    );
    std::process::exit(2);
}

/// Cross-platform symlink for the model-redirect step. The example
/// is a developer tool, but keeping it portable means anyone testing
/// the VAD pipeline on Windows isn't blocked.
#[cfg(unix)]
fn symlink_path(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn symlink_path(target: &Path, link: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}
#[cfg(not(any(unix, windows)))]
fn symlink_path(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink not supported on this platform",
    ))
}

fn print_help() {
    println!(
        "usage: dogfood_vad_engines [--wav PATH] [--count N] [--keep]\n\
         \n\
         compares the streaming sidecar's whisper-silero and ort-silero\n\
         VAD engines on real meeting audio. Defaults to the 5 most recent\n\
         WAVs in ~/meetings/. Output is a structured stdout report; raw\n\
         JSONLs are kept in the tempdir only with --keep."
    );
}

fn find_recent_meeting_wavs(count: usize) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let dir = dirs::home_dir().expect("HOME").join("meetings");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "wav"))
        .filter_map(|e| {
            let m = e.metadata().ok()?.modified().ok()?;
            Some((m, e.path()))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().take(count).map(|(_, p)| p).collect())
}

fn read_wav_to_16k_mono(path: &Path) -> Vec<f32> {
    let mut reader = match hound::WavReader::open(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to open {}: {}", path.display(), e);
            return vec![];
        }
    };
    let spec = reader.spec();
    let raw: Vec<f32> = reader
        .samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / i16::MAX as f32)
        .collect();
    let mono: Vec<f32> = if spec.channels == 1 {
        raw
    } else {
        raw.chunks(spec.channels as usize)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
            .collect()
    };
    if spec.sample_rate == 16_000 {
        mono
    } else {
        minutes_core::transcribe::resample(&mono, spec.sample_rate, 16_000)
    }
}

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
struct EngineRun {
    wall_ms: u128,
}

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
fn run_one_engine(
    engine: &str,
    samples: &[f32],
    live_jsonl: &Path,
    dest: &Path,
) -> Result<EngineRun, Box<dyn std::error::Error>> {
    use std::sync::mpsc;

    // The sidecar writer overwrites this file each run; remove
    // any leftover so we are sure we capture only this run's output.
    let _ = std::fs::remove_file(live_jsonl);

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_clone = Arc::clone(&stop_flag);

    let mut config = minutes_core::config::Config::default();
    config.transcription.vad_engine = engine.to_string();

    let started = Instant::now();
    let sidecar_handle = std::thread::spawn(move || {
        minutes_core::live_transcript::run_sidecar_mpsc(rx, stop_flag_clone, &config);
    });

    // Feed at full speed. recv_timeout(100ms) tolerates faster
    // arrivals — production cadence is the upper bound, not lower.
    for chunk in samples.chunks(1600) {
        if tx.send(chunk.to_vec()).is_err() {
            break;
        }
    }
    // Drop the sender so the sidecar's recv loop sees Disconnected
    // and exits cleanly via the same finalize path production uses
    // for stop_flag. We do NOT also flip stop_flag — Disconnected is
    // sufficient and the redundant store is dead code (codex
    // diff-review #8).
    drop(tx);
    sidecar_handle.join().expect("sidecar panicked");
    // stop_flag exists for completeness in case the sidecar API
    // wants it later; explicitly silence the unused-var warning.
    let _ = &stop_flag;

    let wall_ms = started.elapsed().as_millis();

    if live_jsonl.exists() {
        std::fs::copy(live_jsonl, dest)?;
        let _ = std::fs::remove_file(live_jsonl);
    } else {
        // Sidecar produced nothing. Touch the dest with empty
        // contents so downstream readers don't error; the report
        // will surface the zero count.
        std::fs::write(dest, "")?;
    }
    let _ = engine; // engine is the human label only; behavior is
                    // already encoded in `config.transcription.vad_engine`.
    Ok(EngineRun { wall_ms })
}

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
fn read_jsonl(path: &Path) -> Vec<minutes_core::live_transcript::TranscriptLine> {
    if !path.exists() {
        return vec![];
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn total_words(lines: &[minutes_core::live_transcript::TranscriptLine]) -> usize {
    lines
        .iter()
        .map(|l| l.text.split_whitespace().count())
        .sum()
}

#[derive(Default, Debug)]
struct DiffBuckets {
    identical: usize,
    near: usize,
    /// Both sides are < 12 chars and differ. Real meetings have a
    /// long tail of short fillers ("Yeah." / "Right." / "Got it.")
    /// where edit distance is unreliable as an "approximately the
    /// same" signal. Counted separately so cross-engine comparison
    /// can report short-utterance churn distinctly from substantive
    /// transcription differences (codex post-fix verification #3).
    short_changed: usize,
    different: usize,
    total: usize,
}

/// Index-aligned diagnostic comparison: walk both transcripts in
/// parallel and bucket each (i, i) pair. This is the simple shape
/// for "did the engines produce the same text in roughly the same
/// order", but it cascades misalignment from the first divergence —
/// if ort splits one whisper utterance into two, every subsequent
/// position is offset by one and reads as different. Treat the
/// numbers as a coarse signal, not segment-by-segment evidence;
/// `set_alignment_buckets` below is the order-insensitive companion.
///
/// Buckets:
/// - "Identical": exact text match.
/// - "Near": both sides ≥ 12 chars AND edit distance ≤ 3.
/// - "Short-changed": both sides < 12 chars AND differ. Captures
///   the "Yeah." vs "Yep." case codex flagged in post-fix
///   verification #3 — without this, short fillers inflated
///   "different" and exaggerated the cross-engine signal.
/// - "Different": substantive transcription difference.
fn diff_buckets(
    a: &[minutes_core::live_transcript::TranscriptLine],
    b: &[minutes_core::live_transcript::TranscriptLine],
) -> DiffBuckets {
    let n = a.len().max(b.len());
    let mut buckets = DiffBuckets {
        total: n,
        ..Default::default()
    };
    for i in 0..n {
        match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if x.text == y.text => buckets.identical += 1,
            (Some(x), Some(y))
                if x.text.len() >= 12
                    && y.text.len() >= 12
                    && edit_distance(&x.text, &y.text) <= 3 =>
            {
                buckets.near += 1
            }
            (Some(x), Some(y)) if x.text.len() < 12 && y.text.len() < 12 => {
                buckets.short_changed += 1
            }
            _ => buckets.different += 1,
        }
    }
    buckets
}

/// Order-insensitive set comparison: every utterance text in `a`
/// counted as "matched" if it appears (exact or near per the same
/// rule as `diff_buckets`) anywhere in `b`. This is robust to
/// merge/split misalignment that index-aligned diff cascades on.
/// Total = max(|a|, |b|).
fn set_alignment_buckets(
    a: &[minutes_core::live_transcript::TranscriptLine],
    b: &[minutes_core::live_transcript::TranscriptLine],
) -> DiffBuckets {
    let mut buckets = DiffBuckets {
        total: a.len().max(b.len()),
        ..Default::default()
    };
    for line_a in a {
        let mut found_identical = false;
        let mut found_near = false;
        for line_b in b {
            if line_a.text == line_b.text {
                found_identical = true;
                break;
            }
            if line_a.text.len() >= 12
                && line_b.text.len() >= 12
                && edit_distance(&line_a.text, &line_b.text) <= 3
            {
                found_near = true;
            }
        }
        if found_identical {
            buckets.identical += 1;
        } else if found_near {
            buckets.near += 1;
        } else if line_a.text.len() < 12 && b.iter().any(|lb| lb.text.len() < 12) {
            // Short utterance with no exact match but at least one
            // short candidate exists in b. Treat as short-churn,
            // not substantive difference.
            buckets.short_changed += 1;
        } else {
            buckets.different += 1;
        }
    }
    // If b has more lines than a, the extras are "different" by
    // definition (they aren't in a).
    if b.len() > a.len() {
        buckets.different += b.len() - a.len();
    }
    buckets
}

fn aggregate_buckets<'a, I: IntoIterator<Item = &'a DiffBuckets>>(buckets: I) -> DiffBuckets {
    buckets
        .into_iter()
        .fold(DiffBuckets::default(), |mut acc, b| {
            acc.identical += b.identical;
            acc.near += b.near;
            acc.short_changed += b.short_changed;
            acc.different += b.different;
            acc.total += b.total;
            acc
        })
}

/// Damerau-Levenshtein-ish edit distance, capped at small values so
/// long lines do not pay full O(mn). Implementation kept simple
/// because we only care about the ≤ 3 bucket boundary.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 3 {
        return 4;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
struct WavReport {
    wav: PathBuf,
    ort: EngineRun,
    whisper_a: EngineRun,
    whisper_b: EngineRun,
    ort_lines: usize,
    whisper_a_lines: usize,
    whisper_b_lines: usize,
    ort_words: usize,
    whisper_a_words: usize,
    whisper_b_words: usize,
    cross_diff: DiffBuckets,
    same_diff: DiffBuckets,
    cross_set: DiffBuckets,
    same_set: DiffBuckets,
    jsonl_paths: [PathBuf; 3],
}

#[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
impl WavReport {
    fn print(&self) {
        let label = self
            .wav
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("wav");
        println!("  {}", label);
        println!(
            "    whisper-silero: lines={} words={} wall={}ms",
            self.whisper_a_lines, self.whisper_a_words, self.whisper_a.wall_ms
        );
        println!(
            "    ort-silero:     lines={} words={} wall={}ms",
            self.ort_lines, self.ort_words, self.ort.wall_ms
        );
        println!(
            "    whisper-silero (b, baseline): lines={} words={} wall={}ms",
            self.whisper_b_lines, self.whisper_b_words, self.whisper_b.wall_ms
        );
        println!(
            "    cross idx-align:  identical={} near={} short_changed={} different={} (n={})",
            self.cross_diff.identical,
            self.cross_diff.near,
            self.cross_diff.short_changed,
            self.cross_diff.different,
            self.cross_diff.total
        );
        println!(
            "    same  idx-align:  identical={} near={} short_changed={} different={} (n={})",
            self.same_diff.identical,
            self.same_diff.near,
            self.same_diff.short_changed,
            self.same_diff.different,
            self.same_diff.total
        );
        println!(
            "    cross set-align:  identical={} near={} short_changed={} different={} (n={})",
            self.cross_set.identical,
            self.cross_set.near,
            self.cross_set.short_changed,
            self.cross_set.different,
            self.cross_set.total
        );
        println!(
            "    same  set-align:  identical={} near={} short_changed={} different={} (n={})",
            self.same_set.identical,
            self.same_set.near,
            self.same_set.short_changed,
            self.same_set.different,
            self.same_set.total
        );
        // Delta vs noise floor. If cross.different is close to or
        // below same.different, the engines disagree no more than
        // greedy-decoding does with itself.
        let cross_diff_idx = self.cross_diff.different as i64;
        let same_diff_idx = self.same_diff.different as i64;
        let cross_diff_set = self.cross_set.different as i64;
        let same_diff_set = self.same_set.different as i64;
        println!(
            "    Δdifferent vs noise: idx-align {:+} | set-align {:+}  (>0 = cross noisier than same)",
            cross_diff_idx - same_diff_idx,
            cross_diff_set - same_diff_set
        );
        if self.ort_lines == 0 && self.whisper_a_lines == 0 {
            println!("    [warn] both engines produced zero utterances on this WAV");
        }
        for p in &self.jsonl_paths {
            println!("    jsonl: {}", p.display());
        }
    }
}
