//! A minimal MCP client, so voice can reach tools Minutes does not implement.
//!
//! Servers are launched as child processes and spoken to over newline-delimited
//! JSON-RPC on stdio. Their `tools/list` is very nearly the shape the Live API
//! wants for function declarations, so each one is sanitized and merged into the
//! tool surface under a `server__tool` name, and a call routes straight back.
//!
//! Deliberately blocking, matching the rest of minutes-core: one reader thread
//! per server feeds a channel, and calls are serialized by the voice tool
//! worker, so there is no runtime here and no tokio.
//!
//! Secrets never appear in config. A child inherits this process's environment,
//! so a server that wants a token reads the same variable the user already
//! exports.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use serde_json::{json, Map, Value};

use crate::config::{Config, McpServerConfig};

/// Give a server this long to answer anything.
const CALL_TIMEOUT: Duration = Duration::from_secs(25);
/// Handshake budget: a cold `npx` server can be slow to boot.
const INIT_TIMEOUT: Duration = Duration::from_secs(45);
/// Tools taken from one server when it names no allowlist. Voice context is
/// small and tool choice degrades quickly, so this stays low on purpose.
const DEFAULT_MAX_TOOLS: usize = 8;
/// Separator between the server name and the tool name.
pub const SEP: &str = "__";

/// One connected server plus the tools it offered.
pub struct McpServer {
    pub name: String,
    conn: Mutex<Conn>,
    pub tools: Vec<McpTool>,
}

/// One tool, already named and shaped for the Live API.
#[derive(Debug, Clone)]
pub struct McpTool {
    /// The name the model calls, `server__tool`.
    pub qualified: String,
    /// The name the server knows.
    pub remote: String,
    pub declaration: Value,
}

struct Conn {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    /// Responses that arrived out of order, kept rather than dropped.
    stash: VecDeque<Value>,
    next_id: u64,
}

/// Every server configured for a session.
#[derive(Default)]
pub struct McpPool {
    pub servers: Vec<McpServer>,
}

impl McpPool {
    /// Launch every configured server. A server that fails to start is reported
    /// and skipped: a broken integration must not stop a voice session.
    pub fn launch(config: &Config) -> (Self, Vec<String>) {
        let mut servers = Vec::new();
        let mut problems = Vec::new();
        for spec in &config.voice_live.mcp_servers {
            if spec.name.trim().is_empty() || spec.command.trim().is_empty() {
                problems.push("an mcp_servers entry is missing name or command".to_string());
                continue;
            }
            match McpServer::start(spec) {
                Ok(server) => servers.push(server),
                Err(e) => problems.push(format!("{}: {e}", spec.name)),
            }
        }
        (Self { servers }, problems)
    }

    /// Declarations for every tool across every server.
    pub fn declarations(&self) -> Vec<Value> {
        self.servers
            .iter()
            .flat_map(|s| s.tools.iter().map(|t| t.declaration.clone()))
            .collect()
    }

    /// Route a `server__tool` call. `None` when no server owns that name.
    pub fn call(&self, qualified: &str, args: &Value) -> Option<Result<Value, String>> {
        let server = self
            .servers
            .iter()
            .find(|s| s.tools.iter().any(|t| t.qualified == qualified))?;
        let remote = server
            .tools
            .iter()
            .find(|t| t.qualified == qualified)?
            .remote
            .clone();
        Some(server.call(&remote, args))
    }

    /// Shut every server down.
    pub fn shutdown(&self) {
        for server in &self.servers {
            server.shutdown();
        }
    }
}

impl McpServer {
    fn start(spec: &McpServerConfig) -> Result<Self, String> {
        // Resolve the command to a real path. Minutes runs from a GUI bundle
        // with a minimal PATH, where a bare "node" or "npx" does not exist.
        let program = crate::summarize::resolve_agent_path(&spec.command);
        // Through the repository's audited boundary, so subprocess policy
        // stays enforceable by the disallowed-methods lint.
        let mut child = crate::engine_process::command(&program)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", spec.command))?;
        let stdin = child.stdin.take().ok_or("no stdin on the server")?;
        let stdout = child.stdout.take().ok_or("no stdout on the server")?;

        let (tx, rx) = bounded::<Value>(256);
        std::thread::Builder::new()
            .name(format!("mcp-{}", spec.name))
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if tx.send(value).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|e| format!("could not read from the server: {e}"))?;

        let server = Self {
            name: spec.name.clone(),
            conn: Mutex::new(Conn {
                child,
                stdin,
                rx,
                stash: VecDeque::new(),
                next_id: 1,
            }),
            tools: Vec::new(),
        };
        server.handshake()?;
        let tools = server.load_tools(spec)?;
        Ok(Self { tools, ..server })
    }

    fn handshake(&self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "minutes-voice-live", "version": env!("CARGO_PKG_VERSION") },
            }),
            INIT_TIMEOUT,
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    fn load_tools(&self, spec: &McpServerConfig) -> Result<Vec<McpTool>, String> {
        let listed = self.request("tools/list", json!({}), INIT_TIMEOUT)?;
        let raw = listed
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("tools/list returned no tools array")?;
        Ok(select_tools(&spec.name, raw, &spec.tools, spec.max_tools))
    }

    fn call(&self, remote: &str, args: &Value) -> Result<Value, String> {
        let result = self.request(
            "tools/call",
            json!({ "name": remote, "arguments": args }),
            CALL_TIMEOUT,
        )?;
        Ok(flatten_tool_result(&result))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "server lock poisoned")?;
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        writeln!(conn.stdin, "{msg}").map_err(|e| e.to_string())?;
        conn.stdin.flush().map_err(|e| e.to_string())
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let mut conn = self.conn.lock().map_err(|_| "server lock poisoned")?;
        let id = conn.next_id;
        conn.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(conn.stdin, "{msg}").map_err(|e| format!("writing to the server: {e}"))?;
        conn.stdin
            .flush()
            .map_err(|e| format!("writing to the server: {e}"))?;

        // A response that arrived while an earlier call was waiting.
        if let Some(pos) = conn.stash.iter().position(|v| response_id(v) == Some(id)) {
            let hit = conn.stash.remove(pos).expect("position just found");
            return unwrap_response(&hit);
        }

        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("{method} timed out after {}s", timeout.as_secs()));
            }
            match conn.rx.recv_timeout(left) {
                Ok(value) => match response_id(&value) {
                    Some(got) if got == id => return unwrap_response(&value),
                    // A response to something else, or a server notification.
                    Some(_) => {
                        if conn.stash.len() >= 32 {
                            conn.stash.pop_front();
                        }
                        conn.stash.push_back(value);
                    }
                    None => {}
                },
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("{method} timed out after {}s", timeout.as_secs()))
                }
                Err(RecvTimeoutError::Disconnected) => return Err("the server exited".into()),
            }
        }
    }

    fn shutdown(&self) {
        if let Ok(mut conn) = self.conn.lock() {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
        }
    }
}

impl Drop for McpPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn response_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(Value::as_u64)
}

fn unwrap_response(value: &Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(message.to_string());
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

/// Collapse an MCP tool result into something speakable.
///
/// Content blocks become their text. Anything else is passed through, so a
/// server returning structured content keeps it.
pub fn flatten_tool_result(result: &Value) -> Value {
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    let Some(blocks) = result.get("content").and_then(Value::as_array) else {
        return result.clone();
    };
    let text: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b.get("type").and_then(Value::as_str) {
            Some("text") => b.get("text").and_then(Value::as_str).map(str::to_string),
            Some(other) => Some(format!("[{other} content, not readable aloud]")),
            None => None,
        })
        .collect();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({ "ok": !is_error, "text": text.join("\n") })
}

/// Turn one server's `tools/list` into declarations, honouring an allowlist and
/// a cap. Without a cap a couple of servers would swamp the voice tool surface
/// and the model would stop choosing well.
pub fn select_tools(
    server: &str,
    raw: &[Value],
    allowlist: &[String],
    max_tools: usize,
) -> Vec<McpTool> {
    let cap = if max_tools == 0 {
        DEFAULT_MAX_TOOLS
    } else {
        max_tools
    };
    let mut out = Vec::new();
    for tool in raw {
        if out.len() >= cap {
            break;
        }
        let Some(remote) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !allowlist.is_empty() && !allowlist.iter().any(|a| a == remote) {
            continue;
        }
        let qualified = qualify(server, remote);
        if qualified.is_empty() {
            continue;
        }
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("No description given by the server.");
        let schema = tool
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        out.push(McpTool {
            declaration: json!({
                "name": qualified,
                "description": format!("[{server}] {description}"),
                "parameters": sanitize_schema(&schema),
                "behavior": "NON_BLOCKING",
            }),
            qualified,
            remote: remote.to_string(),
        });
    }
    out
}

/// `server__tool`, reduced to the characters a function name may contain.
pub fn qualify(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let (server, tool) = (clean(server), clean(tool));
    if server.is_empty() || tool.is_empty() {
        return String::new();
    }
    let mut name = format!("{server}{SEP}{tool}");
    name.truncate(64);
    name
}

/// Strip the JSON Schema keywords the Live API rejects.
///
/// Servers ship full JSON Schema. Function declarations take a subset, and one
/// unsupported keyword anywhere rejects the whole session setup, which would
/// take every other tool down with it.
pub fn sanitize_schema(schema: &Value) -> Value {
    const DROP: &[&str] = &[
        "$schema",
        "$id",
        "$ref",
        "$defs",
        "definitions",
        "additionalProperties",
        "patternProperties",
        "allOf",
        "oneOf",
        "not",
        "if",
        "then",
        "else",
        "const",
        "examples",
        "default",
        "minLength",
        "maxLength",
        "pattern",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "uniqueItems",
        "minItems",
        "maxItems",
    ];
    match schema {
        Value::Object(map) => {
            let mut clean = Map::new();
            for (key, value) in map {
                if DROP.contains(&key.as_str()) {
                    continue;
                }
                clean.insert(key.clone(), sanitize_schema(value));
            }
            if !clean.contains_key("type") && clean.contains_key("properties") {
                clean.insert("type".into(), json!("object"));
            }
            Value::Object(clean)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualified_name_is_legal_and_reversible_enough() {
        assert_eq!(
            qualify("hubspot", "search_contacts"),
            "hubspot__search_contacts"
        );
        // Anything a function name may not contain becomes an underscore.
        assert_eq!(qualify("my server", "get/thing"), "my_server__get_thing");
        assert!(qualify("", "x").is_empty());
        assert!(qualify("x", "").is_empty());
        let long = qualify(&"s".repeat(40), &"t".repeat(40));
        assert!(long.len() <= 64, "function names are capped at 64");
    }

    #[test]
    fn unsupported_schema_keywords_are_dropped_at_every_depth() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": { "type": "string", "minLength": 1, "description": "keep me" },
                "nested": { "type": "object", "properties": { "x": { "type": "number", "maximum": 5 } } }
            },
            "required": ["query"]
        });
        let clean = sanitize_schema(&schema);
        let text = clean.to_string();
        for banned in ["$schema", "additionalProperties", "minLength", "maximum"] {
            assert!(!text.contains(banned), "{banned} survived");
        }
        // The parts the model needs survive.
        assert_eq!(clean["type"], "object");
        assert_eq!(clean["properties"]["query"]["description"], "keep me");
        assert_eq!(clean["required"][0], "query");
        assert_eq!(
            clean["properties"]["nested"]["properties"]["x"]["type"],
            "number"
        );
    }

    #[test]
    fn a_missing_type_is_restored_when_there_are_properties() {
        let clean = sanitize_schema(&json!({"properties": {"a": {"type": "string"}}}));
        assert_eq!(clean["type"], "object");
    }

    fn tool(name: &str) -> Value {
        json!({"name": name, "description": "d", "inputSchema": {"type": "object", "properties": {}}})
    }

    #[test]
    fn an_allowlist_selects_and_a_cap_bounds_the_tool_surface() {
        let raw: Vec<Value> = (0..20).map(|i| tool(&format!("t{i}"))).collect();
        // No allowlist: capped, so one server cannot swamp the voice context.
        assert_eq!(select_tools("s", &raw, &[], 0).len(), DEFAULT_MAX_TOOLS);
        assert_eq!(select_tools("s", &raw, &[], 3).len(), 3);
        // Allowlist: exactly those, and the server prefix is applied.
        let picked = select_tools("s", &raw, &["t2".into(), "t5".into()], 0);
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].qualified, "s__t2");
        assert_eq!(picked[0].remote, "t2");
        assert!(picked[0].declaration["description"]
            .as_str()
            .unwrap()
            .starts_with("[s] "));
    }

    #[test]
    fn a_tool_result_becomes_speakable_text() {
        let flat = flatten_tool_result(&json!({
            "content": [{"type": "text", "text": "line one"}, {"type": "text", "text": "line two"}]
        }));
        assert_eq!(flat["ok"], true);
        assert_eq!(flat["text"], "line one\nline two");

        let failed = flatten_tool_result(&json!({
            "content": [{"type": "text", "text": "nope"}], "isError": true
        }));
        assert_eq!(failed["ok"], false);

        // Structured content is handed through untouched.
        let structured =
            flatten_tool_result(&json!({"structuredContent": {"count": 3}, "content": []}));
        assert_eq!(structured["count"], 3);

        // A non-text block is named rather than silently dropped.
        let image = flatten_tool_result(&json!({"content": [{"type": "image", "data": "x"}]}));
        assert!(image["text"].as_str().unwrap().contains("image content"));
    }

    #[test]
    fn an_error_response_surfaces_its_message() {
        let err = unwrap_response(&json!({"id": 1, "error": {"message": "boom"}}));
        assert_eq!(err.unwrap_err(), "boom");
        let ok = unwrap_response(&json!({"id": 1, "result": {"a": 1}})).unwrap();
        assert_eq!(ok["a"], 1);
    }
}
