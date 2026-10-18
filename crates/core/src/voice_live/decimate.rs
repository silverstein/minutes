//! Streaming decimation for the voice-processing microphone path.
//!
//! The voice-processing unit runs both directions at the provider's 24 kHz, but
//! the provider wants 16 kHz microphone audio. A 32-tap Hann-windowed sinc
//! low-passes at the 16 kHz Nyquist before the 3:2 rate change, so nothing above
//! 8 kHz folds back into the speech band. Phase and kernel history carry across
//! calls, so feeding audio in callback-sized pieces yields the same samples as
//! one pass over the whole signal.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Kernel length in input samples.
const TAPS: usize = 32;
const HALF: usize = TAPS / 2;

/// Streaming sample-rate reducer from `from_rate` to `to_rate`.
pub struct Decimator {
    history: VecDeque<f32>,
    /// Position of the next output sample, as an index into `history`.
    pos: f64,
    /// Input samples per output sample.
    step: f64,
    /// Twice the normalized cutoff frequency (the sinc's DC gain).
    cutoff: f64,
}

impl Decimator {
    /// Build a decimator; `from_rate` must be at least `to_rate`.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        assert!(from_rate >= to_rate, "decimator only reduces the rate");
        let mut history = VecDeque::with_capacity(TAPS * 8);
        history.extend(std::iter::repeat_n(0.0f32, HALF));
        Self {
            history,
            pos: HALF as f64,
            step: from_rate as f64 / to_rate as f64,
            cutoff: to_rate as f64 / from_rate as f64,
        }
    }

    /// Feed input samples and append every output sample that is now computable.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.history.extend(input.iter().copied());
        while ((self.pos + HALF as f64).floor() as usize) < self.history.len() {
            out.push(self.sample_at(self.pos));
            self.pos += self.step;
        }
        let keep_from = (self.pos - HALF as f64).floor();
        if keep_from > 0.0 {
            let drop = keep_from as usize;
            self.history.drain(..drop);
            self.pos -= drop as f64;
        }
    }

    fn sample_at(&self, pos: f64) -> f32 {
        let first = (pos - HALF as f64).ceil().max(0.0) as usize;
        let last = ((pos + HALF as f64).floor() as usize).min(self.history.len() - 1);
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for k in first..=last {
            let u = k as f64 - pos;
            let window = 0.5 * (1.0 + (PI * u / HALF as f64).cos());
            let sinc = if u.abs() < 1e-9 {
                1.0
            } else {
                let x = PI * self.cutoff * u;
                x.sin() / x
            };
            let h = self.cutoff * sinc * window;
            acc += h * self.history[k] as f64;
            norm += h;
        }
        if norm.abs() > 1e-6 {
            (acc / norm) as f32
        } else {
            acc as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn output_rate_is_two_thirds_after_a_fixed_startup_delay() {
        let mut d = Decimator::new(24_000, 16_000);
        let mut out = Vec::new();
        d.push(&vec![0.0; 24_000], &mut out);
        let first_second = out.len();
        // The kernel needs half its width of lookahead, so the first second is
        // short by that fixed delay and by no more than that.
        assert!(
            (16_000 - first_second) <= HALF + 1,
            "first second {first_second}"
        );
        d.push(&vec![0.0; 24_000], &mut out);
        // The shortfall is startup latency, not drift, so it does not grow.
        assert_eq!(out.len() - first_second, 16_000);
    }

    #[test]
    fn speech_band_tone_keeps_its_level() {
        let mut d = Decimator::new(24_000, 16_000);
        let mut out = Vec::new();
        d.push(&tone(1_000.0, 24_000, 0.5, 4_800), &mut out);
        let steady = &out[400..out.len() - 400];
        let expected = 0.5 / 2f32.sqrt();
        assert!(
            (rms(steady) - expected).abs() < expected * 0.05,
            "rms {}",
            rms(steady)
        );
    }

    #[test]
    fn tone_above_new_nyquist_is_removed_not_aliased() {
        let mut d = Decimator::new(24_000, 16_000);
        let mut out = Vec::new();
        d.push(&tone(11_000.0, 24_000, 0.5, 4_800), &mut out);
        let steady = &out[400..out.len() - 400];
        assert!(rms(steady) < 0.05, "rms {}", rms(steady));
    }

    #[test]
    fn callback_sized_pieces_match_one_pass() {
        let input = tone(700.0, 24_000, 0.3, 5_000);
        let mut whole = Vec::new();
        Decimator::new(24_000, 16_000).push(&input, &mut whole);
        let mut pieces = Vec::new();
        let mut d = Decimator::new(24_000, 16_000);
        for chunk in input.chunks(37) {
            d.push(chunk, &mut pieces);
        }
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
