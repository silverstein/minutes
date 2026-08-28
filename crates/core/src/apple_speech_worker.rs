//! Exact-capability boundary for private Apple Speech utterances.
//!
//! The desktop process sends bounded 16 kHz mono `f32` bytes only after an
//! embedded one-shot XPC service passes reciprocal code-signing checks. The
//! service never accepts an executable, pathname, environment, or attachment,
//! and the Swift bridge constructs `AVAudioPCMBuffer` directly from the
//! authenticated byte stream.

#[cfg(target_os = "macos")]
use crate::apple_speech::{AppleSpeechMode, AppleSpeechTranscriptionResult};
#[cfg(target_os = "macos")]
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
const REQUEST_MAGIC: &[u8; 8] = b"MASPCM1\0";
#[cfg(target_os = "macos")]
const REQUEST_SCHEMA_VERSION: u32 = 1;
#[cfg(any(test, target_os = "macos"))]
const SAMPLE_RATE_HZ: u32 = 16_000;
#[cfg(any(test, target_os = "macos"))]
const MAX_UTTERANCE_SECONDS: usize = 10 * 60;
#[cfg(any(test, target_os = "macos"))]
pub(crate) const MAX_REQUEST_BYTES: usize =
    8 + 4 + 4_096 + (SAMPLE_RATE_HZ as usize * MAX_UTTERANCE_SECONDS * size_of::<f32>());
#[cfg(target_os = "macos")]
pub(crate) const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(target_os = "macos")]
const WORKER_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(180);
#[cfg(target_os = "macos")]
const WORKER_ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "macos")]
struct PrivateAudioRequest {
    schema_version: u32,
    mode: AppleSpeechMode,
    locale: String,
    ensure_assets: bool,
    sample_rate_hz: u32,
    sample_count: usize,
    // When set, the worker verifies the exact bytes arrived and returns a
    // transport receipt without invoking the Speech bridge. A hosted CI runner
    // has no Speech assets and cannot transcribe, and the analyzer construction
    // aborts the sandboxed XPC worker before it can answer, so acceptance
    // proves the authenticated byte transport instead of the analyzer, which is
    // separately verified on real hardware. Skipped on the wire when false so
    // the product path's metadata stays byte-identical.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    transport_acceptance_echo: bool,
}

/// Content receipt for the transport-acceptance path. Proves that the exact
/// authenticated bytes reached the worker without running the analyzer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "macos")]
pub(crate) struct TransportAcceptanceReceipt {
    pub kind: String,
    pub schema_version: u32,
    pub sample_count: usize,
    pub checksum: String,
    pub runtime_supported: bool,
}

/// Content-free evidence from the fixed public-fixture runtime acceptance.
/// The transcript itself never leaves the parent process.
#[derive(Debug)]
#[cfg(target_os = "macos")]
pub struct SignedRuntimeAcceptanceReceipt {
    pub module_id: String,
    pub word_count: usize,
    pub segment_count: usize,
    pub first_result_elapsed_ms: Option<u64>,
    pub total_elapsed_ms: u64,
}

/// FNV-1a over the little-endian sample bytes. Proves content integrity, not
/// just that the declared byte structure arrived, which parse already checks.
#[cfg(any(test, target_os = "macos"))]
pub(crate) fn transport_acceptance_checksum(samples: &[f32]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for sample in samples {
        for byte in sample.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[cfg(target_os = "macos")]
const EMBEDDED_APPLE_SPEECH_WORKER_CDHASH_PREFIX: &[u8] = b"MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=";
#[cfg(target_os = "macos")]
#[used]
static EMBEDDED_APPLE_SPEECH_WORKER_CDHASH: &[u8] =
    b"MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=0000000000000000000000000000000000000000";

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct MacAppleSpeechWorkerAuthority {
    bundle: std::path::PathBuf,
    cdhash: [u8; 20],
    trusted_distribution: bool,
}

#[cfg(target_os = "macos")]
static APPLE_SPEECH_WORKER_AUTHORITY: std::sync::OnceLock<MacAppleSpeechWorkerAuthority> =
    std::sync::OnceLock::new();

/// Bind the signed embedded service before normal desktop threads start.
#[cfg(target_os = "macos")]
pub fn install_apple_speech_worker_service(
    service_bundle: std::path::PathBuf,
) -> Result<(), String> {
    if !crate::macos_graph_xpc::current_process_is_trusted_distribution() {
        return Ok(());
    }
    let authority = bind_authority(service_bundle)?;
    APPLE_SPEECH_WORKER_AUTHORITY
        .set(authority)
        .map_err(|_| "Apple Speech worker authority was already installed".to_string())
}

// The product dictation/live callers sit behind their own feature gates, and
// the signed-acceptance path now uses the echo probe instead of this. It is
// live product code under those feature sets and unused in lean builds, so
// allow dead_code rather than gate it to a brittle feature combination.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub(crate) fn transcribe_samples(
    samples: &[f32],
    locale: Option<&str>,
    mode: AppleSpeechMode,
    ensure_assets: bool,
) -> crate::error::Result<AppleSpeechTranscriptionResult> {
    use zeroize::Zeroizing;

    if samples.is_empty()
        || samples.len() > SAMPLE_RATE_HZ as usize * MAX_UTTERANCE_SECONDS
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(crate::error::MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Apple Speech private audio exceeded its finite utterance budget",
        )));
    }
    let metadata = PrivateAudioRequest {
        schema_version: REQUEST_SCHEMA_VERSION,
        mode,
        locale: locale.unwrap_or("en-US").to_string(),
        ensure_assets,
        sample_rate_hz: SAMPLE_RATE_HZ,
        sample_count: samples.len(),
        transport_acceptance_echo: false,
    };
    let metadata = serde_json::to_vec(&metadata).map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    if metadata.len() > 4_096 {
        return Err(crate::error::MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Apple Speech request metadata exceeded its byte budget",
        )));
    }
    let capacity = REQUEST_MAGIC
        .len()
        .checked_add(4)
        .and_then(|value| value.checked_add(metadata.len()))
        .and_then(|value| value.checked_add(std::mem::size_of_val(samples)))
        .filter(|value| *value <= MAX_REQUEST_BYTES)
        .ok_or_else(|| {
            crate::error::MinutesError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Apple Speech private audio exceeded its byte budget",
            ))
        })?;
    let mut request = Zeroizing::new(Vec::with_capacity(capacity));
    request.extend_from_slice(REQUEST_MAGIC);
    request.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    request.extend_from_slice(&metadata);
    for sample in samples {
        request.extend_from_slice(&sample.to_le_bytes());
    }

    let authority = authority().map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            error,
        ))
    })?;
    let response = crate::macos_graph_xpc::run_apple_speech(
        &authority.bundle,
        &authority.cdhash,
        authority.trusted_distribution,
        std::io::Cursor::new(request.as_slice()),
        MAX_RESPONSE_BYTES,
        WORKER_WALL_CLOCK,
    )
    .map_err(|error| crate::error::MinutesError::Io(std::io::Error::other(error)))?;
    serde_json::from_slice(&response).map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// Send the fixed canary through the exact authenticated parent -> XPC ->
/// worker path with the acceptance-echo flag set, so the worker returns a
/// content receipt without invoking the Speech bridge. Everything the signed
/// acceptance must prove on CI happens here: the authenticated transport, the
/// exact bytes, and no named plaintext file.
#[cfg(target_os = "macos")]
pub(crate) fn transport_acceptance_probe(
    samples: &[f32],
) -> crate::error::Result<TransportAcceptanceReceipt> {
    use zeroize::Zeroizing;

    let metadata = PrivateAudioRequest {
        schema_version: REQUEST_SCHEMA_VERSION,
        mode: AppleSpeechMode::Dictation,
        locale: "en-US".to_string(),
        ensure_assets: false,
        sample_rate_hz: SAMPLE_RATE_HZ,
        sample_count: samples.len(),
        transport_acceptance_echo: true,
    };
    let metadata = serde_json::to_vec(&metadata).map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    let mut request = Zeroizing::new(Vec::with_capacity(
        REQUEST_MAGIC.len() + 4 + metadata.len() + std::mem::size_of_val(samples),
    ));
    request.extend_from_slice(REQUEST_MAGIC);
    request.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    request.extend_from_slice(&metadata);
    for sample in samples {
        request.extend_from_slice(&sample.to_le_bytes());
    }
    let authority = authority().map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            error,
        ))
    })?;
    let response = crate::macos_graph_xpc::run_apple_speech(
        &authority.bundle,
        &authority.cdhash,
        authority.trusted_distribution,
        std::io::Cursor::new(request.as_slice()),
        MAX_RESPONSE_BYTES,
        WORKER_WALL_CLOCK,
    )
    .map_err(|error| crate::error::MinutesError::Io(std::io::Error::other(error)))?;
    serde_json::from_slice(&response).map_err(|error| {
        crate::error::MinutesError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// Exercise the exact signed parent -> authenticated XPC -> worker byte path
/// without making Apple Speech selectable by normal product configuration. The
/// input is a fixed synthetic canary and this entrypoint accepts no audio,
/// path, or executable from the caller.
///
/// The worker returns a content receipt proving the exact bytes crossed the
/// authenticated transport, and never constructs a Speech type. A hosted CI
/// runner has no Speech assets and cannot transcribe, and constructing the
/// analyzer aborts the sandboxed XPC worker (diagnosed across the run history
/// in the lane evidence, isolated to the launchd XPC-service context). The
/// analyzer itself is verified separately on real hardware via
/// `scripts/verify_apple_speech_hardware.sh`. Returns false because the
/// analyzer did not run; the receipt must not imply otherwise.
#[cfg(target_os = "macos")]
pub fn run_signed_transport_acceptance() -> Result<bool, String> {
    if crate::pipeline::apple_speech_private_audio_transport_supported() {
        return Err("Apple Speech product selection must remain closed during acceptance".into());
    }
    if !crate::macos_graph_xpc::current_process_is_trusted_distribution() {
        return Err("Apple Speech transport acceptance requires a trusted signed app".into());
    }
    // The worker installs its peer requirement from the same verdict, so a
    // trusted parent means the anchored form was used or the worker failed
    // closed. Recording it keeps a green run from hiding a downgrade.
    println!("apple-speech-signed-parent-trusted=true");

    let samples = acceptance_canary_samples();
    let expected_checksum = transport_acceptance_checksum(&samples);
    let receipt = transport_acceptance_probe(&samples)
        .map_err(|error| format!("signed Apple Speech byte transport failed: {error}"))?;
    if receipt.kind != "transport-acceptance"
        || receipt.schema_version != REQUEST_SCHEMA_VERSION
        || receipt.sample_count != samples.len()
        || receipt.checksum != expected_checksum
    {
        return Err(
            "signed Apple Speech transport receipt did not match the exact authenticated bytes"
                .into(),
        );
    }

    // Retained product configuration must still resolve through the same
    // always-compiled gate to Whisper.
    let mut config = crate::config::Config::default();
    config.live_transcript.backend = "apple-speech".to_string();
    if crate::pipeline::resolved_apple_speech_backend(config.effective_live_transcript_backend())
        != "whisper"
    {
        return Err("Apple Speech acceptance did not preserve the product Whisper fallback".into());
    }
    Ok(receipt.runtime_supported)
}

/// Exercise the exact signed parent -> authenticated XPC -> worker -> Apple
/// Speech analyzer path on real macOS hardware. The caller cannot supply audio,
/// a path, a locale, or any worker configuration: the input is the checked-in
/// public fixture embedded into the parent at compile time. Only content-free
/// metrics leave this function, and the normal product gate must remain closed.
#[cfg(target_os = "macos")]
pub fn run_signed_runtime_acceptance() -> Result<SignedRuntimeAcceptanceReceipt, String> {
    if crate::pipeline::apple_speech_private_audio_transport_supported() {
        return Err("Apple Speech product selection must remain closed during acceptance".into());
    }
    if !crate::macos_graph_xpc::current_process_is_trusted_distribution() {
        return Err("Apple Speech runtime acceptance requires a trusted signed app".into());
    }
    println!("apple-speech-signed-parent-trusted=true");

    let samples = runtime_acceptance_samples()?;
    let result = transcribe_samples(
        samples.as_slice(),
        Some("en-US"),
        AppleSpeechMode::Speech,
        true,
    )
    .map_err(|error| format!("signed Apple Speech worker runtime failed: {error}"))?;
    if result.kind != "transcription"
        || result.schema_version != REQUEST_SCHEMA_VERSION
        || result.module_id != "speech-transcriber"
        || !result.runtime_supported
        || result.error.is_some()
        || result.transcript.trim().is_empty()
        || result.word_count < 10
        || result.segments.is_empty()
        || result.first_result_elapsed_ms.is_none()
    {
        return Err("signed Apple Speech worker did not produce a valid bounded transcript".into());
    }

    let mut config = crate::config::Config::default();
    config.live_transcript.backend = "apple-speech".to_string();
    if crate::pipeline::resolved_apple_speech_backend(config.effective_live_transcript_backend())
        != "whisper"
    {
        return Err("Apple Speech acceptance did not preserve the product Whisper fallback".into());
    }

    Ok(SignedRuntimeAcceptanceReceipt {
        module_id: result.module_id,
        word_count: result.word_count,
        segment_count: result.segments.len(),
        first_result_elapsed_ms: result.first_result_elapsed_ms,
        total_elapsed_ms: result.total_elapsed_ms,
    })
}

#[cfg(target_os = "macos")]
fn runtime_acceptance_samples() -> Result<zeroize::Zeroizing<Vec<f32>>, String> {
    const FIXTURE: &[u8] = include_bytes!("../../assets/demo.wav");
    let mut reader = hound::WavReader::new(std::io::Cursor::new(FIXTURE))
        .map_err(|_| "embedded Apple Speech runtime fixture was invalid".to_string())?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != SAMPLE_RATE_HZ
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("embedded Apple Speech runtime fixture had an unexpected format".into());
    }
    let samples = reader
        .samples::<i16>()
        .map(|sample| {
            sample.map(|value| f32::from(value) / 32768.0).map_err(|_| {
                "embedded Apple Speech runtime fixture could not be decoded".to_string()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if samples.is_empty() || samples.len() > SAMPLE_RATE_HZ as usize * MAX_UTTERANCE_SECONDS {
        return Err("embedded Apple Speech runtime fixture exceeded its sample budget".into());
    }
    Ok(zeroize::Zeroizing::new(samples))
}

#[cfg(target_os = "macos")]
fn acceptance_canary_samples() -> Vec<f32> {
    (0..SAMPLE_RATE_HZ as usize)
        .map(|index| f32::from_bits(0x3e00_0000 + (index % 4_096) as u32))
        .collect()
}

#[cfg(target_os = "macos")]
fn authority() -> Result<MacAppleSpeechWorkerAuthority, String> {
    if let Some(authority) = APPLE_SPEECH_WORKER_AUTHORITY.get() {
        return Ok(authority.clone());
    }
    if !crate::macos_graph_xpc::current_process_is_trusted_distribution() {
        return Err("Apple Speech transport requires a trusted Minutes app".into());
    }
    if !crate::macos_graph_xpc::peer_requirement_api_available() {
        return Err("authenticated Apple Speech XPC is unavailable on this macOS version".into());
    }
    let executable = std::env::current_exe()
        .map_err(|_| "Apple Speech parent executable was unavailable".to_string())?;
    let contents = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(std::path::Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .ok_or_else(|| "Apple Speech transport requires an installed Minutes app".to_string())?;
    bind_authority(
        contents
            .join("XPCServices")
            .join("com.useminutes.apple-speech-worker.xpc"),
    )
}

#[cfg(target_os = "macos")]
fn bind_authority(
    service_bundle: std::path::PathBuf,
) -> Result<MacAppleSpeechWorkerAuthority, String> {
    if service_bundle.extension().and_then(|value| value.to_str()) != Some("xpc") {
        return Err("Apple Speech worker authority was not an XPC service".into());
    }
    let executable = service_bundle
        .join("Contents")
        .join("MacOS")
        .join("minutes-apple-speech-worker");
    if !executable.is_file() {
        return Err("Apple Speech XPC executable was unavailable".into());
    }
    let app_bundle = service_bundle
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "XPCServices"))
        .and_then(std::path::Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .and_then(std::path::Path::parent)
        .ok_or_else(|| "Apple Speech XPC service was not inside an application".to_string())?;
    let manifest = app_bundle
        .join("Contents")
        .join("Resources")
        .join("minutes-apple-speech-worker.cdhash");
    let encoded = std::fs::read_to_string(&manifest)
        .map_err(|_| "Apple Speech worker integrity manifest was unavailable".to_string())?;
    let encoded = encoded
        .strip_suffix('\n')
        .ok_or_else(|| "Apple Speech worker integrity manifest was malformed".to_string())?;
    let embedded = EMBEDDED_APPLE_SPEECH_WORKER_CDHASH
        .strip_prefix(EMBEDDED_APPLE_SPEECH_WORKER_CDHASH_PREFIX)
        .ok_or_else(|| "Apple Speech worker executable authority was malformed".to_string())?;
    let cdhash = verify_worker_cdhash_binding(encoded.as_bytes(), embedded)?;
    Ok(MacAppleSpeechWorkerAuthority {
        bundle: app_bundle.to_path_buf(),
        cdhash,
        trusted_distribution: true,
    })
}

#[cfg(target_os = "macos")]
fn verify_worker_cdhash_binding(manifest: &[u8], embedded: &[u8]) -> Result<[u8; 20], String> {
    let manifest = decode_worker_cdhash(
        manifest,
        "Apple Speech worker integrity manifest was malformed",
    )?;
    let embedded = decode_worker_cdhash(
        embedded,
        "Apple Speech worker executable authority was unavailable",
    )?;
    if manifest == embedded {
        Ok(manifest)
    } else {
        Err("Apple Speech worker authority did not match this application".into())
    }
}

#[cfg(target_os = "macos")]
fn decode_worker_cdhash(
    encoded: &[u8],
    malformed_message: &'static str,
) -> Result<[u8; 20], String> {
    if encoded.len() != 40
        || encoded.iter().all(|byte| *byte == b'0')
        || !encoded
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(malformed_message.into());
    }
    let mut cdhash = [0_u8; 20];
    for (index, byte) in cdhash.iter_mut().enumerate() {
        let pair = std::str::from_utf8(&encoded[index * 2..index * 2 + 2])
            .map_err(|_| malformed_message.to_string())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| malformed_message.to_string())?;
    }
    Ok(cdhash)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn minutes_apple_speech_transcribe_pcm(
        samples: *const f32,
        sample_count: usize,
        mode: *const std::ffi::c_char,
        locale: *const std::ffi::c_char,
        ensure_assets: i32,
        response_length: *mut usize,
    ) -> *mut u8;
    fn minutes_apple_speech_free_response(response: *mut u8, response_length: usize);
}

#[cfg(target_os = "macos")]
struct SwiftResponse {
    pointer: *mut u8,
    length: usize,
}

#[cfg(target_os = "macos")]
impl Drop for SwiftResponse {
    fn drop(&mut self) {
        unsafe { minutes_apple_speech_free_response(self.pointer, self.length) };
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct ParsedPrivateAudioRequest {
    metadata: PrivateAudioRequest,
    samples: zeroize::Zeroizing<Vec<f32>>,
}

#[cfg(target_os = "macos")]
fn parse_private_audio_request(input: &[u8]) -> Result<ParsedPrivateAudioRequest, String> {
    if input.len() < REQUEST_MAGIC.len() + 4 || &input[..REQUEST_MAGIC.len()] != REQUEST_MAGIC {
        return Err("Apple Speech request magic was invalid".into());
    }
    let metadata_length = u32::from_le_bytes(
        input[REQUEST_MAGIC.len()..REQUEST_MAGIC.len() + 4]
            .try_into()
            .map_err(|_| "Apple Speech metadata length was invalid")?,
    ) as usize;
    if metadata_length == 0 || metadata_length > 4_096 {
        return Err("Apple Speech metadata exceeded its byte budget".into());
    }
    let metadata_end = REQUEST_MAGIC
        .len()
        .checked_add(4)
        .and_then(|value| value.checked_add(metadata_length))
        .filter(|value| *value <= input.len())
        .ok_or_else(|| "Apple Speech metadata frame was truncated".to_string())?;
    let metadata: PrivateAudioRequest =
        serde_json::from_slice(&input[REQUEST_MAGIC.len() + 4..metadata_end])
            .map_err(|_| "Apple Speech metadata was invalid".to_string())?;
    if metadata.schema_version != REQUEST_SCHEMA_VERSION
        || metadata.sample_rate_hz != SAMPLE_RATE_HZ
        || metadata.sample_count == 0
        || metadata.sample_count > SAMPLE_RATE_HZ as usize * MAX_UTTERANCE_SECONDS
    {
        return Err("Apple Speech metadata violated its schema or sample budget".into());
    }
    let audio = &input[metadata_end..];
    if audio.len() != metadata.sample_count.saturating_mul(size_of::<f32>()) {
        return Err("Apple Speech audio length did not match its exact sample capability".into());
    }
    let mut samples = zeroize::Zeroizing::new(Vec::with_capacity(metadata.sample_count));
    for bytes in audio.chunks_exact(size_of::<f32>()) {
        let sample = f32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| "Apple Speech sample frame was invalid")?,
        );
        if !sample.is_finite() {
            return Err("Apple Speech audio contained a non-finite sample".into());
        }
        samples.push(sample);
    }
    Ok(ParsedPrivateAudioRequest { metadata, samples })
}

#[cfg(target_os = "macos")]
pub(crate) fn process_private_audio_request(input: &[u8]) -> Result<Vec<u8>, String> {
    let parsed = parse_private_audio_request(input)?;
    if parsed.metadata.transport_acceptance_echo {
        // The bytes are already fully validated by parse_private_audio_request
        // (magic, schema, exact length, finite). Return a content receipt and
        // never construct a Speech type: the analyzer cannot run on a hosted
        // runner and aborts this sandboxed XPC worker if constructed.
        let receipt = TransportAcceptanceReceipt {
            kind: "transport-acceptance".to_string(),
            schema_version: REQUEST_SCHEMA_VERSION,
            sample_count: parsed.samples.len(),
            checksum: transport_acceptance_checksum(&parsed.samples),
            runtime_supported: false,
        };
        return serde_json::to_vec(&receipt)
            .map_err(|_| "Apple Speech transport receipt could not be encoded".to_string());
    }
    let mode = std::ffi::CString::new(parsed.metadata.mode.as_helper_arg())
        .map_err(|_| "Apple Speech mode was invalid".to_string())?;
    let locale = std::ffi::CString::new(parsed.metadata.locale.as_str())
        .map_err(|_| "Apple Speech locale was invalid".to_string())?;
    let mut response_length = 0_usize;
    let response = unsafe {
        minutes_apple_speech_transcribe_pcm(
            parsed.samples.as_ptr(),
            parsed.samples.len(),
            mode.as_ptr(),
            locale.as_ptr(),
            i32::from(parsed.metadata.ensure_assets),
            &mut response_length,
        )
    };
    if response.is_null() || response_length == 0 || response_length > MAX_RESPONSE_BYTES as usize {
        return Err("Apple Speech bridge returned an invalid bounded response".into());
    }
    let response = SwiftResponse {
        pointer: response,
        length: response_length,
    };
    let bytes = unsafe { std::slice::from_raw_parts(response.pointer, response.length) };
    serde_json::from_slice::<AppleSpeechTranscriptionResult>(bytes)
        .map_err(|_| "Apple Speech bridge response was invalid".to_string())?;
    Ok(bytes.to_vec())
}

#[cfg(target_os = "macos")]
pub(crate) fn prepare_macos_apple_speech_xpc_worker() -> Result<(), String> {
    let process_count = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &process_count) } != 0 {
        return Err("Apple Speech worker could not install its no-descendants ceiling".into());
    }
    let address_space_limit = macos_virtual_size()?
        .checked_add(WORKER_ADDRESS_SPACE_BYTES)
        .ok_or_else(|| "Apple Speech worker address-space ceiling overflowed".to_string())?;
    let address_space = libc::rlimit {
        rlim_cur: address_space_limit,
        rlim_max: address_space_limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0 {
        return Err("Apple Speech worker could not install its address-space ceiling".into());
    }
    unsafe extern "C" {
        fn setitimer(
            which: libc::c_int,
            new_value: *const libc::itimerval,
            old_value: *mut libc::itimerval,
        ) -> libc::c_int;
    }
    let timer = libc::itimerval {
        it_interval: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        it_value: libc::timeval {
            tv_sec: WORKER_WALL_CLOCK.as_secs() as libc::time_t,
            tv_usec: 0,
        },
    };
    if unsafe { setitimer(libc::ITIMER_REAL, &timer, std::ptr::null_mut()) } != 0 {
        return Err("Apple Speech worker could not install its wall-clock ceiling".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_virtual_size() -> Result<u64, String> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{
        task_basic_info_64, task_info_t, TASK_BASIC_INFO_64, TASK_BASIC_INFO_64_COUNT,
    };
    use mach2::traps::mach_task_self;

    let mut info = task_basic_info_64::default();
    let mut count = TASK_BASIC_INFO_64_COUNT;
    let status = unsafe {
        task_info(
            mach_task_self(),
            TASK_BASIC_INFO_64,
            (&mut info as *mut task_basic_info_64).cast::<libc::c_int>() as task_info_t,
            &mut count,
        )
    };
    if status != KERN_SUCCESS || count != TASK_BASIC_INFO_64_COUNT {
        return Err("Apple Speech worker could not measure its baseline address space".into());
    }
    Ok(info.virtual_size)
}

#[cfg(target_os = "macos")]
pub fn run_xpc_service_main() -> ! {
    crate::macos_graph_xpc::run_apple_speech_service_main()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_budget_is_finite_and_below_one_hour() {
        assert!(MAX_UTTERANCE_SECONDS <= 10 * 60);
        assert!(MAX_REQUEST_BYTES < 64 * 1024 * 1024);
    }

    #[cfg(target_os = "macos")]
    fn framed_request(mut metadata: PrivateAudioRequest, samples: &[f32]) -> Vec<u8> {
        metadata.sample_count = samples.len();
        let metadata = serde_json::to_vec(&metadata).unwrap();
        let mut request = Vec::new();
        request.extend_from_slice(REQUEST_MAGIC);
        request.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        request.extend_from_slice(&metadata);
        for sample in samples {
            request.extend_from_slice(&sample.to_le_bytes());
        }
        request
    }

    #[cfg(target_os = "macos")]
    fn valid_metadata() -> PrivateAudioRequest {
        PrivateAudioRequest {
            schema_version: REQUEST_SCHEMA_VERSION,
            mode: AppleSpeechMode::Dictation,
            locale: "en-US".to_string(),
            ensure_assets: false,
            sample_rate_hz: SAMPLE_RATE_HZ,
            sample_count: 0,
            transport_acceptance_echo: false,
        }
    }

    #[test]
    fn transport_acceptance_checksum_is_deterministic_and_content_sensitive() {
        let a = [0.25_f32, -0.5, 0.75, 1.0];
        let b = [0.25_f32, -0.5, 0.75, 1.0];
        let c = [0.25_f32, -0.5, 0.75, 1.000_001];
        assert_eq!(
            transport_acceptance_checksum(&a),
            transport_acceptance_checksum(&b),
            "identical samples must hash identically"
        );
        assert_ne!(
            transport_acceptance_checksum(&a),
            transport_acceptance_checksum(&c),
            "a single changed sample must change the checksum"
        );
        assert_eq!(transport_acceptance_checksum(&a).len(), 16);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signed_runtime_fixture_is_bounded_canonical_pcm() {
        let samples = runtime_acceptance_samples().unwrap();
        assert_eq!(samples.len(), 170_013);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert!(samples.iter().any(|sample| sample.abs() > 0.01));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_audio_parser_accepts_only_exact_finite_pcm_cardinality() {
        let request = framed_request(valid_metadata(), &[0.25, -0.5, 0.75]);
        let parsed = parse_private_audio_request(&request).unwrap();
        assert_eq!(parsed.metadata.locale, "en-US");
        assert_eq!(parsed.samples.as_slice(), &[0.25, -0.5, 0.75]);

        let mut truncated = request.clone();
        truncated.pop();
        assert!(parse_private_audio_request(&truncated)
            .unwrap_err()
            .contains("exact sample capability"));

        let non_finite = framed_request(valid_metadata(), &[f32::NAN]);
        assert!(parse_private_audio_request(&non_finite)
            .unwrap_err()
            .contains("non-finite"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_audio_parser_rejects_magic_metadata_schema_and_budget_attacks() {
        let valid = framed_request(valid_metadata(), &[0.0]);

        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 0xff;
        assert!(parse_private_audio_request(&bad_magic)
            .unwrap_err()
            .contains("magic"));

        let mut zero_metadata = valid.clone();
        zero_metadata[REQUEST_MAGIC.len()..REQUEST_MAGIC.len() + 4]
            .copy_from_slice(&0_u32.to_le_bytes());
        assert!(parse_private_audio_request(&zero_metadata)
            .unwrap_err()
            .contains("metadata exceeded"));

        let mut bad_schema = valid_metadata();
        bad_schema.schema_version += 1;
        assert!(
            parse_private_audio_request(&framed_request(bad_schema, &[0.0]))
                .unwrap_err()
                .contains("schema or sample budget")
        );

        let mut bad_rate = valid_metadata();
        bad_rate.sample_rate_hz += 1;
        assert!(
            parse_private_audio_request(&framed_request(bad_rate, &[0.0]))
                .unwrap_err()
                .contains("schema or sample budget")
        );

        let oversized_metadata = PrivateAudioRequest {
            sample_count: SAMPLE_RATE_HZ as usize * MAX_UTTERANCE_SECONDS + 1,
            ..valid_metadata()
        };
        let metadata = serde_json::to_vec(&oversized_metadata).unwrap();
        let mut oversized = Vec::new();
        oversized.extend_from_slice(REQUEST_MAGIC);
        oversized.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        oversized.extend_from_slice(&metadata);
        assert!(parse_private_audio_request(&oversized)
            .unwrap_err()
            .contains("schema or sample budget"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn acceptance_canary_is_fixed_finite_and_one_second() {
        let samples = acceptance_canary_samples();
        assert_eq!(samples.len(), SAMPLE_RATE_HZ as usize);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert_eq!(samples[0].to_bits(), 0x3e00_0000);
        assert_eq!(samples[4_095].to_bits(), 0x3e00_0fff);
        assert_eq!(samples[4_096].to_bits(), 0x3e00_0000);
    }
}
