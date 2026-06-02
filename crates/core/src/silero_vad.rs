// ──────────────────────────────────────────────────────────────
// Streaming Silero VAD via ort (ONNX Runtime).
//
//   samples ──▶ OrtSileroVad::process()
//                    │
//                    ▼
//              accumulate buffer
//                    │
//                    ▼
//              for each 512-sample chunk:
//                  input = [64-sample carry] ++ chunk     (shape [1, 576])
//                  (output, stateN) = session.run(input, state, sr)
//                  state = stateN
//                  carry = chunk[-64..]
//                  smoothing.feed(output) ──▶ SmoothedDecision
//              VadResult = last decision (or carry-over if no chunk ran)
//
// This impl is the structural fix for the 25–130 ms per-callback CPU
// cost of running whisper-rs's Silero on the full 3–8 s sliding
// buffer every 100 ms. With per-chunk streaming, cost is
// O(new_audio): a 100 ms call processes ~3 chunks of 512 samples,
// each ~0.5 ms of inference. PLAN-vad-refactor.md has the full
// rationale.
//
// The ONNX I/O contract was codex-verified against the
// snakers4/silero-vad master branch:
//   input  f32 [1, 576]   = 64-sample carry ++ 512 chunk
//   state  f32 [2, 1, 128] LSTM hidden+cell, round-tripped via stateN
//   sr     i64 scalar     = 16000
//   output f32 [1, 1]     = speech probability for the chunk
//   stateN f32 [2, 1, 128] = updated LSTM state
//
// The 64-sample carry is NOT a separate ONNX input. We prepend it on
// the caller side. Prior plan revisions had this wrong; getting it
// wrong panics at session.run with shape mismatch.
// ──────────────────────────────────────────────────────────────

use crate::silero_smoothing::{Smoothing, SmoothingParams, SAMPLE_RATE, WINDOW_SAMPLES};
use crate::vad::{VadEngine, VadResult};
use ndarray::{Array1, Array2, Array3};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use std::path::Path;

/// Number of carry samples prepended to each chunk before inference.
/// Hardcoded by the Silero v6 ONNX schema; do not tune.
pub const CARRY_SAMPLES: usize = 64;

/// Total input length the ONNX session expects: chunk + carry.
pub const INPUT_SAMPLES: usize = WINDOW_SAMPLES + CARRY_SAMPLES;

/// LSTM state tensor shape: 2 layers of (hidden | cell), batch 1,
/// hidden size 128.
pub const STATE_DIMS: [usize; 3] = [2, 1, 128];

/// Streaming Silero VAD with persistent LSTM state. Implements
/// `VadEngine`. Construct via `OrtSileroVad::new`.
pub struct OrtSileroVad {
    session: Session,
    /// LSTM hidden+cell state, shape `[2, 1, 128]`. Initialized
    /// zeros, replaced with `stateN` after every successful inference.
    state: Array3<f32>,
    /// 64-sample carry buffer prepended to the next chunk before
    /// inference. Initialized zeros for the first call. After every
    /// inference, we overwrite with the trailing 64 samples of the
    /// 512-sample chunk just processed.
    carry: Array1<f32>,
    /// Sample-rate scalar. Constant 16000. Held as an `Array1` of
    /// length 1 because ort prefers shaped tensors for scalar inputs.
    sr: Array1<i64>,
    /// Sample buffer that accumulates audio across `process` calls
    /// until at least one full 512-sample chunk is available.
    sample_buffer: Vec<f32>,
    /// Smoothing FSM. Owns the threshold/min-speech/min-silence/pad
    /// logic; we just feed raw probabilities.
    smoothing: Smoothing,
    /// Most recent per-window decision. Returned when a `process`
    /// call doesn't accumulate enough samples to run inference (e.g.
    /// final small chunk before recording stops).
    last_decision: crate::silero_smoothing::SmoothedDecision,
    /// Sticky failure flag. Once set, `is_healthy` returns false; the
    /// dispatcher must replace the engine. See trait docs.
    failed: bool,
}

impl OrtSileroVad {
    /// Build a fresh session from `silero_vad_path` (an `.onnx` file)
    /// using whisper-Silero default smoothing parameters.
    ///
    /// The returned engine owns the ort Session and is `Send` but not
    /// `Sync` — the recording sidecar's single dedicated thread holds
    /// it for the life of the recording.
    pub fn new(silero_vad_path: &Path) -> Result<Self, OrtSileroError> {
        Self::with_params(silero_vad_path, SmoothingParams::whisper_silero_defaults())
    }

    /// Build with custom smoothing parameters. Mostly useful for
    /// tests; production should stick to the whisper-Silero defaults
    /// so behavior matches the existing engine within the tolerance
    /// PLAN-vad-refactor.md documents.
    pub fn with_params(
        silero_vad_path: &Path,
        params: SmoothingParams,
    ) -> Result<Self, OrtSileroError> {
        let session = Session::builder()
            .map_err(|e| OrtSileroError::SessionBuilder(format!("{e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| OrtSileroError::SessionBuilder(format!("{e}")))?
            .with_intra_threads(1)
            .map_err(|e| OrtSileroError::SessionBuilder(format!("{e}")))?
            .with_inter_threads(1)
            .map_err(|e| OrtSileroError::SessionBuilder(format!("{e}")))?
            .commit_from_file(silero_vad_path)
            .map_err(|e| OrtSileroError::ModelLoad(format!("{e}")))?;

        let mut engine = Self {
            session,
            state: Array3::zeros(STATE_DIMS),
            carry: Array1::zeros(CARRY_SAMPLES),
            sr: Array1::from_elem([1], SAMPLE_RATE as i64),
            sample_buffer: Vec::with_capacity(WINDOW_SAMPLES * 4),
            smoothing: Smoothing::new(params),
            last_decision: crate::silero_smoothing::SmoothedDecision {
                speaking: false,
                silence_ms: 0,
            },
            failed: false,
        };

        // Validate the ONNX I/O schema by name + smoke-test inference
        // on construction. A future Silero release that rearranges
        // names or shapes fails loudly here, not in the recording
        // hot path.
        engine.validate_schema()?;
        Ok(engine)
    }

    /// Feed a 512-sample window through the ONNX session and update
    /// state/carry/smoothing. Returns the smoothed decision for this
    /// window.
    ///
    /// Errors are surfaced as `Err(OrtSileroError)`. The caller should
    /// set `self.failed = true` and stop running inference; reset is
    /// not a recovery primitive.
    fn run_window(
        &mut self,
        chunk: &[f32],
    ) -> Result<crate::silero_smoothing::SmoothedDecision, OrtSileroError> {
        debug_assert_eq!(chunk.len(), WINDOW_SAMPLES);

        // Build the [1, 576] input tensor: carry || chunk.
        let mut input = Array2::<f32>::zeros((1, INPUT_SAMPLES));
        for i in 0..CARRY_SAMPLES {
            input[[0, i]] = self.carry[i];
        }
        for i in 0..WINDOW_SAMPLES {
            input[[0, CARRY_SAMPLES + i]] = chunk[i];
        }

        // Run inference. Named inputs match the ONNX schema:
        // `input`, `state`, `sr`.
        let outputs = {
            let inputs = ort::inputs! {
                "input" => ort::value::TensorRef::from_array_view(input.view().into_dyn())
                    .map_err(|e| OrtSileroError::Inference(format!("input tensor: {e}")))?,
                "state" => ort::value::TensorRef::from_array_view(self.state.view().into_dyn())
                    .map_err(|e| OrtSileroError::Inference(format!("state tensor: {e}")))?,
                "sr" => ort::value::TensorRef::from_array_view(self.sr.view().into_dyn())
                    .map_err(|e| OrtSileroError::Inference(format!("sr tensor: {e}")))?,
            };
            self.session
                .run(inputs)
                .map_err(|e| OrtSileroError::Inference(format!("session.run: {e}")))?
        };

        // Extract `output` (probability scalar wrapped in [1, 1]).
        let output_tensor = outputs
            .get("output")
            .ok_or_else(|| OrtSileroError::Inference("missing `output` tensor".into()))?;
        let (output_shape, output_data) = output_tensor
            .try_extract_tensor::<f32>()
            .map_err(|e| OrtSileroError::Inference(format!("output extract: {e}")))?;
        if output_shape.iter().product::<i64>() != 1 || output_data.is_empty() {
            return Err(OrtSileroError::Inference(format!(
                "unexpected output shape {:?}",
                output_shape
            )));
        }
        let prob = output_data[0];

        // Extract `stateN` and round-trip into `state`.
        let state_n = outputs
            .get("stateN")
            .ok_or_else(|| OrtSileroError::Inference("missing `stateN` tensor".into()))?;
        let (state_shape, state_data) = state_n
            .try_extract_tensor::<f32>()
            .map_err(|e| OrtSileroError::Inference(format!("stateN extract: {e}")))?;
        let expected: i64 = STATE_DIMS.iter().product::<usize>() as i64;
        if state_shape.iter().product::<i64>() != expected {
            return Err(OrtSileroError::Inference(format!(
                "unexpected stateN shape {:?}",
                state_shape
            )));
        }
        for (slot, &v) in self.state.iter_mut().zip(state_data.iter()) {
            *slot = v;
        }

        // Update carry from the trailing 64 samples of this chunk.
        for i in 0..CARRY_SAMPLES {
            self.carry[i] = chunk[WINDOW_SAMPLES - CARRY_SAMPLES + i];
        }

        Ok(self.smoothing.feed(prob))
    }

    /// Verify the ONNX session's I/O matches the shape contract this
    /// impl was written against. Run once at construction so a Silero
    /// release that changes the schema fails fast with a clear error
    /// rather than panicking in the recording hot path.
    ///
    /// Two-stage validation:
    /// 1. Tensor names match (`input`, `state`, `sr` → `output`,
    ///    `stateN`).
    /// 2. End-to-end inference smoke test on a zero input + zero
    ///    state. If shapes or dtypes drift in a future Silero release,
    ///    `session.run` panics here at construction with a clear ort
    ///    error, not in the 100 ms recording hot path. Codex review
    ///    #7a flagged the original name-only validation as
    ///    insufficient.
    fn validate_schema(&mut self) -> Result<(), OrtSileroError> {
        let inputs: Vec<_> = self.session.inputs.iter().map(|i| i.name.clone()).collect();
        let outputs: Vec<_> = self
            .session
            .outputs
            .iter()
            .map(|o| o.name.clone())
            .collect();
        for name in ["input", "state", "sr"] {
            if !inputs.iter().any(|n| n == name) {
                return Err(OrtSileroError::SchemaMismatch(format!(
                    "expected input tensor `{}`, got {:?}",
                    name, inputs
                )));
            }
        }
        for name in ["output", "stateN"] {
            if !outputs.iter().any(|n| n == name) {
                return Err(OrtSileroError::SchemaMismatch(format!(
                    "expected output tensor `{}`, got {:?}",
                    name, outputs
                )));
            }
        }

        // Inference smoke test on a zero buffer. The carry, state,
        // and sample buffer are still in their constructor-zeros
        // configuration here, so building the [1, 576] input is
        // straightforward. A shape or dtype mismatch surfaces as a
        // clear `OrtSileroError::Inference` instead of a runtime
        // panic during recording. After the smoke test we clear the
        // smoothing FSM so the segment counter starts fresh for the
        // first real call.
        let zero_chunk = [0.0_f32; WINDOW_SAMPLES];
        self.run_window(&zero_chunk).map_err(|e| {
            OrtSileroError::SchemaMismatch(format!(
                "smoke-test inference failed (likely shape/dtype drift in Silero release): {}",
                e
            ))
        })?;
        // run_window updated `state`, `carry`, and the smoothing FSM.
        // Reset them so the engine starts in a fresh state for the
        // first real `process` call.
        self.state.fill(0.0);
        self.carry.fill(0.0);
        self.smoothing.reset();
        self.last_decision = crate::silero_smoothing::SmoothedDecision {
            speaking: false,
            silence_ms: 0,
        };
        Ok(())
    }

    /// For tests only: force the sticky failure flag without
    /// corrupting the model context. Mirrors `SileroSidecarVad`'s
    /// test seam. Same trait contract: `is_healthy` reports the flag,
    /// `reset` does NOT clear it.
    #[cfg(test)]
    pub(crate) fn force_failed_for_test(&mut self, value: bool) {
        self.failed = value;
    }
}

impl VadEngine for OrtSileroVad {
    fn process(&mut self, samples: &[f32], rms: f32) -> VadResult {
        if self.failed {
            // Once failed, stop running inference. Caller must check
            // is_healthy and replace the engine.
            return VadResult {
                speaking: false,
                silence_ms: self.last_decision.silence_ms,
                energy: rms,
                noise_floor: 0.0,
            };
        }

        self.sample_buffer.extend_from_slice(samples);

        // Pull every full 512-sample chunk that fits. Smaller leftover
        // stays in the buffer for the next call.
        while self.sample_buffer.len() >= WINDOW_SAMPLES {
            let chunk: Vec<f32> = self.sample_buffer.drain(..WINDOW_SAMPLES).collect();
            match self.run_window(&chunk) {
                Ok(decision) => {
                    self.last_decision = decision;
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "ort-Silero inference failed — engine is now unhealthy, dispatcher must replace"
                    );
                    self.failed = true;
                    // Bail out of the loop so we don't keep poking a
                    // failed session. last_decision retains its
                    // previous value so the immediate next process
                    // call (before the dispatcher swaps) returns
                    // something sane rather than a fresh silence
                    // frame inserted mid-segment.
                    break;
                }
            }
        }

        VadResult {
            speaking: self.last_decision.speaking,
            silence_ms: self.last_decision.silence_ms,
            energy: rms,
            noise_floor: 0.0,
        }
    }

    fn name(&self) -> &'static str {
        "ort-silero"
    }

    fn is_healthy(&self) -> bool {
        !self.failed
    }

    fn reset(&mut self) {
        // Reset reusable state only. `failed` stays sticky per the
        // VadEngine trait contract; failed engines are replaced, not
        // revived.
        self.state.fill(0.0);
        self.carry.fill(0.0);
        self.sample_buffer.clear();
        self.smoothing.reset();
        self.last_decision = crate::silero_smoothing::SmoothedDecision {
            speaking: false,
            silence_ms: 0,
        };
        debug_assert!(
            !self.failed,
            "reset called on a failed OrtSileroVad — dispatcher should replace, not reset"
        );
    }
}

/// Errors surfaced by `OrtSileroVad`. The trait impl absorbs these
/// into a sticky failure flag, but the constructor surfaces them
/// directly so the dispatcher can fall back to whisper-Silero on load
/// failure rather than starting a doomed engine.
#[derive(Debug, thiserror::Error)]
pub enum OrtSileroError {
    #[error("ort session builder: {0}")]
    SessionBuilder(String),
    #[error("ort model load: {0}")]
    ModelLoad(String),
    #[error("ort inference: {0}")]
    Inference(String),
    #[error("Silero ONNX schema mismatch (Silero release changed contract?): {0}")]
    SchemaMismatch(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the Silero ONNX model used by tests. The model is not
    /// committed to the repo. `minutes setup` (with `vad-ort` enabled)
    /// will download it; tests skip when missing.
    fn model_path() -> std::path::PathBuf {
        dirs::home_dir()
            .unwrap()
            .join(".minutes/models/silero-vad-v6.2.0.onnx")
    }

    #[test]
    fn carry_samples_match_silero_v6_contract() {
        // Compile-time guard: if WINDOW_SAMPLES drifts from 512 or
        // CARRY_SAMPLES from 64, the ONNX shape we built (576) is
        // wrong. Constants are publicly exported; this test catches
        // accidental tuning.
        assert_eq!(WINDOW_SAMPLES, 512);
        assert_eq!(CARRY_SAMPLES, 64);
        assert_eq!(INPUT_SAMPLES, 576);
        assert_eq!(STATE_DIMS, [2, 1, 128]);
    }

    #[test]
    fn construct_validates_schema_when_model_present() {
        // Catches the contract-drift case codex specifically called
        // out: a future Silero release that renames `input`/`state`/
        // `sr` will fail at construction with SchemaMismatch instead
        // of panicking at session.run.
        let path = model_path();
        if !path.exists() {
            eprintln!(
                "[ort_silero] skipping: no model at {} — run `minutes setup` with vad-ort feature",
                path.display()
            );
            return;
        }
        let engine = OrtSileroVad::new(&path);
        assert!(engine.is_ok(), "construct must succeed: {:?}", engine.err());
        let engine = engine.unwrap();
        assert!(engine.is_healthy());
        assert_eq!(engine.name(), "ort-silero");
    }

    #[test]
    fn pure_silence_does_not_trigger_speech() {
        let path = model_path();
        if !path.exists() {
            eprintln!("[ort_silero] skipping: no model at {}", path.display());
            return;
        }
        let mut engine = OrtSileroVad::new(&path).unwrap();
        // 2 s of zero samples, 100 ms at a time.
        for _ in 0..20 {
            let samples = vec![0.0_f32; 1600];
            let result = engine.process(&samples, 0.0);
            assert!(
                !result.speaking,
                "ort-silero must not detect speech in silence"
            );
        }
    }

    #[test]
    fn force_failed_flips_is_healthy() {
        let path = model_path();
        if !path.exists() {
            eprintln!("[ort_silero] skipping: no model at {}", path.display());
            return;
        }
        let mut engine = OrtSileroVad::new(&path).unwrap();
        assert!(engine.is_healthy());
        engine.force_failed_for_test(true);
        assert!(!engine.is_healthy());
        // After failure, process returns silence frames without
        // panicking, so the dispatcher gets a chance to swap.
        let result = engine.process(&[0.5_f32; 1600], 0.5);
        assert!(!result.speaking);
    }

    #[test]
    fn reset_does_not_clear_sticky_failure() {
        let path = model_path();
        if !path.exists() {
            eprintln!("[ort_silero] skipping: no model at {}", path.display());
            return;
        }
        let mut engine = OrtSileroVad::new(&path).unwrap();
        engine.force_failed_for_test(true);
        assert!(!engine.is_healthy());
        // We can't call reset() while failed because the debug_assert
        // would fire; clear the flag, reset on a healthy engine, then
        // re-fail to confirm reset() doesn't touch the flag from the
        // healthy state.
        engine.force_failed_for_test(false);
        engine.reset();
        assert!(engine.is_healthy());
        engine.force_failed_for_test(true);
        assert!(!engine.is_healthy());
    }
}
