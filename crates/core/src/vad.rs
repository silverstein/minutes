// ──────────────────────────────────────────────────────────────
// Voice Activity Detection (VAD).
//
//   samples + rms ──▶ VadEngine::process() ──▶ VadResult
//
// `VadEngine` is the engine-agnostic trait. Concrete impls in this
// crate today: energy (here) and whisper-Silero (in `live_transcript`).
// Callers hold a `Box<dyn VadEngine>` or one of the concrete types.
//
// The energy impl below is also reused directly via its inherent
// `process(rms)` API (Prompter, dictation) — keep that signature stable.
// ──────────────────────────────────────────────────────────────

/// VAD output for each audio chunk.
#[derive(Debug, Clone, Copy)]
pub struct VadResult {
    /// Whether speech is detected.
    pub speaking: bool,
    /// Milliseconds of continuous silence (0 when speaking).
    pub silence_ms: u64,
    /// Current RMS energy level.
    pub energy: f32,
    /// Adaptive noise floor estimate.
    pub noise_floor: f32,
}

/// Engine-agnostic VAD trait.
///
/// Implementations absorb their own transient errors per call and
/// return a `VadResult`; sticky failures (e.g. a model context the
/// engine cannot recover) must surface via `is_healthy() == false` so
/// the composite dispatcher (`RecordingSidecarVad`) can swap to a
/// healthier engine. Returning silence frames forever from a broken
/// engine without flipping `is_healthy` would silently truncate
/// recordings.
///
/// ## Caller contract
///
/// Callers using trait dispatch MUST check [`is_healthy`] immediately
/// after every [`process`] call and replace the engine when it flips
/// to `false`. A failed engine returns a silence-frame `VadResult`,
/// and one such frame can leak through before the dispatcher swaps —
/// downstream utterance-finalization logic must not act on a single
/// post-failure silence frame as if it were authoritative.
///
/// ## Reset is not a recovery primitive
///
/// [`reset`] clears reusable per-utterance state (buffers, accumulated
/// silence durations, adaptive noise floors) on a *healthy* engine.
/// It does NOT clear sticky failure state. A failed engine must be
/// replaced via dispatcher fallback, not "fixed" by calling reset.
///
/// [`is_healthy`]: VadEngine::is_healthy
/// [`process`]: VadEngine::process
/// [`reset`]: VadEngine::reset
pub trait VadEngine: Send {
    /// Process the next audio window.
    ///
    /// `samples` is the new-since-last-call audio at 16 kHz mono f32.
    /// Engines that only need RMS (energy) ignore the slice; engines
    /// that run a model on raw samples consume it.
    ///
    /// On internal failure, implementations return a silence-frame
    /// `VadResult` and surface the failure via `is_healthy() == false`
    /// on the next call. Callers must check `is_healthy` after every
    /// `process` call before treating the result as authoritative.
    fn process(&mut self, samples: &[f32], rms: f32) -> VadResult;

    /// Stable name used for logs and metrics. One of: `"energy"`,
    /// `"whisper-silero"`, `"ort-silero"`.
    fn name(&self) -> &'static str;

    /// Whether the engine is in a usable state. Defaults to `true`.
    /// An engine that hit a sticky failure (model context corrupted,
    /// inference repeatedly erroring, etc.) must override this to
    /// return `false`. Once `false`, it stays `false` for the life of
    /// the engine — `reset` does not flip it back. The dispatcher's
    /// job is to replace the engine, not revive it.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Reset reusable per-utterance state. Default is a no-op; engines
    /// with LSTM hidden state (Silero) or adaptive thresholds (energy)
    /// override to zero state and clear adaptive estimates.
    ///
    /// **Reset does NOT recover a failed engine.** Implementations
    /// must leave any sticky `is_healthy() == false` state intact.
    /// Calling `reset` on an unhealthy engine is a no-op for the
    /// failure flag, by design — failed engines are replaced, not
    /// rebooted.
    fn reset(&mut self) {}
}

/// Energy-based VAD with adaptive threshold. The original VAD impl;
/// suitable for Prompter, dictation fallback, and as the
/// `RecordingSidecarVad` floor.
pub struct Vad {
    noise_floor: f32,
    multiplier: f32,
    is_speaking: bool,
    hangover_chunks: u32,
    hangover_remaining: u32,
    silence_ms: u64,
    chunk_ms: u64,
    adapt_rate: f32,
}

impl Vad {
    /// Create a new VAD with sensible defaults.
    pub fn new() -> Self {
        Self {
            noise_floor: 0.001,
            multiplier: 4.0,
            is_speaking: false,
            hangover_chunks: 5, // 500ms hangover
            hangover_remaining: 0,
            silence_ms: 0,
            chunk_ms: 100,
            adapt_rate: 0.02,
        }
    }

    /// Process one audio chunk's RMS energy and return the VAD result.
    pub fn process(&mut self, rms: f32) -> VadResult {
        let threshold = self.noise_floor * self.multiplier;

        if rms > threshold {
            self.is_speaking = true;
            self.hangover_remaining = self.hangover_chunks;
            self.silence_ms = 0;
        } else if self.hangover_remaining > 0 {
            self.hangover_remaining -= 1;
            self.silence_ms = 0;
        } else {
            self.is_speaking = false;
            self.silence_ms += self.chunk_ms;

            // Adapt noise floor during confirmed silence
            if rms > self.noise_floor {
                self.noise_floor += (rms - self.noise_floor) * self.adapt_rate;
            } else {
                self.noise_floor += (rms - self.noise_floor) * (self.adapt_rate * 3.0);
            }
            self.noise_floor = self.noise_floor.clamp(0.0001, 0.02);
        }

        VadResult {
            speaking: self.is_speaking,
            silence_ms: self.silence_ms,
            energy: rms,
            noise_floor: self.noise_floor,
        }
    }

    /// Reset VAD state.
    pub fn reset(&mut self) {
        self.noise_floor = 0.001;
        self.is_speaking = false;
        self.hangover_remaining = 0;
        self.silence_ms = 0;
    }
}

impl Default for Vad {
    fn default() -> Self {
        Self::new()
    }
}

impl VadEngine for Vad {
    /// Energy VAD ignores the sample slice — adaptive threshold runs
    /// on RMS only. Forwards to the inherent `process(rms)` to keep
    /// the existing per-chunk semantics identical.
    fn process(&mut self, _samples: &[f32], rms: f32) -> VadResult {
        Vad::process(self, rms)
    }

    fn name(&self) -> &'static str {
        "energy"
    }

    fn reset(&mut self) {
        Vad::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_stays_silent() {
        let mut vad = Vad::new();
        for _ in 0..20 {
            let r = vad.process(0.0005);
            assert!(!r.speaking);
        }
        assert!(vad.process(0.0005).silence_ms > 0);
    }

    #[test]
    fn speech_detected() {
        let mut vad = Vad::new();
        for _ in 0..10 {
            vad.process(0.0005);
        }
        let r = vad.process(0.05);
        assert!(r.speaking);
        assert_eq!(r.silence_ms, 0);
    }

    #[test]
    fn hangover_prevents_flapping() {
        let mut vad = Vad::new();
        for _ in 0..10 {
            vad.process(0.0005);
        }
        vad.process(0.05);
        assert!(vad.is_speaking);
        // Brief silence — hangover keeps speaking
        let r = vad.process(0.0005);
        assert!(r.speaking);
        // After hangover expires
        for _ in 0..6 {
            vad.process(0.0005);
        }
        assert!(!vad.process(0.0005).speaking);
    }

    /// A minimal `VadEngine` impl that overrides only `process` and
    /// `name`, used to verify that the trait's `is_healthy` and
    /// `reset` defaults behave as documented.
    struct MinimalEngine;
    impl VadEngine for MinimalEngine {
        fn process(&mut self, _samples: &[f32], rms: f32) -> VadResult {
            VadResult {
                speaking: false,
                silence_ms: 0,
                energy: rms,
                noise_floor: 0.0,
            }
        }
        fn name(&self) -> &'static str {
            "minimal"
        }
    }

    #[test]
    fn trait_defaults_are_healthy_and_no_op_reset() {
        let mut engine = MinimalEngine;
        assert!(engine.is_healthy(), "default is_healthy must be true");
        engine.reset(); // must not panic
        assert!(
            engine.is_healthy(),
            "default reset must not change is_healthy"
        );
    }

    #[test]
    fn energy_engine_reset_via_trait_clears_state() {
        // Ramp the noise floor up via repeated mid-energy samples,
        // then reset via the trait method and confirm the noise floor
        // returns to its initial value. Drift between the trait's
        // `reset` and the inherent `reset` would mean the dispatcher
        // could not reliably re-initialize an engine via trait
        // dispatch in commit 2.
        let mut vad = Vad::new();
        let initial = vad.noise_floor;
        // 0.003 sits below the 0.004 speech threshold (noise_floor *
        // multiplier = 0.001 * 4.0), so the VAD stays in silence and
        // the noise floor adapts upward.
        for _ in 0..200 {
            vad.process(0.003);
        }
        assert!(vad.noise_floor > initial);
        <Vad as VadEngine>::reset(&mut vad);
        assert!(
            (vad.noise_floor - initial).abs() < f32::EPSILON,
            "trait reset must restore noise_floor to initial"
        );
        assert!(<Vad as VadEngine>::is_healthy(&vad));
    }

    #[test]
    fn energy_engine_trait_matches_inherent_process() {
        // The trait impl must produce identical decisions to the
        // inherent `process(rms)`, since dictation/Prompter still call
        // the inherent path. Drift between the two would silently
        // diverge VAD behavior depending on call site.
        let rms_sequence = [0.0005_f32, 0.0005, 0.05, 0.05, 0.0005];
        let mut inherent = Vad::new();
        let mut via_trait = Vad::new();
        for &rms in &rms_sequence {
            let a = inherent.process(rms);
            let b = <Vad as VadEngine>::process(&mut via_trait, &[], rms);
            assert_eq!(a.speaking, b.speaking);
            assert_eq!(a.silence_ms, b.silence_ms);
            assert_eq!(a.energy, b.energy);
        }
        assert_eq!(<Vad as VadEngine>::name(&via_trait), "energy");
    }

    #[test]
    fn noise_floor_adapts() {
        let mut vad = Vad::new();
        let initial = vad.noise_floor;
        for _ in 0..100 {
            vad.process(0.003);
        }
        assert!(vad.noise_floor > initial);
    }
}
