// ──────────────────────────────────────────────────────────────
// Energy-based Voice Activity Detection (VAD).
//
//   AudioChunk.rms ──▶ Vad::process() ──▶ VadResult
//
// Simple energy threshold with adaptive noise floor and hangover.
// No ML model, no Python, ~80 lines. Suitable for:
//   - Teleprompter scroll control (Prompter)
//   - Live transcription preview (Minutes)
//   - Any real-time speech/silence detection
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

/// Voice Activity Detector with adaptive threshold.
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
            hangover_chunks: 5,     // 500ms hangover
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
        for _ in 0..10 { vad.process(0.0005); }
        let r = vad.process(0.05);
        assert!(r.speaking);
        assert_eq!(r.silence_ms, 0);
    }

    #[test]
    fn hangover_prevents_flapping() {
        let mut vad = Vad::new();
        for _ in 0..10 { vad.process(0.0005); }
        vad.process(0.05);
        assert!(vad.is_speaking);
        // Brief silence — hangover keeps speaking
        let r = vad.process(0.0005);
        assert!(r.speaking);
        // After hangover expires
        for _ in 0..6 { vad.process(0.0005); }
        assert!(!vad.process(0.0005).speaking);
    }

    #[test]
    fn noise_floor_adapts() {
        let mut vad = Vad::new();
        let initial = vad.noise_floor;
        for _ in 0..100 { vad.process(0.003); }
        assert!(vad.noise_floor > initial);
    }
}
