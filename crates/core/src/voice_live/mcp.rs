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
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Longest single line a server may send. Without a bound, a server that
/// never writes a newline makes the reader allocate forever, which no channel
/// bound or stash limit can stop.
const MAX_LINE_BYTES: usize = 1 << 20;
/// Longest request we will write.
const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// How long to keep trying to hand a server a request before giving up on it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Separator between the server name and the tool name.
pub const SEP: &str = "__";

/// One connected server plus the tools it offered.
pub struct McpServer {
    pub name: String,
    conn: Mutex<Conn>,
    /// Kept outside the lock so a wedged connection can still be killed. A
    /// server that stops reading its stdin blocks the writer while it holds the
    /// lock, and shutdown that also wanted the lock would wait forever.
    pid: u32,
    /// Set once the child has been waited on. After that the pid may belong to
    /// an unrelated process, so it must never be signalled again.
    reaped: AtomicBool,
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
    /// Set when a write gave up part way. Whatever reached the server is an
    /// unterminated fragment, and appending the next request to it produces one
    /// malformed frame rather than two good ones, so the connection is done.
    broken: bool,
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
    ///
    /// Names are deduplicated across servers as well as within one: a server
    /// called `a` with a tool `b__c` and a server called `a__b` with a tool `c`
    /// both reduce to `a__b__c`, and dispatch takes the first.
    pub fn declarations(&self) -> Vec<Value> {
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for server in &self.servers {
            for tool in &server.tools {
                if seen.iter().any(|n| n == &tool.qualified) {
                    continue;
                }
                seen.push(tool.qualified.clone());
                out.push(tool.declaration.clone());
            }
        }
        out
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
        // Non-blocking, so a server that stops reading cannot pin the writer,
        // and with it the connection lock, for the life of the process. A size
        // cap alone was not enough: successive requests fill the pipe and the
        // next write blocks before any timeout of ours can start.
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                let fd = stdin.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                }
            }
        }
        let stdout = child.stdout.take().ok_or("no stdout on the server")?;

        let (tx, rx) = bounded::<Value>(256);
        std::thread::Builder::new()
            .name(format!("mcp-{}", spec.name))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    // Bounded: `read_line` on a server that never sends one
                    // grows a single allocation without limit.
                    let mut limited = (&mut reader).take(MAX_LINE_BYTES as u64);
                    match limited.read_until(b'\n', &mut line) {
                        Ok(0) => return,
                        Ok(n) => {
                            if n >= MAX_LINE_BYTES && !line.ends_with(b"\n") {
                                // Oversized and still unterminated: this server
                                // is broken or hostile, and continuing means
                                // resynchronising on a stream we cannot frame.
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(text) {
                        if tx.send(value).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|e| format!("could not read from the server: {e}"))?;

        let pid = child.id();
        let server = Self {
            name: spec.name.clone(),
            pid,
            reaped: AtomicBool::new(false),
            conn: Mutex::new(Conn {
                child,
                stdin,
                broken: false,
                rx,
                stash: VecDeque::new(),
                next_id: 1,
            }),
            tools: Vec::new(),
        };
        // A server that starts but never answers must not be left running when
        // we give up on it.
        if let Err(e) = server.handshake() {
            server.shutdown();
            return Err(e);
        }
        let tools = match server.load_tools(spec) {
            Ok(tools) => tools,
            Err(e) => {
                server.shutdown();
                return Err(e);
            }
        };
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
        if conn.broken {
            return Err("that server is no longer usable".into());
        }
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string();
        let sent = write_line(&mut conn.stdin, &msg);
        if sent.is_err() {
            conn.broken = true;
        }
        sent
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let mut conn = self.conn.lock().map_err(|_| "server lock poisoned")?;
        if conn.broken {
            return Err("that server is no longer usable".into());
        }
        let id = conn.next_id;
        conn.next_id += 1;
        let msg =
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        if msg.len() > MAX_REQUEST_BYTES {
            return Err(format!(
                "that request is too large to send to {method} safely"
            ));
        }
        if let Err(e) = write_line(&mut conn.stdin, &msg) {
            // A partial line is on the wire and cannot be taken back.
            conn.broken = true;
            return Err(e);
        }

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
        if self.reaped.load(Ordering::SeqCst) {
            return;
        }
        // Prefer the clean path. `Child` owns the handle, so this cannot signal
        // a pid that has since been reused.
        if let Ok(mut conn) = self.conn.try_lock() {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
            self.reaped.store(true, Ordering::SeqCst);
            return;
        }
        // The lock is held, which means a write is blocked on a server that
        // stopped reading, which in turn means nothing has waited on the child.
        // So the pid is still this child's and signalling it is safe. Exactly
        // the case the clean path cannot reach.
        #[cfg(unix)]
        if self.pid > 0 {
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

impl Drop for McpPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The id of a genuine response to one of our requests.
///
/// A server may send its own requests, and those carry ids from its own
/// numbering. Matching on the id alone let a server-initiated `ping` with the
/// id we happened to be waiting on satisfy our call with an empty result.
/// Write one line, giving up rather than blocking forever.
///
/// The pipe is non-blocking, so a server that has stopped reading returns
/// `WouldBlock` instead of parking the caller. Without this a wedged server
/// holds the connection lock for the life of the process, which also stops
/// shutdown from taking the lock to kill it.
fn write_line(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    let bytes = line.as_bytes();
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut sent = 0;
    while sent < bytes.len() {
        if Instant::now() > deadline {
            return Err("the server stopped reading its input".into());
        }
        match stdin.write(&bytes[sent..]) {
            Ok(0) => return Err("the server closed its input".into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("writing to the server: {e}")),
        }
    }
    loop {
        if Instant::now() > deadline {
            return Err("the server stopped reading its input".into());
        }
        match stdin.write(b"\n") {
            Ok(0) => return Err("the server closed its input".into()),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("writing to the server: {e}")),
        }
    }
    // Flushing a pipe is a no-op, and a non-blocking one must not be waited on.
    Ok(())
}

fn response_id(value: &Value) -> Option<u64> {
    if value.get("method").is_some() {
        return None;
    }
    if value.get("result").is_none() && value.get("error").is_none() {
        return None;
    }
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
        // Two remote names can reduce to one function name, by character
        // replacement or by truncation. Dispatch matches the first, so a call
        // meant for the second would silently run the first with its arguments.
        if out.iter().any(|t: &McpTool| t.qualified == qualified) {
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
    sanitize_inner(schema, false)
}

/// `keys_are_names` marks an object whose keys are argument names rather than
/// schema keywords. Without it the filter deleted a legitimate argument called
/// `pattern` or `default` while leaving it listed in `required`, so the tool
/// declared a parameter the model could never supply.
fn sanitize_inner(schema: &Value, keys_are_names: bool) -> Value {
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
                if !keys_are_names && DROP.contains(&key.as_str()) {
                    continue;
                }
                // The values under `properties` are schemas; its keys are not.
                let child_keys_are_names = !keys_are_names && key == "properties";
                clean.insert(key.clone(), sanitize_inner(value, child_keys_are_names));
            }
            // Only in keyword position. An argument that happens to be called
            // "properties" is a name, and giving it a type here produces a
            // declaration that can fail the whole session setup.
            if !keys_are_names && !clean.contains_key("type") && clean.contains_key("properties") {
                clean.insert("type".into(), json!("object"));
            }
            Value::Object(clean)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| sanitize_inner(item, false))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
impl McpServer {
    /// A server with no child behind it, for testing name handling only.
    fn for_test(name: &str, tools: Vec<McpTool>) -> Self {
        let (_tx, rx) = bounded::<Value>(1);
        Self {
            name: name.to_string(),
            pid: 0,
            reaped: AtomicBool::new(true),
            conn: Mutex::new(Conn {
                child: crate::engine_process::command("true")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .spawn()
                    .expect("true should spawn"),
                stdin: {
                    let mut helper = crate::engine_process::command("true")
                        .stdin(Stdio::piped())
                        .spawn()
                        .expect("true should spawn");
                    helper.stdin.take().expect("piped stdin")
                },
                broken: false,
                rx,
                stash: VecDeque::new(),
                next_id: 1,
            }),
            tools,
        }
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
    fn an_argument_named_like_a_keyword_survives() {
        let clean = sanitize_schema(&json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": {"type": "string", "description": "a regex"},
                "default": {"type": "string"}
            },
            "required": ["pattern"]
        }));
        // These are argument names, not schema keywords.
        assert_eq!(clean["properties"]["pattern"]["description"], "a regex");
        assert!(clean["properties"].get("default").is_some());
        // The keyword in keyword position is still dropped.
        assert!(clean.get("additionalProperties").is_none());
        // And a keyword inside an argument's own schema still goes.
        let nested = sanitize_schema(&json!({
            "properties": {"q": {"type": "string", "minLength": 2}}
        }));
        assert!(nested["properties"]["q"].get("minLength").is_none());
    }

    #[test]
    fn a_name_cannot_be_claimed_by_two_servers() {
        let raw = vec![json!({"name": "c", "inputSchema": {"type": "object", "properties": {}}})];
        // Server "a" with tool "b__c" and server "a__b" with tool "c" collide.
        let first = select_tools("a", &[json!({"name": "b__c", "inputSchema": {}})], &[], 0);
        let second = select_tools("a__b", &raw, &[], 0);
        assert_eq!(first[0].qualified, second[0].qualified);
        let pool = McpPool {
            servers: vec![
                McpServer::for_test("a", first),
                McpServer::for_test("a__b", second),
            ],
        };
        assert_eq!(pool.declarations().len(), 1);
    }

    #[test]
    fn two_remote_names_cannot_become_one_function() {
        let raw = vec![
            json!({"name": "get/thing", "inputSchema": {"type": "object", "properties": {}}}),
            json!({"name": "get_thing", "inputSchema": {"type": "object", "properties": {}}}),
        ];
        let picked = select_tools("s", &raw, &[], 0);
        // Both reduce to s__get_thing; keeping both would dispatch the wrong one.
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].remote, "get/thing");
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
