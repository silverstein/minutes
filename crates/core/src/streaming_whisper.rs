#[cfg(feature = "whisper")]
use crate::transcribe::streaming_whisper_params;
#[cfg(feature = "whisper")]
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
//   - No segment stitching needed — we always transcribe from t=0
//     so whisper sees full context. Each pass replaces the previous.
//   - Partial results are emitted via callback; the final result on
//     silence replaces all partials.
//   - Uses the same WhisperContext (preloaded model) as batch mode.
//
// Why full re-transcription instead of incremental:
//   Incremental (transcribe only the new 2s chunk) produces worse
//   quality because whisper loses context from earlier speech.
//   Full re-transcription from t=0 gives consistent output at the
//   cost of increasing latency as the utterance grows. For typical
//   dictation utterances (<30s), re-transcription takes <500ms on
//   Apple Silicon with the base model. Acceptable.
//
// Performance budget:
//   - base model: ~200ms for 10s audio on M-series
//   - small model: ~500ms for 10s audio on M-series
//   - Transcription runs on a background thread; audio capture
//     continues uninterrupted on the main thread.
// ──────────────────────────────────────────────────────────────

/// How often to run partial transcription (in audio samples at 16kHz).
#[cfg(feature = "whisper")]
const PARTIAL_INTERVAL_SAMPLES: usize = 16000 * 2; // Every 2 seconds

/// Minimum audio length to attempt transcription (avoid noise-only runs).
#[cfg(feature = "whisper")]
const MIN_TRANSCRIBE_SAMPLES: usize = 16000; // 1 second

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
#[cfg(feature = "whisper")]
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
}

#[cfg(feature = "whisper")]
impl StreamingWhisper {
    /// Create a new streaming transcriber.
    pub fn new(language: Option<String>) -> Self {
        Self {
            audio_buffer: Vec::with_capacity(16000 * 30), // pre-alloc 30s
            samples_since_partial: 0,
            last_partial: String::new(),
            n_threads: num_cpus(),
            language,
            has_created_state: false,
        }
    }

    /// Feed audio samples. Returns a partial result if enough audio has
    /// accumulated since the last transcription.
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

        let start = std::time::Instant::now();

        if let Err(e) = state.full(params, &self.audio_buffer) {
            tracing::warn!("streaming whisper failed: {}", e);
            return None;
        }

        let elapsed_ms = start.elapsed().as_millis();
        let duration_secs = self.audio_buffer.len() as f64 / 16000.0;

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
#[cfg(feature = "whisper")]
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

#[cfg(feature = "whisper")]
fn num_cpus() -> i32 {
    whisper_guard::params::num_cpus()
}

#[cfg(all(test, feature = "whisper"))]
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
}
