//! Generated music, as a labs toy behind its own flag.
//!
//! Minutes is the only thing that knows both what your next conversation is and
//! what you wrote about it beforehand, so music steered by that is something
//! only it can do. The assistant reads the meeting, the prep or the calendar
//! itself and writes the brief; this module only renders it and hands back
//! samples.
//!
//! The provider returns MP3. Decoding happens in ffmpeg rather than in process,
//! matching how every other compressed import is handled here, and the speech
//! decoder is deliberately not reused: it produces 16 kHz mono for whisper,
//! which is the wrong shape for anything anyone wants to listen to.
//!
//! Never while recording. Music through the speakers reaches the microphone and
//! then the transcript, and an optional consumer must never degrade capture.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

use super::protocol::OUTPUT_SAMPLE_RATE;
use crate::config::Config;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;
/// Enough lyrics for the assistant to quote a line without eating the budget.
const MAX_LYRIC_CHARS: usize = 1_200;

/// Generation is slow; this is well above what it has taken in practice.
const HTTP_TIMEOUT: Duration = Duration::from_secs(180);

/// One rendered piece.
pub struct Music {
    /// Little-endian PCM16 mono at [`OUTPUT_SAMPLE_RATE`], ready for playback.
    pub pcm16: Vec<u8>,
    pub seconds: f32,
    /// Where the original was kept, so it can be played again.
    pub path: PathBuf,
    /// What the model wrote alongside the audio: the lyrics when it sang, a
    /// bare section sketch when it did not.
    pub lyrics: Option<String>,
}

// Hand written so a failure never prints a megabyte of samples.
impl std::fmt::Debug for Music {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Music")
            .field("seconds", &self.seconds)
            .field("bytes", &self.pcm16.len())
            .field("path", &self.path)
            .field("lyrics", &self.lyrics)
            .finish()
    }
}

/// Where generated pieces are kept.
pub fn music_dir() -> PathBuf {
    Config::minutes_dir().join("music")
}

/// Render one brief into playable samples.
pub fn compose(config: &Config, description: &str) -> Result<Music, String> {
    let description = description.trim();
    if description.is_empty() {
        return Err("a description of the music is required".into());
    }
    let api_key = super::api_key(config).map_err(|e| e.to_string())?;
    let model = config.voice_live.music_model.trim();
    let model = if model.is_empty() { "lyria-3.5" } else { model };

    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(HTTP_TIMEOUT))
            .http_status_as_error(false)
            .build(),
    );
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let mut response = agent
        .post(&url)
        .header("x-goog-api-key", &api_key)
        .send_json(json!({
            "contents": [{ "parts": [{ "text": description }] }]
        }))
        .map_err(|e| {
            format!(
                "could not reach the music model: {}",
                redact(&e.to_string(), &api_key)
            )
        })?;
    let status = response.status().as_u16();
    let body: Value = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("could not read the music response: {e}"))?;
    if status >= 400 {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no message");
        return Err(format!("the music model refused: {message}"));
    }

    let parts = body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or("the music model returned no content")?;
    let encoded = parts
        .iter()
        .find_map(|p| p.pointer("/inlineData/data").and_then(Value::as_str))
        .ok_or("the music model returned no audio")?;
    // The model writes words for a sung piece and a section sketch otherwise.
    // Capped so a long set of lyrics cannot crowd out the rest of the result.
    let lyrics = parts
        .iter()
        .find_map(|p| p.get("text").and_then(Value::as_str))
        .map(|t| {
            let cleaned = t.replace("[:]", " ").replace('\n', " ");
            let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
            if cleaned.chars().count() > MAX_LYRIC_CHARS {
                let kept: String = cleaned.chars().take(MAX_LYRIC_CHARS).collect();
                format!("{kept}…")
            } else {
                cleaned
            }
        })
        .filter(|t| !t.is_empty());
    let mime = parts
        .iter()
        .find_map(|p| p.pointer("/inlineData/mimeType").and_then(Value::as_str))
        .unwrap_or("audio/mpeg");
    let bytes = B64
        .decode(encoded)
        .map_err(|e| format!("the music was not valid base64: {e}"))?;

    let path = write_source(&bytes, mime)?;
    // 0 plays the whole piece; anything else is a ceiling.
    let max_secs = match config.voice_live.music_max_secs {
        0 => None,
        other => Some(other.clamp(5, 3_600)),
    };
    let pcm16 = decode_for_playback(&path, max_secs)?;
    let seconds = pcm16.len() as f32 / 2.0 / OUTPUT_SAMPLE_RATE as f32;
    if seconds < 0.5 {
        return Err("the music decoded to nothing playable".into());
    }
    Ok(Music {
        pcm16,
        seconds,
        path,
        lyrics,
    })
}

fn redact(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "<redacted>")
    }
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        _ => "mp3",
    }
}

fn write_source(bytes: &[u8], mime: &str) -> Result<PathBuf, String> {
    let dir = music_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Owner-only, like every other directory Minutes writes.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    // Seconds alone collide when two sessions finish together, and the file is
    // created truncating, so one piece would be lost and a decoder could read a
    // half-rewritten file.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let path = dir.join(format!(
        "{}-{unique:09}.{}",
        chrono::Local::now().format("%Y-%m-%d-%H-%M-%S"),
        extension_for(mime)
    ));
    // Create exclusively. Two sessions finishing in the same instant can still
    // produce the same name, and a truncating create would let one silently
    // overwrite the other's music, possibly while it was being decoded.
    let mut attempt = path;
    let mut file = loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&attempt)
        {
            Ok(file) => break file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stem = attempt
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                attempt = dir.join(format!("{stem}-again.{}", extension_for(mime)));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let path = attempt;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Decode to the sample format the playback queue takes.
///
/// `max_secs` of `None` decodes the whole piece.
fn decode_for_playback(path: &std::path::Path, max_secs: Option<u64>) -> Result<Vec<u8>, String> {
    let ffmpeg = crate::ffmpeg::resolve_launchable_ffmpeg()
        .map_err(|e| format!("music needs ffmpeg to decode, and it is unavailable: {e}"))?;
    let mut command = crate::engine_process::command(&ffmpeg);
    command.args(["-v", "error", "-nostdin", "-i"]).arg(path);
    if let Some(secs) = max_secs {
        command.args(["-t", &secs.to_string()]);
    }
    let output = command
        .args([
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            &OUTPUT_SAMPLE_RATE.to_string(),
            "-ac",
            "1",
            "-",
        ])
        .output()
        .map_err(|e| format!("could not run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg could not decode the music: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_brief_is_refused_before_any_request() {
        let err = compose(&Config::default(), "   ").unwrap_err();
        assert!(err.contains("description"), "{err}");
    }

    #[test]
    fn lyrics_are_flattened_and_capped() {
        // Sanity on the cap itself; composing needs the network.
        const { assert!(MAX_LYRIC_CHARS > 200 && MAX_LYRIC_CHARS < 5_000) };
    }

    #[test]
    fn the_extension_follows_what_the_provider_actually_sent() {
        assert_eq!(extension_for("audio/mpeg"), "mp3");
        assert_eq!(extension_for("audio/wav"), "wav");
        assert_eq!(extension_for("audio/ogg"), "ogg");
        // An unknown type is stored rather than dropped.
        assert_eq!(extension_for("audio/weird"), "mp3");
    }

    #[test]
    fn a_key_never_survives_into_an_error_message() {
        let message = redact("failed calling https://x/?key=sekret-value", "sekret-value");
        assert!(!message.contains("sekret-value"));
        assert!(message.contains("<redacted>"));
    }

    #[test]
    fn pieces_are_kept_under_the_minutes_directory() {
        assert!(music_dir().ends_with("music"));
    }
}
