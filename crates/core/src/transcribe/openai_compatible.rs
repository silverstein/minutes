//! Explicit, batch-only uploads. No retries, redirects, or engine fallback.
use super::{
    Config, DecodeHints, FilterStats, TranscribeError, TranscribeResult, TranscriptSegment,
};
use crate::search::{read_authorized_meeting, AuthorizedMeetingSnapshot};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use zeroize::Zeroizing;

fn failed(message: &str) -> TranscribeError {
    TranscribeError::TranscriptionFailed(format!(
        "Remote batch transcription: {message}. Recording and stems are preserved; no retry or fallback was attempted"
    ))
}

fn endpoint(config: &Config) -> Result<(String, String, bool), TranscribeError> {
    let base = config
        .transcription
        .openai_compatible_base_url
        .trim()
        .trim_end_matches('/');
    let uri: ureq::http::Uri = base.parse().map_err(|_| failed("invalid base URL"))?;
    let host = uri
        .host()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| failed("base URL needs a host"))?;
    if uri.authority().is_some_and(|a| a.as_str().contains('@'))
        || uri.query().is_some()
        || base.contains('#')
    {
        return Err(failed(
            "base URL must not contain credentials, query, or fragment",
        ));
    }
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
    if uri.scheme_str() != Some("https") && !(local && uri.scheme_str() == Some("http")) {
        return Err(failed("use HTTPS, or HTTP on loopback for a local server"));
    }
    let url = if base.ends_with("/audio/transcriptions") {
        base.to_owned()
    } else {
        format!("{base}/audio/transcriptions")
    };
    Ok((url, host.to_owned(), local))
}

fn api_key(config: &Config, local: bool) -> Result<Option<Zeroizing<String>>, TranscribeError> {
    let name = config.transcription.openai_compatible_api_key_env.trim();
    if name.is_empty() && local {
        return Ok(None);
    }
    if name.is_empty()
        || !name
            .bytes()
            .enumerate()
            .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    {
        return Err(failed(
            "set openai_compatible_api_key_env to an environment variable name",
        ));
    }
    let key = Zeroizing::new(
        std::env::var(name)
            .map_err(|_| failed("configured API key environment variable is missing"))?,
    );
    if key.trim().is_empty() || key.contains(['\r', '\n']) {
        return Err(failed(
            "configured API key environment variable is empty or invalid",
        ));
    }
    Ok(Some(key))
}

/// Configuration-only health check: never probes an endpoint or prints a key.
pub fn remote_transcription_status(config: &Config) -> Result<(), String> {
    let check = || {
        let (_, _, local) = endpoint(config)?;
        api_key(config, local)?;
        multipart_parts(config)?;
        Ok::<_, TranscribeError>(())
    };
    check().map_err(|e| e.to_string())
}

/// Validate an endpoint without connecting, returning only its host.
/// Desktop settings use this before persisting a URL so credentials cannot
/// accidentally be saved in URL userinfo or a query string.
pub fn remote_transcription_host(config: &Config) -> Result<String, String> {
    endpoint(config)
        .map(|(_, host, _)| host)
        .map_err(|e| e.to_string())
}

/// Shared health/settings/consent wording. Only a parsed host is displayed.
pub fn remote_transcription_notice(config: &Config) -> String {
    let host = remote_transcription_host(config)
        .unwrap_or_else(|_| "an invalid endpoint (uploads blocked)".into());
    format!("Batch transcription is remote: audio is sent to {host}. Live transcript and dictation stay local. Unclassified or restricted recordings cannot be uploaded.")
}

/// Exact policy bytes travel with prepared stems, never as caller-supplied
/// booleans. Equality preserves DecodeHints' existing comparison contract.
#[derive(Debug, Clone)]
pub(super) struct AudioPolicy(AuthorizedMeetingSnapshot);

impl PartialEq for AudioPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.0.path == other.0.path && self.0.content == other.0.content
    }
}
impl Eq for AudioPolicy {}

pub(super) fn authorize_audio(
    audio: &Path,
    config: &Config,
) -> Result<AudioPolicy, TranscribeError> {
    // The companion must be an active corpus member. Raw audio is not proof
    // of normal sensitivity. Never invent metadata from a future transcript.
    let snapshot = read_authorized_meeting(&audio.with_extension("md"), config, false)
        .map_err(|_| failed("egress policy refused: a matching active, normal-sensitivity Markdown record is required before transcription"))?;
    if snapshot.frontmatter.capture == Some(crate::markdown::CapturePolicy::None) {
        return Err(failed(
            "egress policy refused: conversation has capture disabled",
        ));
    }
    Ok(AudioPolicy(snapshot))
}

const BOUNDARY: &str = "minutes-audio-transcription-boundary";

fn multipart_parts(config: &Config) -> Result<(Vec<u8>, Vec<u8>, usize), TranscribeError> {
    let model = config.transcription.openai_compatible_model.trim();
    if model.is_empty() || model.contains(['\r', '\n']) || model.contains(BOUNDARY) {
        return Err(failed("openai_compatible_model is empty or invalid"));
    }
    let mut prefix = format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n");
    if let Some(language) = config
        .transcription
        .language
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "auto")
    {
        if !language
            .bytes()
            .all(|c| c.is_ascii_alphabetic() || c == b'-')
        {
            return Err(failed("invalid transcription language"));
        }
        prefix.push_str(&format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n{language}\r\n"));
    }
    prefix.push_str(&format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"));
    let suffix = format!("\r\n--{BOUNDARY}--\r\n").into_bytes();
    let max = config.transcription.openai_compatible_max_upload_bytes;
    // Bound allocation even for an accidentally enormous setting. WAV is
    // mono PCM16; include its 44-byte header and all multipart overhead.
    let capacity = max.checked_sub(prefix.len() + suffix.len() + 44).map(|n| n / 2)
        .filter(|n| *n > 0 && max <= 100_000_000)
        .ok_or_else(|| failed("openai_compatible_max_upload_bytes must fit a WAV plus multipart overhead and be at most 100 MB"))?;
    Ok((prefix.into_bytes(), suffix, capacity))
}

#[derive(Deserialize)]
struct Response {
    segments: Vec<Segment>,
    language: Option<String>,
}
#[derive(Deserialize)]
struct Segment {
    start: f64,
    end: f64,
    text: String,
}

pub(super) fn transcribe(
    audio: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    let (url, _, local) = endpoint(config)?;
    let key = api_key(config, local)?;
    let (prefix, suffix, capacity) = multipart_parts(config)?;
    let policy = if let Some(policy) = &hints.remote_policy {
        policy.clone()
    } else {
        std::sync::Arc::new(authorize_audio(audio, config)?)
    };
    policy
        .0
        .reauthorize_exact(config, false)
        .map_err(|_| failed("egress policy changed or became unavailable"))?;
    let samples =
        super::load_audio_samples_with_format(audio, hints.audio_format_extension(), config)?;
    if samples.is_empty() {
        return Err(failed("audio is empty"));
    }
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build(),
    );
    let mut segments = Vec::new();
    let mut detected_language = None;
    for (index, chunk) in samples.chunks(capacity).enumerate() {
        let mut body = prefix.clone();
        super::write_wav_16k_mono_to_writer(&mut body, chunk)?;
        body.extend_from_slice(&suffix);
        let mut request = agent.post(&url).header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
        let auth = key
            .as_ref()
            .map(|key| Zeroizing::new(format!("Bearer {}", key.as_str())));
        if let Some(auth) = &auth {
            let mut header = ureq::http::HeaderValue::from_str(auth.as_str())
                .map_err(|_| failed("invalid API key header"))?;
            header.set_sensitive(true);
            request = request.header("Authorization", header);
        }
        // Final live check for EVERY upload, including the second stem/chunk.
        policy
            .0
            .reauthorize_exact(config, false)
            .map_err(|_| failed("egress policy changed or became unavailable"))?;
        let mut response = request
            .send(body.as_slice())
            .map_err(|_| failed("request failed (check endpoint and network)"))?;
        if !response.status().is_success() {
            return Err(failed(&format!(
                "endpoint returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let response: Response = response
            .body_mut()
            .with_config()
            .limit(8_000_000)
            .read_json()
            .map_err(|_| failed("invalid or oversized verbose_json response"))?;
        let offset = (index * capacity) as f64 / 16000.0;
        let duration = chunk.len() as f64 / 16000.0;
        let mut previous = 0.0;
        for segment in response.segments {
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < previous
                || segment.end < segment.start
                || segment.end > duration + 0.01
            {
                return Err(failed(
                    "invalid segment timestamps in verbose_json response",
                ));
            }
            previous = segment.start;
            if !segment.text.trim().is_empty() {
                segments.push(TranscriptSegment {
                    start: offset + segment.start,
                    end: offset + segment.end,
                    text: segment.text.trim().to_owned(),
                });
            }
        }
        if detected_language.is_none() {
            detected_language = response.language;
        }
    }
    let words = segments
        .iter()
        .map(|s| s.text.split_whitespace().count())
        .sum::<usize>();
    if words == 0 || words < config.transcription.min_words {
        return Err(failed("transcription produced insufficient text"));
    }
    let text = segments
        .iter()
        .map(|s| {
            let secs = s.start.floor() as u64;
            format!("[{}:{:02}] {}\n", secs / 60, secs % 60, s.text)
        })
        .collect::<String>();
    Ok(TranscribeResult {
        text,
        stats: FilterStats {
            audio_duration_secs: samples.len() as f64 / 16000.0,
            samples_after_silence_strip: samples.len(),
            raw_segments: segments.len(),
            after_no_speech_filter: segments.len(),
            after_dedup: segments.len(),
            after_interleaved: segments.len(),
            after_script_filter: segments.len(),
            after_noise_markers: segments.len(),
            after_trailing_trim: segments.len(),
            final_words: words,
            ..Default::default()
        },
        segments,
        detected_language,
    })
}

#[cfg(test)]
mod tests;
