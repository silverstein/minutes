//! Isolated text-embedding worker for Minutes semantic search.
//!
//! Runs EmbeddingGemma 2 (GGUF) through llama.cpp with Metal, in its own
//! process: its ggml never shares an image with whisper.cpp's, and a crash or
//! runaway allocation here cannot take the app down.
//!
//! Protocol: one JSON object per line on stdin, one per line on stdout.
//! - Startup writes `{"ready":true,"dimensions":N}` or `{"error":"..."}`.
//! - `{"documents":[{"title":"..","text":".."}]}` or `{"query":".."}` returns
//!   `{"vectors":[[f32;N],..]}` (mean-pooled, truncated to `--dimensions`,
//!   L2-normalized) or `{"error":".."}`.
//!
//! The model's retrieval prompts live here, beside the model, so callers send
//! meaning rather than formatting.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

const MAX_ITEMS_PER_REQUEST: usize = 64;
const MAX_CHARS_PER_ITEM: usize = 16_384;
const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    title: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    documents: Vec<Document>,
    #[serde(default)]
    query: Option<String>,
}

#[derive(Serialize, Default)]
struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vectors: Option<Vec<Vec<f32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// EmbeddingGemma's retrieval prompts (config_sentence_transformers.json).
fn document_prompt(title: &str, text: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { "none" } else { title };
    format!("title: {} | text: {}", title.replace('|', "/"), text)
}

fn query_prompt(query: &str) -> String {
    format!("task: search result | query: {query}")
}

fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

fn normalize(mut vector: Vec<f32>, dimensions: usize) -> Option<Vec<f32>> {
    vector.truncate(dimensions);
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    vector.iter_mut().for_each(|v| *v /= norm);
    Some(vector)
}

fn reply(out: &mut impl Write, response: &Response) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, response)?;
    out.write_all(b"\n")?;
    out.flush()
}

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(current) = args.next() {
        if current == name {
            return args.next();
        }
    }
    None
}

fn main() {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let outcome = run(&mut out);
    if let Err(error) = outcome {
        let _ = reply(
            &mut out,
            &Response {
                error: Some(error),
                ..Response::default()
            },
        );
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_out: &mut impl Write) -> Result<(), String> {
    Err("the embedding worker is only built for macOS".into())
}

#[cfg(target_os = "macos")]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/llama_bindings.rs"));
}

/// Tokens decoded together. EmbeddingGemma attends bidirectionally, and
/// llama.cpp requires a non-causal decode to fit one micro-batch, so this is
/// both the batch and the micro-batch; sequences are packed up to it.
#[cfg(target_os = "macos")]
const DECODE_TOKENS: u32 = 4_096;
/// Longest single input, in tokens (a 1,200-character chunk is ~300).
#[cfg(target_os = "macos")]
const SEQUENCE_TOKENS: usize = 2_048;

/// The loaded model and context. Freed in reverse order on drop.
#[cfg(target_os = "macos")]
struct Engine {
    model: *mut sys::llama_model,
    context: *mut sys::llama_context,
    vocab: *const sys::llama_vocab,
    native_dimensions: usize,
}

#[cfg(target_os = "macos")]
impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: both pointers came from llama.cpp, are non-null (checked in
        // `load`), and are freed exactly once, context before model.
        unsafe {
            sys::llama_free(self.context);
            sys::llama_model_free(self.model);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn discard_log(
    _level: sys::ggml_log_level,
    _text: *const std::ffi::c_char,
    _user: *mut std::ffi::c_void,
) {
}

#[cfg(target_os = "macos")]
impl Engine {
    fn load(model_path: &str) -> Result<Self, String> {
        let path = std::ffi::CString::new(model_path).map_err(|_| "model path contains NUL")?;
        // SAFETY: plain llama.cpp initialization calls with valid arguments;
        // every returned pointer is checked before use.
        unsafe {
            // stdout carries the protocol and stderr is the host's log budget;
            // llama.cpp's load chatter belongs in neither.
            sys::llama_log_set(Some(discard_log), std::ptr::null_mut());
            sys::llama_backend_init();
            let mut model_params = sys::llama_model_default_params();
            model_params.n_gpu_layers = 999;
            let model = sys::llama_model_load_from_file(path.as_ptr(), model_params);
            if model.is_null() {
                return Err("load model: llama.cpp could not load the file".into());
            }
            let mut params = sys::llama_context_default_params();
            params.embeddings = true;
            params.pooling_type = sys::llama_pooling_type_LLAMA_POOLING_TYPE_MEAN;
            params.n_ctx = DECODE_TOKENS;
            params.n_batch = DECODE_TOKENS;
            params.n_ubatch = DECODE_TOKENS;
            params.n_seq_max = MAX_ITEMS_PER_REQUEST as u32;
            let context = sys::llama_init_from_model(model, params);
            if context.is_null() {
                sys::llama_model_free(model);
                return Err("create context: llama.cpp refused the parameters".into());
            }
            let native_dimensions =
                usize::try_from(sys::llama_model_n_embd_out(model)).unwrap_or(0);
            Ok(Self {
                model,
                context,
                vocab: sys::llama_model_get_vocab(model),
                native_dimensions,
            })
        }
    }

    fn tokenize(&self, text: &str) -> Vec<sys::llama_token> {
        let bytes = text.as_bytes();
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        let mut tokens = vec![0; bytes.len() + 8];
        // SAFETY: `tokens` has room for `tokens.len()` entries; a negative
        // result is the size needed, retried once with exactly that room.
        let mut count = unsafe {
            sys::llama_tokenize(
                self.vocab,
                bytes.as_ptr().cast(),
                len,
                tokens.as_mut_ptr(),
                tokens.len() as i32,
                true,
                false,
            )
        };
        if count < 0 {
            tokens.resize(count.unsigned_abs() as usize, 0);
            count = unsafe {
                sys::llama_tokenize(
                    self.vocab,
                    bytes.as_ptr().cast(),
                    len,
                    tokens.as_mut_ptr(),
                    tokens.len() as i32,
                    true,
                    false,
                )
            };
        }
        tokens.truncate(usize::try_from(count).unwrap_or(0).min(SEQUENCE_TOKENS));
        tokens
    }

    /// Mean-pooled embeddings for sequences that fit one decode together.
    fn embed_group(&mut self, group: &[Vec<sys::llama_token>]) -> Result<Vec<Vec<f32>>, String> {
        let total: usize = group.iter().map(Vec::len).sum();
        // SAFETY: the batch is allocated for `total` tokens and `group.len()`
        // sequences and filled within those bounds; it is freed on every path.
        unsafe {
            let mut batch = sys::llama_batch_init(total.max(1) as i32, 0, group.len() as i32);
            for (seq, tokens) in group.iter().enumerate() {
                for (pos, token) in tokens.iter().enumerate() {
                    let i = batch.n_tokens as usize;
                    *batch.token.add(i) = *token;
                    *batch.pos.add(i) = pos as i32;
                    *batch.n_seq_id.add(i) = 1;
                    *(*batch.seq_id.add(i)) = seq as i32;
                    *batch.logits.add(i) = 1;
                    batch.n_tokens += 1;
                }
            }
            sys::llama_memory_clear(sys::llama_get_memory(self.context), true);
            let status = sys::llama_decode(self.context, batch);
            let mut vectors = Vec::with_capacity(group.len());
            let mut failure = (status != 0).then(|| format!("decode failed ({status})"));
            for seq in 0..group.len() {
                if failure.is_some() {
                    break;
                }
                let pooled = sys::llama_get_embeddings_seq(self.context, seq as i32);
                if pooled.is_null() {
                    failure = Some("no pooled embedding returned".into());
                } else {
                    vectors
                        .push(std::slice::from_raw_parts(pooled, self.native_dimensions).to_vec());
                }
            }
            sys::llama_batch_free(batch);
            match failure {
                Some(error) => Err(error),
                None => Ok(vectors),
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run(out: &mut impl Write) -> Result<(), String> {
    let model_path = arg("--model").ok_or("missing --model <path to .gguf>")?;
    let dimensions: usize = arg("--dimensions")
        .map(|value| value.parse().map_err(|_| "invalid --dimensions"))
        .transpose()?
        .unwrap_or(768);
    let mut engine = Engine::load(&model_path)?;
    if engine.native_dimensions == 0 || dimensions == 0 || dimensions > engine.native_dimensions {
        return Err(format!(
            "dimensions must be 1..={}",
            engine.native_dimensions
        ));
    }
    reply(
        out,
        &Response {
            ready: Some(true),
            dimensions: Some(dimensions),
            ..Response::default()
        },
    )
    .map_err(|e| e.to_string())?;

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(format!("read request: {error}")),
        }
        let response = match parse_request(&line) {
            Ok(texts) => embed_all(&mut engine, &texts, dimensions),
            Err(error) => error_response(&error),
        };
        reply(out, &response).map_err(|e| e.to_string())?;
    }
}

#[cfg(target_os = "macos")]
fn embed_all(engine: &mut Engine, texts: &[String], dimensions: usize) -> Response {
    let tokenized: Vec<_> = texts.iter().map(|text| engine.tokenize(text)).collect();
    let mut vectors = Vec::with_capacity(tokenized.len());
    let mut start = 0;
    while start < tokenized.len() {
        // Pack as many whole sequences as fit in one decode.
        let mut end = start;
        let mut total = 0;
        while end < tokenized.len() && total + tokenized[end].len() <= DECODE_TOKENS as usize {
            total += tokenized[end].len();
            end += 1;
        }
        match engine.embed_group(&tokenized[start..end]) {
            Ok(group) => {
                for raw in group {
                    match normalize(raw, dimensions) {
                        Some(vector) => vectors.push(vector),
                        None => return error_response("model returned an invalid vector"),
                    }
                }
            }
            Err(error) => return error_response(&error),
        }
        start = end;
    }
    Response {
        vectors: Some(vectors),
        ..Response::default()
    }
}

/// The prompted texts a request asks for, or why it is refused.
fn parse_request(line: &str) -> Result<Vec<String>, String> {
    if line.len() > MAX_REQUEST_BYTES {
        return Err("request too large".into());
    }
    match serde_json::from_str::<Request>(line) {
        Ok(Request {
            query: Some(query),
            documents,
        }) if documents.is_empty() => Ok(vec![query_prompt(truncate_chars(
            &query,
            MAX_CHARS_PER_ITEM,
        ))]),
        Ok(Request {
            query: None,
            documents,
        }) if !documents.is_empty() && documents.len() <= MAX_ITEMS_PER_REQUEST => Ok(documents
            .iter()
            .map(|doc| document_prompt(&doc.title, truncate_chars(&doc.text, MAX_CHARS_PER_ITEM)))
            .collect()),
        Ok(_) => Err(format!(
            "send either a query or 1..={MAX_ITEMS_PER_REQUEST} documents"
        )),
        Err(error) => Err(format!("invalid request: {error}")),
    }
}

fn error_response(error: &str) -> Response {
    Response {
        error: Some(error.to_string()),
        ..Response::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_match_the_model_card() {
        assert_eq!(document_prompt("", "hi"), "title: none | text: hi");
        assert_eq!(document_prompt("A | B", "hi"), "title: A / B | text: hi");
        assert_eq!(
            query_prompt("payroll"),
            "task: search result | query: payroll"
        );
    }

    #[test]
    fn normalize_truncates_and_rejects_degenerate_vectors() {
        let v = normalize(vec![3.0, 4.0, 12.0], 2).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert!(normalize(vec![0.0, 0.0], 2).is_none());
        assert!(normalize(vec![f32::NAN, 1.0], 2).is_none());
    }

    #[test]
    fn requests_are_a_query_or_a_bounded_document_list() {
        assert_eq!(
            parse_request(r#"{"query":"payroll"}"#).unwrap(),
            vec!["task: search result | query: payroll"]
        );
        assert_eq!(
            parse_request(r#"{"documents":[{"title":"T","text":"x"}]}"#).unwrap(),
            vec!["title: T | text: x"]
        );
        assert!(parse_request(r#"{"query":"a","documents":[{"text":"b"}]}"#).is_err());
        assert!(parse_request(r#"{"documents":[]}"#).is_err());
        assert!(parse_request(r#"{"texts":["raw"]}"#).is_err());
        let many = format!(
            r#"{{"documents":[{}]}}"#,
            vec![r#"{"text":"x"}"#; 65].join(",")
        );
        assert!(parse_request(&many).is_err());
    }

    #[test]
    fn truncate_chars_respects_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 10), "hi");
    }
}
