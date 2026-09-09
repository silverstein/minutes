//! Whisper parameter presets for stable transcription.
//!
//! These presets match whisper.cpp CLI defaults and include integrated
//! Silero VAD support. Without these, whisper-rs with default `Greedy { best_of: 1 }`
//! can loop indefinitely on non-English or noisy audio.

/// Build whisper FullParams with sane defaults matching whisper.cpp CLI.
///
/// Uses `best_of=5`, entropy/logprob thresholds, and temperature fallback
/// to prevent decoder loops. When a Silero VAD model path is provided,
/// enables integrated VAD so whisper only transcribes speech segments.
///
/// Use this for batch transcription. For latency-sensitive streaming,
/// use [`streaming_whisper_params`] instead.
pub fn default_whisper_params<'a, 'b>(
    vad_model_path: Option<&str>,
) -> whisper_rs::FullParams<'a, 'b> {
    let mut params =
        whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 5 });

    // Match whisper.cpp CLI defaults for stable decoding
    params.set_temperature(0.0);
    params.set_temperature_inc(0.2); // retry at higher temp on high-entropy segments
    params.set_entropy_thold(2.4); // flag segments with entropy above this
    params.set_logprob_thold(-1.0); // flag segments with avg logprob below this
    params.set_no_speech_thold(0.6); // probability threshold for silence detection
    params.set_suppress_blank(true); // suppress blank/repeated token hallucinations

    // Enable Silero VAD if model is available
    if let Some(path) = vad_model_path {
        params.set_vad_model_path(Some(path));
        params.enable_vad(true);
        params.set_vad_params(whisper_rs::WhisperVadParams::default());
        tracing::info!("Silero VAD enabled for transcription");
    }

    // Suppress noisy output
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params
}

/// Lighter whisper params for streaming/dictation where latency matters.
///
/// Keeps `best_of=1` and disables temperature fallback to stay within
/// the ~200ms (base) / ~500ms (small) budget for partial transcription.
/// Still sets entropy/logprob/no-speech thresholds and suppress_blank
/// to catch the worst hallucinations without the 5x cost of best_of=5.
pub fn streaming_whisper_params<'a, 'b>() -> whisper_rs::FullParams<'a, 'b> {
    let mut params =
        whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });

    params.set_temperature(0.0);
    params.set_temperature_inc(0.0); // no retry — latency budget too tight
    params.set_entropy_thold(2.4);
    params.set_logprob_thold(-1.0);
    params.set_no_speech_thold(0.6);
    params.set_suppress_blank(true);

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params
}

/// Get number of CPU threads to use for whisper.
/// Caps at 8 — diminishing returns beyond that.
pub fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|p| p.get() as i32)
        .unwrap_or(4)
        .min(8)
}

/// Install an abort callback on `params` that polls `callback` from whisper's
/// compute threads. Returning `true` aborts the in-flight pass.
///
/// This exists because whisper-rs 0.16's `FullParams::set_abort_callback_safe`
/// is unsound. It boxes the closure twice, hands whisper the address of the
/// outer `Box<Box<dyn FnMut>>`, and then has its trampoline reinterpret that
/// fat pointer as the caller's concrete closure type. A capturing closure
/// therefore reads the bytes next to the box instead of its own captures. A
/// closure that captures an `Arc<AtomicBool>` ends up loading the flag from
/// garbage and returns `true` on almost every call, so ggml aborts the
/// encoder before the first token and `full()` fails with "failed to encode"
/// (whisper.cpp return code -6). Closures whose garbage happens to read as
/// `false` work by accident, which is why the batch deadline never fired and
/// never failed either.
///
/// The caller keeps `callback` alive until `WhisperState::full` returns. The
/// `'a` borrow ties `params` to that lifetime so the compiler enforces it.
pub fn set_abort_callback<'a, 'b, F>(params: &mut whisper_rs::FullParams<'a, 'b>, callback: &'a F)
where
    F: Fn() -> bool + Sync,
{
    // SAFETY: the trampoline matches `ggml_abort_callback`'s ABI and reads
    // `callback` back through the user-data pointer installed alongside it.
    // Both stay valid for `'a`, which covers every use of `params`.
    unsafe {
        params.set_abort_callback(Some(abort_trampoline::<F>));
        params.set_abort_callback_user_data(callback as *const F as *mut std::ffi::c_void);
    }
}

/// C-ABI shim whisper calls with the pointer stored by [`set_abort_callback`].
///
/// # Safety
/// `user_data` must be the `&F` installed by [`set_abort_callback`] and must
/// still be alive.
unsafe extern "C" fn abort_trampoline<F: Fn() -> bool>(user_data: *mut std::ffi::c_void) -> bool {
    let callback = &*(user_data as *const F);
    callback()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn call<F: Fn() -> bool>(callback: &F) -> bool {
        // SAFETY: `callback` outlives the call and matches the trampoline's `F`.
        unsafe { abort_trampoline::<F>(callback as *const F as *mut c_void) }
    }

    #[test]
    fn trampoline_reads_a_capturing_closure_through_its_own_pointer() {
        // The exact shape that broke the recording sidecar: a closure whose
        // only capture is an `Arc<AtomicBool>` stop flag.
        let flag = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&flag);
        let callback = move || shared.load(Ordering::Relaxed);

        assert!(!call(&callback), "must not abort while the flag is clear");
        flag.store(true, Ordering::Relaxed);
        assert!(call(&callback), "must abort once the flag is set");
    }

    #[test]
    fn trampoline_sees_state_captured_by_value() {
        // Batch transcription captures a deadline by value; the helper must
        // read the real capture, not whatever sits next to a heap box.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        let not_yet = move || std::time::Instant::now() > deadline;
        assert!(!call(&not_yet));

        let past = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let already = move || std::time::Instant::now() > past;
        assert!(call(&already));
    }

    #[test]
    fn install_on_params_compiles_with_a_stack_callback() {
        // Compile-time check that the borrow lets the callback live on the
        // caller's stack for the duration of a `full()` call.
        let stop = AtomicBool::new(false);
        let callback = || stop.load(Ordering::Relaxed);
        let mut params = streaming_whisper_params();
        set_abort_callback(&mut params, &callback);
        drop(params);
    }
}
