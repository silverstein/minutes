use crate::transcribe::{set_abort_callback, streaming_whisper_params};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use whisper_rs::WhisperContext;

// ──────────────────────────────────────────────────────────────
// Streaming whisper transcription — progressive text output.
//
// Instead of batch (accumulate all audio → transcribe once),
// this transcribes in rolling windows while the user speaks:
//
//   Audio chunks accumulate:
//     [0s──────2s]                         → whisper → "Switch to monthly"
//     [0s──────────────4s]                 → whisper → "Switch to monthly billing for"
//     [0s──────────────────────6s]         → whisper → "Switch to monthly billing for consultants"
//     [0s──────────────────────────────8s] → (silence) → FINAL
//
// Key design decisions:
//   - Full re-transcription on each pass (not incremental). Whisper
//     is fast enough on the accumulated buffer because we're using
//     the small/base model and utterances are short (<2 min).
//   - No segment stitching needed. Until the configured cost ceiling we
//     transcribe from t=0; longer speech uses the newest bounded window.
//     Each pass replaces the previous, and the final still uses everything.
//   - Partial results are emitted via callback; the final result on
//     silence replaces all partials.
//   - Uses the same WhisperContext (preloaded model) as batch mode.
//
// Why full re-transcription instead of incremental:
//   Incremental (transcribe only the new 2s chunk) produces worse
//   quality because whisper loses context from earlier speech.
//   Full re-transcription from t=0 gives consistent output for typical
//   utterances. Once the configured ceiling is reached, a rolling window
//   prevents latency from growing without bound while keeping current speech
//   available. Finalization retains the complete utterance.
//
// Performance budget:
//   - base model: ~200ms for 10s audio on M-series
//   - small model: ~500ms for 10s audio on M-series
//   - Transcription runs on a background thread; audio capture
//     continues uninterrupted on the main thread.
// ──────────────────────────────────────────────────────────────

/// Whisper passes that returned an error since the last reset. Read by the
/// recording sidecar's session summary; reset when a sidecar starts.
static WHISPER_FAILURES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn failure_count() -> u64 {
    WHISPER_FAILURES.load(Ordering::Relaxed)
}

pub(crate) fn reset_failure_count() {
    WHISPER_FAILURES.store(0, Ordering::Relaxed);
}

/// How often to run partial transcription (in audio samples at 16kHz).
const PARTIAL_INTERVAL_SAMPLES: usize = 16000 * 2; // Every 2 seconds

/// Minimum audio length to attempt transcription (avoid noise-only runs).
const MIN_TRANSCRIBE_SAMPLES: usize = 16000; // 1 second

/// Default cap for partial transcription cost. See `StreamingWhisper::new` for
/// the full reasoning — past this many seconds of accumulated audio, partial
/// passes are skipped (the utterance still finalizes correctly).
pub const DEFAULT_PARTIAL_MAX_SECS: u32 = 30;

/// Result from a streaming transcription pass.
#[derive(Debug, Clone)]
pub struct StreamingResult {
    /// The transcribed text (replaces any previous partial).
    pub text: String,
    /// Whether this is a final result (silence detected) or partial (still speaking).
    pub is_final: bool,
    /// Duration of audio transcribed in seconds.
    pub duration_secs: f64,
}

/// Streaming whisper transcriber. Holds the accumulated audio buffer
/// and runs partial transcriptions at intervals.
pub struct StreamingWhisper {
    /// All audio samples accumulated so far (16kHz mono f32).
    audio_buffer: Vec<f32>,
    /// Samples since last partial transcription.
    samples_since_partial: usize,
    /// The last partial text emitted (for dedup).
    last_partial: String,
    /// Number of CPU threads for whisper.
    n_threads: i32,
    /// Language hint (None = auto-detect).
    language: Option<String>,
    /// Whether we've created a state before (suppress init noise on subsequent calls).
    has_created_state: bool,
    /// Cap on partial-transcription window length, in samples at 16kHz. Past
    /// this length, partials use the newest bounded window while finalization
    /// still uses the complete utterance.
    partial_max_samples: usize,
    /// Optional session stop signal. Recording sidecars use this to abort an
    /// in-flight Whisper pass so optional live evidence cannot hold capture
    /// shutdown open after the WAV has been sealed.
    abort_signal: Option<Arc<AtomicBool>>,
}

impl StreamingWhisper {
    /// Create a new streaming transcriber with the default partial cap (30s).
    pub fn new(language: Option<String>) -> Self {
        Self::with_partial_max_secs(language, DEFAULT_PARTIAL_MAX_SECS)
    }

    /// Create a new streaming transcriber with a custom partial-cap limit
    /// (in seconds). Past this many seconds of accumulated audio the partial
    /// `state.full(...)` pass uses the newest bounded window. The utterance
    /// still finalizes from its complete buffer via `finalize()` when the
    /// caller (typically VAD/silence detection in `live_transcript.rs`)
    /// decides the utterance is over.
    ///
    /// Why this matters: partial cost is O(buffer_len). At ~200ms per 10s of
    /// audio on Apple Silicon with the base model, a 60s buffer takes ~1.2s
    /// per partial — slower than the 2s partial interval, so partials queue
    /// up and fall further behind. 30s keeps each partial well under the
    /// interval and stops the runaway.
    pub fn with_partial_max_secs(language: Option<String>, partial_max_secs: u32) -> Self {
        let partial_max_samples = (partial_max_secs as usize).saturating_mul(16000);
        Self {
            audio_buffer: Vec::with_capacity(16000 * 30), // pre-alloc 30s
            samples_since_partial: 0,
            last_partial: String::new(),
            n_threads: num_cpus(),
            language,
            has_created_state: false,
            partial_max_samples,
            abort_signal: None,
        }
    }

    /// Abort an in-flight Whisper pass when `abort_signal` becomes true.
    ///
    /// Ordinary standalone streaming keeps the historical behavior. Capture
    /// sidecars opt in because their draft/final evidence is optional and must
    /// yield immediately to recording shutdown.
    pub fn with_abort_signal(mut self, abort_signal: Arc<AtomicBool>) -> Self {
        self.abort_signal = Some(abort_signal);
        self
    }

    /// Feed audio samples. Returns a partial result if enough audio has
    /// accumulated since the last transcription.
    ///
    /// Once `audio_buffer` exceeds `partial_max_samples`, partials use the
    /// newest bounded window to avoid CPU runaway while staying current during
    /// long uninterrupted speech. `finalize()` still sees the full utterance.
    pub fn feed(&mut self, samples: &[f32], ctx: &WhisperContext) -> Option<StreamingResult> {
        self.audio_buffer.extend_from_slice(samples);
        self.samples_since_partial += samples.len();

        // Only transcribe if enough new audio AND enough total audio
        if self.samples_since_partial >= PARTIAL_INTERVAL_SAMPLES
            && self.audio_buffer.len() >= MIN_TRANSCRIBE_SAMPLES
        {
            self.samples_since_partial = 0;
            return self.transcribe(ctx, false);
        }

        None
    }

    /// Finalize: run one last transcription and return the final result.
    /// Call this when silence is detected or the user stops.
    pub fn finalize(&mut self, ctx: &WhisperContext) -> Option<StreamingResult> {
        if self.audio_buffer.len() < MIN_TRANSCRIBE_SAMPLES {
            return None;
        }
        self.transcribe(ctx, true)
    }

    /// Reset the buffer for the next utterance (keeps the model loaded).
    pub fn reset(&mut self) {
        self.audio_buffer.clear();
        self.samples_since_partial = 0;
        self.last_partial.clear();
    }

    /// Total audio duration accumulated so far.
    pub fn duration_secs(&self) -> f64 {
        self.audio_buffer.len() as f64 / 16000.0
    }

    fn transcription_window_start(&self, is_final: bool) -> usize {
        if is_final || self.partial_max_samples == 0 {
            0
        } else {
            self.audio_buffer
                .len()
                .saturating_sub(self.partial_max_samples)
        }
    }

    /// Run whisper on the full accumulated buffer.
    fn transcribe(&mut self, ctx: &WhisperContext, is_final: bool) -> Option<StreamingResult> {
        // Suppress whisper's noisy C-level stderr output on subsequent state creations.
        // The first call prints GPU/backend info (useful); subsequent calls repeat it (noise).
        let mut state = if self.has_created_state {
            // Redirect stderr to /dev/null during state creation
            let state = suppress_stderr(|| ctx.create_state().ok());
            state?
        } else {
            self.has_created_state = true;
            ctx.create_state().ok()?
        };

        let mut params = streaming_whisper_params();
        params.set_n_threads(self.n_threads);
        params.set_language(self.language.as_deref());
        // Stack-owned so it outlives `state.full`; whisper-rs's closure
        // setter is unsound for capturing closures (see `set_abort_callback`).
        let abort_signal = self.abort_signal.clone();
        let abort_when_stopped = move || {
            abort_signal
                .as_ref()
                .is_some_and(|signal| signal.load(Ordering::Relaxed))
        };
        if self.abort_signal.is_some() {
            set_abort_callback(&mut params, &abort_when_stopped);
        }

        let start = std::time::Instant::now();

        let window_start = self.transcription_window_start(is_final);
        let transcription_audio = &self.audio_buffer[window_start..];
        if let Err(e) = state.full(params, transcription_audio) {
            tracing::warn!("streaming whisper failed: {}", e);
            // The desktop app has no tracing subscriber; persist a bounded
            // trail so a whisper pass that fails on every utterance is
            // visible in minutes.log rather than only as a thin transcript.
            let failures = WHISPER_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            if crate::live_transcript::should_persist_repeat(failures) {
                crate::logging::append_log(&serde_json::json!({
                    "ts": chrono::Local::now().to_rfc3339(),
                    "level": "warn",
                    "step": "streaming_whisper_failed",
                    "file": "",
                    "message": "streaming whisper pass failed; the utterance produced no text",
                    "extra": {
                        "error": e.to_string(),
                        "failures": failures,
                        "is_final": is_final,
                        "audio_secs": format!("{:.1}", transcription_audio.len() as f64 / 16000.0),
                    },
                }))
                .ok();
            }
            return None;
        }

        let elapsed_ms = start.elapsed().as_millis();
        let duration_secs = transcription_audio.len() as f64 / 16000.0;

        // Extract text from all segments
        let num_segments = state.full_n_segments();
        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(t) = seg.to_str_lossy() {
                    let t = t.trim();
                    if !t.is_empty() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(t);
                    }
                }
            }
        }

        let text = text.trim().to_string();

        // Skip if empty or identical to last partial (no new info)
        if text.is_empty() {
            return None;
        }
        if !is_final && text == self.last_partial {
            return None;
        }

        tracing::debug!(
            partial = !is_final,
            words = text.split_whitespace().count(),
            audio_secs = format!("{:.1}", duration_secs),
            whisper_ms = elapsed_ms,
            "streaming transcription"
        );

        self.last_partial = text.clone();

        Some(StreamingResult {
            text,
            is_final,
            duration_secs,
        })
    }
}

/// Temporarily suppress stderr (whisper C code prints noisy init logs).
fn suppress_stderr<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let stderr_fd = std::io::stderr().as_raw_fd();
        let saved = unsafe { libc::dup(stderr_fd) };
        if saved >= 0 {
            let devnull = std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .ok();
            if let Some(ref dn) = devnull {
                unsafe { libc::dup2(dn.as_raw_fd(), stderr_fd) };
            }
            let result = f();
            unsafe { libc::dup2(saved, stderr_fd) };
            unsafe { libc::close(saved) };
            return result;
        }
    }
    f()
}

fn num_cpus() -> i32 {
    whisper_guard::params::num_cpus()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_streaming_whisper_has_empty_buffer() {
        let sw = StreamingWhisper::new(None);
        assert_eq!(sw.duration_secs(), 0.0);
        assert!(sw.audio_buffer.is_empty());
    }

    #[test]
    fn feed_below_interval_returns_none() {
        let mut sw = StreamingWhisper::new(None);
        // Feed 1 second of silence (below 2s interval)
        let silence = vec![0.0f32; 16000];
        // We can't test with a real WhisperContext without a model,
        // but we can verify the buffer grows correctly
        sw.audio_buffer.extend_from_slice(&silence);
        sw.samples_since_partial += silence.len();
        assert_eq!(sw.duration_secs(), 1.0);
        assert_eq!(sw.samples_since_partial, 16000);
    }

    #[test]
    fn reset_clears_state() {
        let mut sw = StreamingWhisper::new(Some("en".into()));
        sw.audio_buffer.extend_from_slice(&[0.0; 16000]);
        sw.samples_since_partial = 16000;
        sw.last_partial = "hello".into();

        sw.reset();

        assert!(sw.audio_buffer.is_empty());
        assert_eq!(sw.samples_since_partial, 0);
        assert!(sw.last_partial.is_empty());
        assert_eq!(sw.duration_secs(), 0.0);
    }

    #[test]
    fn partial_max_samples_is_set_from_secs() {
        let sw = StreamingWhisper::with_partial_max_secs(None, 45);
        assert_eq!(sw.partial_max_samples, 45 * 16000);

        let sw_default = StreamingWhisper::new(None);
        assert_eq!(
            sw_default.partial_max_samples,
            DEFAULT_PARTIAL_MAX_SECS as usize * 16000
        );
    }

    #[test]
    fn zero_partial_max_disables_cap() {
        let sw = StreamingWhisper::with_partial_max_secs(None, 0);
        assert_eq!(sw.partial_max_samples, 0);
        // A zero cap keeps the historical full-buffer partial behavior.
    }

    #[test]
    fn long_partial_uses_recent_bounded_window_but_final_uses_everything() {
        let mut sw = StreamingWhisper::with_partial_max_secs(None, 30);
        sw.audio_buffer.resize(45 * 16000, 0.0);

        assert_eq!(sw.transcription_window_start(false), 15 * 16000);
        assert_eq!(sw.transcription_window_start(true), 0);
    }

    #[test]
    fn recording_sidecar_abort_signal_is_explicit_and_shared() {
        let stop = Arc::new(AtomicBool::new(false));
        let sw = StreamingWhisper::new(None).with_abort_signal(Arc::clone(&stop));
        let configured = sw.abort_signal.expect("abort signal should be configured");

        assert!(!configured.load(Ordering::Relaxed));
        stop.store(true, Ordering::Relaxed);
        assert!(configured.load(Ordering::Relaxed));
    }
}
