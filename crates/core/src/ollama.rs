//! Shared Ollama HTTP/NDJSON streaming adapter.
//!
//! Recall chat and the real-time copilot both use this module. Keeping the
//! transport here gives every surface the same cancellation, timeout, HTTP
//! error, and NDJSON parsing behavior without coupling the core to Tauri.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_shared(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

impl OllamaChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OllamaStreamRequest {
    pub messages: Vec<OllamaChatMessage>,
    /// Ollama accepts either `"json"` or a JSON schema object here.
    pub format: Option<serde_json::Value>,
    pub temperature: Option<f32>,
    /// Thinking-capable models default this on in current Ollama releases.
    /// Latency-sensitive callers must explicitly disable it.
    pub think: Option<bool>,
}

impl OllamaStreamRequest {
    pub fn chat(messages: Vec<OllamaChatMessage>) -> Self {
        Self {
            messages,
            format: None,
            temperature: None,
            think: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaStreamResult {
    pub text: String,
    pub chunks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaHealth {
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, thiserror::Error)]
pub enum OllamaError {
    #[error("Ollama request cancelled")]
    Cancelled,
    #[error("Ollama transport error: {0}")]
    Transport(String),
    #[error("Ollama HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("Ollama stream read error: {0}")]
    StreamRead(String),
    #[error("Ollama returned an invalid NDJSON frame: {0}")]
    InvalidFrame(String),
    #[error("Ollama returned no response text")]
    EmptyResponse,
}

#[derive(Debug, Clone)]
pub struct OllamaAdapter {
    base_url: String,
    model: String,
    timeout: Duration,
}

impl OllamaAdapter {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, timeout: Duration) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            timeout,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(self.timeout))
                .http_status_as_error(false)
                .build(),
        )
    }

    /// Check whether Ollama is reachable without loading a model.
    pub fn health(&self) -> OllamaHealth {
        let url = format!("{}/api/tags", self.base_url);
        match self.agent().get(&url).call() {
            Ok(response) if response.status().as_u16() < 400 => OllamaHealth {
                available: true,
                detail: format!("Ollama is reachable at {}", self.base_url),
            },
            Ok(mut response) => OllamaHealth {
                available: false,
                detail: format!(
                    "Ollama HTTP {}: {}",
                    response.status().as_u16(),
                    response.body_mut().read_to_string().unwrap_or_default()
                ),
            },
            Err(error) => OllamaHealth {
                available: false,
                detail: format!("Ollama is unavailable at {}: {error}", self.base_url),
            },
        }
    }

    /// Ask Ollama to load the configured model and keep it warm.
    pub fn prewarm(&self) -> Result<(), OllamaError> {
        let url = format!("{}/api/generate", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "prompt": "",
            "stream": false,
            "keep_alive": "10m"
        });
        let mut response = self
            .agent()
            .post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|error| OllamaError::Transport(error.to_string()))?;
        if response.status().as_u16() >= 400 {
            return Err(OllamaError::Http {
                status: response.status().as_u16(),
                body: response.body_mut().read_to_string().unwrap_or_default(),
            });
        }
        Ok(())
    }

    /// Stream `/api/chat` NDJSON, forwarding each content delta to `on_chunk`.
    ///
    /// Cancellation is cooperative between frames. The HTTP timeout remains a
    /// hard upper bound when a server accepts the connection but stops sending.
    pub fn stream_chat(
        &self,
        request: &OllamaStreamRequest,
        cancel: &CancelToken,
        mut on_chunk: impl FnMut(&str),
    ) -> Result<OllamaStreamResult, OllamaError> {
        if cancel.is_cancelled() {
            return Err(OllamaError::Cancelled);
        }

        let url = format!("{}/api/chat", self.base_url);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": request.messages,
            "stream": true,
        });
        if let Some(format) = &request.format {
            body["format"] = format.clone();
        }
        if let Some(temperature) = request.temperature {
            body["options"] = serde_json::json!({ "temperature": temperature });
        }
        if let Some(think) = request.think {
            body["think"] = serde_json::Value::Bool(think);
        }

        let mut response = self
            .agent()
            .post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|error| OllamaError::Transport(error.to_string()))?;

        if response.status().as_u16() >= 400 {
            return Err(OllamaError::Http {
                status: response.status().as_u16(),
                body: response.body_mut().read_to_string().unwrap_or_default(),
            });
        }

        let mut response_body = response.into_body();
        let reader = BufReader::new(response_body.as_reader());
        let mut full_response = String::new();
        let mut chunks = 0usize;

        for line_result in reader.lines() {
            if cancel.is_cancelled() {
                return Err(OllamaError::Cancelled);
            }
            let line = line_result.map_err(|error| OllamaError::StreamRead(error.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(&line)
                .map_err(|error| OllamaError::InvalidFrame(error.to_string()))?;
            if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
                return Err(OllamaError::InvalidFrame(error.to_string()));
            }
            if let Some(text) = value
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_str())
            {
                if !text.is_empty() {
                    if cancel.is_cancelled() {
                        return Err(OllamaError::Cancelled);
                    }
                    full_response.push_str(text);
                    chunks += 1;
                    on_chunk(text);
                }
            }
        }

        if cancel.is_cancelled() {
            return Err(OllamaError::Cancelled);
        }
        if full_response.trim().is_empty() {
            return Err(OllamaError::EmptyResponse);
        }

        Ok(OllamaStreamResult {
            text: full_response,
            chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn cancel_token_shares_state() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn adapter_normalizes_base_url() {
        let adapter = OllamaAdapter::new(
            "http://localhost:11434/",
            "llama3.2",
            Duration::from_secs(5),
        );
        assert_eq!(adapter.base_url(), "http://localhost:11434");
        assert_eq!(adapter.model(), "llama3.2");
    }

    #[test]
    fn adapter_streams_ollama_ndjson_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut buffer = [0_u8; 1024];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "client closed before sending headers");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            while bytes.len() < header_end + content_length {
                let mut buffer = [0_u8; 1024];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "client closed before sending its body");
                bytes.extend_from_slice(&buffer[..count]);
            }
            request_tx
                .send(String::from_utf8_lossy(&bytes).into_owned())
                .unwrap();

            let body = concat!(
                "{\"message\":{\"content\":\"hel\"},\"done\":false}\n",
                "{\"message\":{\"content\":\"lo\"},\"done\":true}\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            stream.flush().unwrap();
        });

        let adapter = OllamaAdapter::new(
            format!("http://{address}"),
            "llama3.2",
            Duration::from_secs(2),
        );
        let mut request = OllamaStreamRequest::chat(vec![OllamaChatMessage::new("user", "hello")]);
        request.think = Some(false);
        let mut chunks = Vec::new();
        let result = adapter
            .stream_chat(&request, &CancelToken::new(), |chunk| {
                chunks.push(chunk.to_string())
            })
            .unwrap();

        assert_eq!(result.text, "hello");
        assert_eq!(result.chunks, 2);
        assert_eq!(chunks, ["hel", "lo"]);
        let raw_request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(raw_request.contains("POST /api/chat"));
        let (_, request_body) = raw_request.split_once("\r\n\r\n").unwrap();
        let request_json: serde_json::Value = serde_json::from_str(request_body).unwrap();
        assert_eq!(request_json["model"], "llama3.2");
        assert_eq!(request_json["stream"], true);
        assert_eq!(request_json["think"], false);
        server.join().unwrap();
    }
}
