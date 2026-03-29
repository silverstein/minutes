use crate::config::Config;
use crate::error::TranscribeError;
use crate::streaming_whisper::StreamingResult;
use serde::Deserialize;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const PARTIAL_INTERVAL_SAMPLES: usize = 16000 * 2;
const MIN_TRANSCRIBE_SAMPLES: usize = 16000;
const CHUNK_SIZE: usize = 1600;

#[derive(Debug, Deserialize)]
struct HelperResponse {
    #[serde(rename = "type")]
    response_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    duration_secs: Option<f64>,
    #[serde(default)]
    message: Option<String>,
}

pub struct StreamingParakeet {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    buffer_samples: usize,
    samples_since_partial: usize,
    last_partial: String,
}

impl StreamingParakeet {
    pub fn new(config: &Config) -> Result<Self, TranscribeError> {
        let binary = crate::transcribe::resolve_parakeet_coreml_binary(config)?;
        let model_dir = &config.transcription.parakeet_coreml_model_dir;

        let mut cmd = Command::new(&binary);
        cmd.arg("--stream").arg("--model-dir").arg(model_dir);
        if let Some(language) = config.transcription.language.as_deref() {
            cmd.arg("--language").arg(language);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TranscribeError::ParakeetCoremlNotFound
            } else {
                TranscribeError::ParakeetCoremlFailed(format!("spawn error: {}", e))
            }
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            TranscribeError::ParakeetCoremlFailed("failed to capture helper stdin".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            TranscribeError::ParakeetCoremlFailed("failed to capture helper stdout".into())
        })?;

        tracing::info!(
            binary = %binary.display(),
            model_dir = %model_dir.display(),
            "started parakeet-coreml streaming helper"
        );

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            buffer_samples: 0,
            samples_since_partial: 0,
            last_partial: String::new(),
        })
    }

    pub fn feed(&mut self, samples: &[f32]) -> Option<StreamingResult> {
        for chunk in samples.chunks(CHUNK_SIZE) {
            if let Err(e) = self.write_command(&serde_json::json!({
                "cmd": "audio",
                "samples": chunk,
            })) {
                tracing::warn!("parakeet-coreml audio write failed: {}", e);
                return None;
            }
            self.buffer_samples += chunk.len();
            self.samples_since_partial += chunk.len();
        }

        if self.samples_since_partial < PARTIAL_INTERVAL_SAMPLES
            || self.buffer_samples < MIN_TRANSCRIBE_SAMPLES
        {
            return None;
        }

        if let Err(e) = self.write_command(&serde_json::json!({ "cmd": "transcribe" })) {
            tracing::warn!("parakeet-coreml transcribe command failed: {}", e);
            return None;
        }
        self.samples_since_partial = 0;

        match self.read_response() {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("parakeet-coreml partial read failed: {}", e);
                None
            }
        }
    }

    pub fn finalize(&mut self) -> Option<StreamingResult> {
        if self.buffer_samples < MIN_TRANSCRIBE_SAMPLES {
            return None;
        }

        if let Err(e) = self.write_command(&serde_json::json!({ "cmd": "finalize" })) {
            tracing::warn!("parakeet-coreml finalize command failed: {}", e);
            return None;
        }

        match self.read_response() {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("parakeet-coreml final read failed: {}", e);
                None
            }
        }
    }

    pub fn reset(&mut self) {
        if let Err(e) = self.write_command(&serde_json::json!({ "cmd": "reset" })) {
            tracing::warn!("parakeet-coreml reset command failed: {}", e);
        }
        self.buffer_samples = 0;
        self.samples_since_partial = 0;
        self.last_partial.clear();
    }

    pub fn duration_secs(&self) -> f64 {
        self.buffer_samples as f64 / 16000.0
    }

    fn write_command(&mut self, value: &serde_json::Value) -> Result<(), TranscribeError> {
        serde_json::to_writer(&mut self.stdin, value)
            .map_err(|e| TranscribeError::ParakeetCoremlFailed(format!("encode error: {}", e)))?;
        self.stdin.write_all(b"\n").map_err(TranscribeError::Io)?;
        self.stdin.flush().map_err(TranscribeError::Io)
    }

    fn read_response(&mut self) -> Result<Option<StreamingResult>, TranscribeError> {
        let mut line = String::new();

        loop {
            line.clear();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(TranscribeError::Io)?;
            if read == 0 {
                return Err(TranscribeError::ParakeetCoremlFailed(
                    "helper exited before sending a response".into(),
                ));
            }

            let raw = line.trim();
            if raw.is_empty() {
                continue;
            }

            let response: HelperResponse = match serde_json::from_str(raw) {
                Ok(response) => response,
                Err(e) => {
                    tracing::debug!(
                        line = raw,
                        error = %e,
                        "skipping non-JSON parakeet-coreml helper output"
                    );
                    continue;
                }
            };

            if response.response_type == "error" {
                let message = response
                    .message
                    .or({
                        if response.text.is_empty() {
                            None
                        } else {
                            Some(response.text)
                        }
                    })
                    .unwrap_or_else(|| raw.to_string());
                return Err(TranscribeError::ParakeetCoremlFailed(message));
            }

            if response.response_type != "partial" && response.response_type != "final" {
                tracing::debug!(
                    response_type = %response.response_type,
                    "skipping unexpected parakeet-coreml response"
                );
                continue;
            }

            let text = response.text.trim().to_string();
            if text.is_empty() {
                return Ok(None);
            }

            let is_final = response.response_type == "final";
            if !is_final && text == self.last_partial {
                return Ok(None);
            }

            let duration_secs = response
                .duration_secs
                .unwrap_or_else(|| self.duration_secs());
            self.last_partial = text.clone();

            return Ok(Some(StreamingResult {
                text,
                is_final,
                duration_secs,
            }));
        }
    }
}

impl Drop for StreamingParakeet {
    fn drop(&mut self) {
        let _ = self.write_command(&serde_json::json!({ "cmd": "quit" }));

        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
