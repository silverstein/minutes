// ──────────────────────────────────────────────────────────────
// Generates the four parity-test WAV fixtures committed to
// crates/assets/. Run with:
//
//   cargo run --release -p minutes-core --example build_parity_fixtures
//
// The output is deterministic (no RNG, integer math only on the
// existing demo.wav). Re-running on a different machine produces
// byte-identical files. Re-run only if demo.wav changes or a fixture
// definition needs revising; otherwise the committed WAVs in
// crates/assets/ are the canonical source of truth, listenable in any
// audio player so reviewers can sanity-check that what each test
// claims to play actually plays.
//
// Why fixtures instead of in-test synthesis (per the parallel-session
// guardrail): float determinism across machines is one less worry,
// and `say "fixture matches description"` is cheaper than reading
// signal-generation code.
//
// Each fixture targets a parity-bar gap codex flagged in the commit 2
// review. demo.wav alone (continuous speech) trivially passes both
// engines and tells us nothing about boundary drift. These four
// stress different FSM paths:
//
//   parity_brief_spike.wav          — silence + 120ms spike + silence
//   parity_three_utterances.wav     — three utterances, 300/500/800ms gaps
//   parity_low_volume.wav           — full-length speech scaled to ~5%
//   parity_trailing_partial.wav     — truncated to non-multiple of 512
// ──────────────────────────────────────────────────────────────

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::{Path, PathBuf};

const SR: u32 = 16_000;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let assets = manifest.parent().expect("crate parent").join("assets");
    let demo = assets.join("demo.wav");
    assert!(
        demo.exists(),
        "demo.wav missing at {} — cannot build parity fixtures",
        demo.display()
    );

    let demo_samples = read_wav_mono_16khz_f32(&demo);
    println!(
        "demo.wav: {} samples ({:.2}s) at {} Hz",
        demo_samples.len(),
        demo_samples.len() as f32 / SR as f32,
        SR
    );

    // Sanity guard: every fixture below assumes demo.wav is at least
    // ~7 seconds long. If someone shrinks demo.wav, fail loudly here
    // rather than let a fixture silently truncate-by-empty.
    assert!(
        demo_samples.len() >= 7 * SR as usize,
        "demo.wav must be ≥7s for parity fixtures; got {} samples",
        demo_samples.len()
    );

    write_brief_spike(&assets, &demo_samples);
    write_three_utterances(&assets, &demo_samples);
    write_low_volume(&assets, &demo_samples);
    write_trailing_partial(&assets, &demo_samples);

    println!("done. fixtures committed alongside demo.wav.");
}

fn ms_to_samples(ms: usize) -> usize {
    ms * SR as usize / 1000
}

/// Read a WAV as mono f32 at 16 kHz. Mirrors the test helper at
/// `crates/core/src/live_transcript.rs:2740` so the example stays
/// self-contained — `cargo run --example` cannot reach `cfg(test)`
/// helpers.
fn read_wav_mono_16khz_f32(path: &Path) -> Vec<f32> {
    let mut reader = WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let raw: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / i16::MAX as f32)
        .collect();
    let mono = if spec.channels == 1 {
        raw
    } else {
        raw.chunks(spec.channels as usize)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
            .collect()
    };
    assert_eq!(
        spec.sample_rate, SR,
        "demo.wav must be 16 kHz; got {}",
        spec.sample_rate
    );
    mono
}

fn write_wav(path: &Path, samples: &[f32]) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).expect("create wav");
    for &s in samples {
        let pcm = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer.write_sample(pcm).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
    println!(
        "wrote {} ({} samples = {:.3}s)",
        path.display(),
        samples.len(),
        samples.len() as f32 / SR as f32
    );
}

/// Fixture A: 1 s silence + 80 ms spike + 1 s silence.
///
/// 80 ms is well below the 150 ms min_speech_duration the snakers4
/// reference and our smoothing FSM enforce, so a correctly tuned
/// engine reports zero speech segments. This is the regression guard
/// for codex's commit 2 review #1: if the min_speech gate is ever
/// dropped, this fixture flips to "speaking=true for 500 ms" and the
/// parity test fails loud.
///
/// Why 80 ms and not codex's recommended 120 ms: Silero's perceptual
/// window expands a real-speech slice into roughly 160–200 ms of
/// high-probability windows because the model's response continues
/// past the audio boundary. A 120 ms slice produces enough sustained
/// high-prob to exceed the 150 ms gate and get through. 80 ms
/// produces ~120–140 ms of high-prob, which the gate rejects. The
/// goal is to test the gate, so we pick a duration the gate actually
/// rejects.
///
/// The spike is a slice of demo.wav (real speech), not white noise,
/// so the engines actually score high probability on it — that's
/// what makes the min_speech filter the load-bearing line.
fn write_brief_spike(assets: &Path, demo: &[f32]) {
    let lead = vec![0.0_f32; ms_to_samples(1000)];
    let spike_start = ms_to_samples(1000);
    let spike_end = spike_start + ms_to_samples(80);
    let spike = &demo[spike_start..spike_end];
    let tail = vec![0.0_f32; ms_to_samples(1000)];

    let mut out = Vec::with_capacity(lead.len() + spike.len() + tail.len());
    out.extend_from_slice(&lead);
    out.extend_from_slice(spike);
    out.extend_from_slice(&tail);

    write_wav(&assets.join("parity_brief_spike.wav"), &out);
}

/// Fixture B: three 1.5 s utterances separated by 300 / 500 / 800 ms
/// gaps with 200 ms silence head and 800 ms tail.
///
/// 300 ms gap < 500 ms min_silence: the FSM holds the segment open,
/// so utterances 1 and 2 collapse into one island.
/// 500 ms gap == min_silence: ambiguous, but commonly resolves to a
/// segment end at exactly the threshold.
/// 800 ms gap > min_silence: clean split.
///
/// Both engines should produce the same island count and boundary
/// indices (within ±200 ms). Different islands count or drift > 200 ms
/// indicates the FSMs are not aligned.
///
/// We pull the three speech slices from non-overlapping regions of
/// demo.wav (offsets 0.5 / 3.5 / 6.0 s) so the model has fresh
/// content at each segment rather than three copies of the same
/// audio (which would produce identical probability traces and mask
/// state-tensor drift).
fn write_three_utterances(assets: &Path, demo: &[f32]) {
    let utt1 = &demo[ms_to_samples(500)..ms_to_samples(2000)];
    let utt2 = &demo[ms_to_samples(3500)..ms_to_samples(5000)];
    let utt3 = &demo[ms_to_samples(6000)..ms_to_samples(7500)];

    let head = vec![0.0_f32; ms_to_samples(200)];
    let gap1 = vec![0.0_f32; ms_to_samples(300)];
    let gap2 = vec![0.0_f32; ms_to_samples(500)];
    let gap3 = vec![0.0_f32; ms_to_samples(800)];
    let tail = vec![0.0_f32; ms_to_samples(800)];

    let mut out = Vec::new();
    out.extend_from_slice(&head);
    out.extend_from_slice(utt1);
    out.extend_from_slice(&gap1);
    out.extend_from_slice(utt2);
    out.extend_from_slice(&gap2);
    out.extend_from_slice(utt3);
    out.extend_from_slice(&gap3);
    out.extend_from_slice(&tail);

    write_wav(&assets.join("parity_three_utterances.wav"), &out);
}

/// Fixture C: full demo.wav scaled to 5% amplitude.
///
/// At 5% gain the audio sits well below the energy-VAD's adaptive
/// threshold floor but should still be loud enough for Silero (the
/// model is robust to quiet speech). The parity test checks that
/// ort-Silero and whisper-Silero both still detect speech here, even
/// though a naive RMS-only VAD would see silence. If either engine
/// drops the segment, the model-vs-energy gap is wider than expected
/// and we'd want to know.
fn write_low_volume(assets: &Path, demo: &[f32]) {
    let scaled: Vec<f32> = demo.iter().map(|s| s * 0.05).collect();
    write_wav(&assets.join("parity_low_volume.wav"), &scaled);
}

/// Fixture D: demo.wav truncated to a length that is NOT a multiple
/// of 512 (the Silero window size).
///
/// 16384 + 137 = 16521 samples ≈ 1.033 s. 16521 / 512 = 32.27, so
/// the streaming path runs 32 full inferences and leaves 137 samples
/// in the buffer. The recording sidecar's session-stop logic would
/// drain the buffer with zero-padding to a full window. This fixture
/// stresses that final-flush path; if zero-padding broke the LSTM
/// state by introducing an artifact, the last decision would
/// disagree between engines.
fn write_trailing_partial(assets: &Path, demo: &[f32]) {
    let target_len = 16384 + 137;
    assert!(
        demo.len() >= target_len,
        "demo.wav too short for trailing-partial fixture"
    );
    write_wav(
        &assets.join("parity_trailing_partial.wav"),
        &demo[..target_len],
    );
}
