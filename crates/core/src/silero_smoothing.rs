// ──────────────────────────────────────────────────────────────
// Silero VAD smoothing (VADIterator-shaped, snakers4 reference).
//
//   probability per 512-sample window ──▶ Smoothing::feed() ──▶ SmoothedDecision
//
// Hosts the threshold/min-speech/min-silence/pad logic snakers4's
// silero-vad applies on top of raw Silero ONNX output. Lives in its
// own module so it can be unit-tested with synthetic probability
// streams independent of the ort feature, and so the ort-Silero
// engine in `silero_vad` and any future on-device VAD can share it.
//
// Why this is its own thing instead of inline in OrtSileroVad:
// codex caught the original 4-state FSM in PLAN-vad-refactor.md as
// too simplified — the reference implementation uses hysteresis (a
// `neg_threshold = threshold - 0.15` band where neither speech nor
// silence is committed) plus a `temp_end` accounting trick that a
// naive port would silently get wrong on boundary-grazing audio.
// ──────────────────────────────────────────────────────────────

/// Window size in samples that the FSM expects per `feed` call. The
/// caller (OrtSileroVad) feeds a probability scalar produced by
/// running the ONNX session on exactly 512 samples at 16 kHz, so each
/// window represents 32 ms of audio.
pub const WINDOW_SAMPLES: usize = 512;

/// Sample rate the FSM assumes when converting durations between
/// samples and milliseconds.
pub const SAMPLE_RATE: u32 = 16_000;

/// Hysteresis band: the model has to drop below `threshold - 0.15`
/// before silence is even *provisional*. Without this, a single
/// boundary-grazing window flips the segment.
pub const HYSTERESIS_DROP: f32 = 0.15;

/// Per-window decision the FSM produces. `speaking` is the in-segment
/// flag; `silence_ms` is the run-length of consecutive non-speaking
/// windows since the last speech segment ended (resets to 0 on entry
/// to a segment).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothedDecision {
    pub speaking: bool,
    pub silence_ms: u64,
}

/// Smoothing parameters. Defaults match the existing whisper-Silero
/// constants in `live_transcript`.
#[derive(Debug, Clone, Copy)]
pub struct SmoothingParams {
    /// Positive threshold for entering a speech segment.
    pub threshold: f32,
    /// Hysteresis: silence is only provisional once probability drops
    /// below this. Defaults to `threshold - HYSTERESIS_DROP`.
    pub neg_threshold: f32,
    /// Minimum continuous speech duration. Segments shorter than this
    /// are dropped (filtered post-construction, NOT during the walk).
    pub min_speech_duration_ms: u32,
    /// Continuous silence required before declaring a segment ended.
    pub min_silence_duration_ms: u32,
    /// Padding applied to the recorded start/end timestamps. Does NOT
    /// alter FSM transitions; only shifts boundaries on emission.
    pub speech_pad_ms: u32,
}

impl SmoothingParams {
    /// Defaults that mirror `crates/core/src/live_transcript.rs:1101-1114`.
    pub fn whisper_silero_defaults() -> Self {
        Self {
            threshold: 0.2,
            neg_threshold: 0.2 - HYSTERESIS_DROP,
            min_speech_duration_ms: 150,
            min_silence_duration_ms: 500,
            speech_pad_ms: 80,
        }
    }
}

impl Default for SmoothingParams {
    fn default() -> Self {
        Self::whisper_silero_defaults()
    }
}

/// VADIterator-shaped streaming smoother. State is intentionally
/// minimal — a `triggered` boolean, a deferred-end sample index, and
/// a "validated" flag — matching snakers4's reference so future drift
/// between us and the canonical implementation is easy to spot.
///
/// `speaking` emission is gated by `min_speech_duration`. A segment
/// has to accumulate at least `min_speech_samples` of *consecutive
/// above-threshold* probability before it is considered validated.
/// Until then, `speaking` stays false even though `triggered` is true.
/// A brief probability spike that doesn't reach the threshold is
/// fully suppressed: the segment stays alive (via min_silence) but
/// `validated` never flips, so `speaking` never emits. Once
/// validated, the segment continues to emit speech through hysteresis
/// dips until `min_silence_duration` ends it.
///
/// Trade-off: ~150 ms latency at segment start, in exchange for not
/// opening the ASR gate on a 30 ms cough.
#[derive(Debug, Clone)]
pub struct Smoothing {
    params: SmoothingParams,
    /// Currently inside a candidate speech segment (probability
    /// crossed `threshold` and `min_silence_duration` has not yet
    /// elapsed)? FSM-level flag; `speaking` is gated additionally on
    /// `validated`.
    triggered: bool,
    /// Whether the current segment has accumulated enough continuous
    /// above-threshold probability to be considered real speech.
    /// Once true, stays true until the segment ends. Reset when the
    /// segment ends. Without this flag, a 30 ms blip would emit
    /// speaking=true for the duration of min_silence (~500 ms).
    validated: bool,
    /// Continuous high-probability sample run within the current
    /// segment. Resets to 0 on entry to a new segment OR on a dip
    /// below `neg_threshold` (a hysteresis-zone dip preserves it).
    /// Once it crosses `min_speech_samples`, `validated` flips to
    /// true and stays true for the segment.
    high_prob_run_samples: u64,
    /// Sample index where probability first dropped below
    /// `neg_threshold` while `triggered`. `0` means "no pending end".
    /// Used to enforce `min_silence_duration_ms` before committing a
    /// segment end.
    temp_end_sample: u64,
    /// Running input sample counter. Incremented by `WINDOW_SAMPLES`
    /// after every `feed` call.
    current_sample: u64,
    /// Sample index at which the current candidate segment started.
    /// Kept for observability/logging; the load-bearing gate uses
    /// `high_prob_run_samples`.
    active_segment_start_sample: Option<u64>,
    /// Run-length of consecutive non-speaking windows since the last
    /// validated segment ended. In milliseconds for caller convenience.
    silence_ms: u64,
}

impl Smoothing {
    pub fn new(params: SmoothingParams) -> Self {
        Self {
            params,
            triggered: false,
            validated: false,
            high_prob_run_samples: 0,
            temp_end_sample: 0,
            current_sample: 0,
            active_segment_start_sample: None,
            silence_ms: 0,
        }
    }

    /// Reset all state. Called between recordings so the FSM does not
    /// carry a stale `temp_end_sample` or partial validation into a
    /// fresh session.
    pub fn reset(&mut self) {
        self.triggered = false;
        self.validated = false;
        self.high_prob_run_samples = 0;
        self.temp_end_sample = 0;
        self.current_sample = 0;
        self.active_segment_start_sample = None;
        self.silence_ms = 0;
    }

    /// Feed one Silero probability for the next 512-sample window.
    /// Returns the smoothed per-window decision.
    pub fn feed(&mut self, prob: f32) -> SmoothedDecision {
        let window_samples = WINDOW_SAMPLES as u64;
        let window_ms = window_samples * 1000 / SAMPLE_RATE as u64;
        let min_silence_samples =
            (self.params.min_silence_duration_ms as u64) * (SAMPLE_RATE as u64) / 1000;
        let min_speech_samples =
            (self.params.min_speech_duration_ms as u64) * (SAMPLE_RATE as u64) / 1000;

        self.current_sample = self.current_sample.saturating_add(window_samples);

        // High prob: cancel any pending segment end. Silence we
        // accumulated between the dip and now is undone — the speaker
        // didn't really stop, the model just dipped briefly.
        if prob >= self.params.threshold && self.temp_end_sample != 0 {
            self.temp_end_sample = 0;
        }

        // Entering a segment.
        if prob >= self.params.threshold && !self.triggered {
            self.triggered = true;
            self.validated = false;
            self.high_prob_run_samples = 0;
            self.active_segment_start_sample = Some(self.current_sample);
        }

        // Accumulate continuous high-probability run while the
        // segment is active. Once the run reaches min_speech_samples,
        // `validated` latches true for the rest of the segment. A dip
        // into the hysteresis zone (between neg_threshold and
        // threshold) preserves the run; only a dip below
        // neg_threshold resets it. This is the load-bearing fix for
        // codex review #1: a 30 ms spike never accumulates enough
        // continuous high-prob to validate, so `speaking` never emits.
        if self.triggered && prob >= self.params.threshold {
            self.high_prob_run_samples = self.high_prob_run_samples.saturating_add(window_samples);
            if self.high_prob_run_samples >= min_speech_samples {
                self.validated = true;
            }
        }

        // Provisional silence: probability has dropped below
        // hysteresis floor while triggered. Reset the high-prob run
        // (validation is sticky for the segment, but the run counter
        // is not), mark `temp_end` if we haven't already, and check
        // whether we've held below long enough to commit a segment
        // end.
        if prob < self.params.neg_threshold && self.triggered {
            self.high_prob_run_samples = 0;
            if self.temp_end_sample == 0 {
                self.temp_end_sample = self.current_sample;
            }
            if self.current_sample.saturating_sub(self.temp_end_sample) >= min_silence_samples {
                self.triggered = false;
                self.validated = false;
                self.temp_end_sample = 0;
                self.active_segment_start_sample = None;
            }
        }

        // Per-window output. `speaking` is `triggered` AND
        // `validated`. Without the validation gate, a brief blip
        // would emit `speaking=true` for the entire min_silence
        // window. With it, sub-threshold segments are fully
        // suppressed.
        let speaking = self.triggered && self.validated;

        if speaking {
            self.silence_ms = 0;
        } else {
            self.silence_ms = self.silence_ms.saturating_add(window_ms);
        }

        SmoothedDecision {
            speaking,
            silence_ms: self.silence_ms,
        }
    }

    /// Snapshot useful for observability (logs, metrics).
    pub fn current_sample(&self) -> u64 {
        self.current_sample
    }
}

// Note on `min_speech_duration` enforcement: snakers4's VADIterator
// emits a speech-start event the moment `triggered` flips, then on
// segment-end filters segments by length. We do the equivalent for a
// streaming per-window API by holding back the `speaking=true` output
// until the candidate segment has lasted at least `min_speech_samples`.
// If silence arrives before the gate opens, no `speaking=true` ever
// leaks — the brief spike is fully suppressed.
//
// Trade-off: ~150 ms latency at segment start, in exchange for not
// opening the ASR gate on a 30 ms cough. The recording sidecar's
// downstream whisper feed has its own buffering, so this latency is
// hidden in the pipeline.

#[cfg(test)]
mod tests {
    use super::*;

    fn run(probs: &[f32]) -> Vec<SmoothedDecision> {
        let mut sm = Smoothing::new(SmoothingParams::whisper_silero_defaults());
        probs.iter().map(|&p| sm.feed(p)).collect()
    }

    #[test]
    fn pure_silence_never_triggers() {
        // 100 windows of 0.0 prob. silence_ms accumulates by 32 ms
        // per window.
        let out = run(&vec![0.0; 100]);
        assert!(out.iter().all(|d| !d.speaking));
        assert_eq!(out.last().unwrap().silence_ms, 100 * 32);
    }

    #[test]
    fn pure_speech_triggers_after_min_speech_gate() {
        // The min_speech gate (150 ms ≈ 5 windows of 32 ms) delays
        // `speaking=true` emission. This is the load-bearing fix
        // codex flagged in commit 2's review: without the gate, a
        // 30 ms cough opens the ASR pipeline for ~500 ms.
        let out = run(&vec![0.9_f32; 100]);
        assert!(
            !out[0].speaking,
            "first window must hold back behind min_speech gate"
        );
        // By window 5+ (160 ms cumulative, > 150 ms min_speech), the
        // gate opens. Allow ±1 window for boundary sample-vs-ms math.
        assert!(
            out[5..].iter().all(|d| d.speaking),
            "speech must emit once min_speech (150 ms) has elapsed"
        );
        assert!(out[5..].iter().all(|d| d.silence_ms == 0));
    }

    #[test]
    fn brief_spike_under_min_speech_never_emits_speech() {
        // The case codex specifically called out: 3 windows
        // (~96 ms, < 150 ms min_speech) of high probability followed
        // by silence. With the min_speech gate the spike is fully
        // suppressed — `speaking=false` throughout. Without the gate,
        // these 3 windows would set `speaking=true` and the silence
        // detector would take 500 ms+ to recover, opening the ASR
        // pipeline on every cough or click.
        let mut probs = vec![0.9_f32; 3];
        probs.extend(vec![0.0_f32; 100]);
        let out = run(&probs);
        assert!(
            out.iter().all(|d| !d.speaking),
            "brief spike (<150 ms) must NOT emit speaking"
        );
    }

    #[test]
    fn hysteresis_zone_does_not_flip_after_triggered() {
        // Once a candidate segment is established and held for at
        // least min_speech (150 ms ≈ 5 windows), probability between
        // neg_threshold (0.05) and threshold (0.2) keeps the segment
        // alive — neither commits a new entry nor commits a silence.
        let mut probs = vec![0.9_f32; 6]; // enter + clear min_speech gate
        probs.extend(vec![0.1_f32; 50]); // hover in hysteresis zone
        let out = run(&probs);
        // Windows 0..5 are within the min_speech holdback.
        assert!(
            out[5..].iter().all(|d| d.speaking),
            "hysteresis zone must not end the segment after the gate opens"
        );
    }

    #[test]
    fn hysteresis_zone_does_not_trigger_from_silence() {
        // From silence, probability in the hysteresis band (0.05..0.2)
        // never enters a segment. Only crossing `threshold` does.
        let probs = vec![0.15_f32; 100];
        let out = run(&probs);
        assert!(out.iter().all(|d| !d.speaking));
    }

    #[test]
    fn brief_dip_below_neg_threshold_does_not_end_segment() {
        // Enter segment, hold long enough to clear min_speech, then
        // dip below neg_threshold for a few windows (well under the
        // 500 ms min_silence), then climb back. The segment must
        // remain emitting `speaking=true` throughout.
        let mut probs = vec![0.9_f32; 10]; // enter + clear min_speech gate
        probs.extend(vec![0.0_f32; 5]); // ~160ms silence (< 500ms)
        probs.extend(vec![0.9_f32; 5]); // back to speech
        let out = run(&probs);
        // Once the gate opens (~window 5), all subsequent windows
        // must report speaking.
        assert!(
            out[5..].iter().all(|d| d.speaking),
            "dip under min_silence must not end segment after gate opens"
        );
    }

    #[test]
    fn long_silence_after_speech_ends_segment() {
        // Enter segment, hold long enough to clear min_speech,
        // then drop below neg_threshold and hold for > 500 ms
        // (16 windows of 32 ms). Segment must end.
        let mut probs = vec![0.9_f32; 15]; // enter + clear min_speech
        probs.extend(vec![0.0_f32; 25]); // ~800 ms silence > 500 ms
        let out = run(&probs);
        // Windows 5..15 are speaking; windows 15+ start dropping below
        // neg_threshold; speaking holds until min_silence elapses (16
        // windows after the dip starts at window 15, so the segment
        // ends around window 31).
        assert!(out[5..15].iter().all(|d| d.speaking));
        let first_silent = out[15..]
            .iter()
            .position(|d| !d.speaking)
            .expect("must drop out of segment after long silence");
        // min_silence_samples = 8000 ≈ 16 windows. Segment ends
        // ~16 windows after the dip (window 31). Allow some slack.
        assert!(
            first_silent <= 20,
            "segment must end within ~16 windows of dip start, got {}",
            first_silent
        );
    }

    #[test]
    fn reset_clears_segment_state() {
        let mut sm = Smoothing::new(SmoothingParams::whisper_silero_defaults());
        for _ in 0..20 {
            sm.feed(0.9);
        }
        assert!(sm.feed(0.9).speaking, "gate must be open after 20 windows");
        sm.reset();
        // After reset, current_sample is back to 0 — the next 0.9
        // re-establishes triggered but the min_speech gate has to
        // re-open from scratch. So the immediate next window does
        // NOT report speaking.
        let after = sm.feed(0.9);
        assert!(
            !after.speaking,
            "after reset, min_speech gate must close again"
        );
        assert_eq!(sm.current_sample(), WINDOW_SAMPLES as u64);
    }

    #[test]
    fn silence_ms_resets_on_segment_entry() {
        // Accumulate silence, then a long run of speech. Once the
        // min_speech gate opens, silence_ms resets to 0.
        let mut probs = vec![0.0_f32; 30]; // accumulate silence
        probs.extend(vec![0.9_f32; 10]); // enter + clear min_speech gate
        let out = run(&probs);
        assert_eq!(out[29].silence_ms, 30 * 32);
        // The first speech window after silence is gated — speaking
        // false, silence_ms keeps growing.
        assert!(!out[30].speaking);
        // By window 35 (5 windows of speech, gate has just opened),
        // speaking is true and silence_ms reset.
        assert!(out[35].speaking);
        assert_eq!(out[35].silence_ms, 0);
    }

    #[test]
    fn boundary_at_threshold_triggers() {
        // Probability exactly at threshold (0.2) is "speech" by the
        // `>=` comparison. Drift between a `>` and `>=` here would
        // silently shift segment boundaries. Long enough run that
        // min_speech opens and we see `speaking=true`.
        let out = run(&[0.2_f32; 10]);
        assert!(
            out[5..].iter().all(|d| d.speaking),
            "exactly-at-threshold must enter segment after gate opens"
        );
    }

    #[test]
    fn boundary_at_neg_threshold_does_not_end_segment() {
        // While triggered, probability exactly at neg_threshold (0.05)
        // is still "above" the hysteresis floor (`<` is strict). Must
        // not commit a temp_end.
        let mut probs = vec![0.9_f32; 10]; // enter + clear gate
        probs.extend(vec![0.05_f32; 50]); // exactly at neg_threshold
        let out = run(&probs);
        assert!(
            out[5..].iter().all(|d| d.speaking),
            "exactly-at-neg_threshold must keep segment open"
        );
    }
}
