//! Tool declarations and dispatch into minutes-core.
//!
//! The model sees only what these return. Results are JSON text, truncated to a
//! per-call budget so one transcript cannot consume the voice context. Reads use
//! `include_restricted: true` because voice runs on the operator's own surface,
//! matching the desktop app. Phase 1 writes exactly one thing: `add_note`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Local};
use serde_json::{json, Value};

use crate::config::Config;
use crate::events::{InsightFilter, MeetingInsight};
use crate::graph::{PolicyProjectionRequest, PolicyProjectionResponse};
use crate::search::{self, SearchFilters};

use super::names::NameIndex;

/// Shared, read-only context for tool execution.
pub struct ToolContext {
    pub config: Config,
    pub names: Arc<NameIndex>,
    pub brain_root: Option<PathBuf>,
    pub max_chars: usize,
}

/// Result of one tool call.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub text: String,
    pub is_error: bool,
    pub elapsed: Duration,
    /// An image the host must push into the session before delivering `text`.
    /// Tool results are text, so a frame reaches the model as session media and
    /// the text only tells it the frame is there.
    pub image: Option<Vec<u8>>,
}

impl ToolContext {
    /// Resolve the knowledge root from config when brain search is on.
    pub fn new(config: Config, names: Arc<NameIndex>) -> Self {
        let brain_root =
            if config.voice_live.brain_search && !config.knowledge.path.as_os_str().is_empty() {
                let p = expand_home(&config.knowledge.path);
                p.is_dir().then(|| p.canonicalize().unwrap_or(p))
            } else {
                None
            };
        let max_chars = config.voice_live.max_tool_chars.max(1_000);
        Self {
            config,
            names,
            brain_root,
            max_chars,
        }
    }

    /// Function declarations in the Live API's OpenAPI-subset schema.
    pub fn declarations(&self) -> Vec<Value> {
        let mut d = vec![
            decl("get_status", "Whether a recording or processing job is active right now.", json!({})),
            decl(
                "list_meetings",
                "List recent meetings and voice memos, newest first. Paths returned here are the exact strings to pass to get_meeting.",
                json!({
                    "limit": {"type": "integer", "description": "Maximum results, 1 to 50 (default 10)"},
                    "type": {"type": "string", "enum": ["meeting", "memo"], "description": "Filter by content type"}
                }),
            ),
            decl(
                "search_meetings",
                "Full-text search across meeting transcripts and memos. Returns title, date, path, and a snippet.",
                json!({
                    "query": {"type": "string", "description": "Text to search for"},
                    "type": {"type": "string", "enum": ["meeting", "memo"]},
                    "since": {"type": "string", "description": "Only results on or after this date, YYYY-MM-DD"},
                    "attendee": {"type": "string", "description": "Only meetings with this attendee"},
                    "limit": {"type": "integer", "description": "Maximum results (default 15)"}
                }),
            ),
            decl(
                "get_meeting",
                "Read one meeting or memo by exact path: frontmatter (attendees, decisions, action items) plus the transcript, truncated.",
                json!({"path": {"type": "string", "description": "Exact path from list_meetings or search_meetings"}}),
            ),
            decl(
                "get_meeting_insights",
                "Extracted decisions, commitments, and open questions across recent meetings, with confidence and source.",
                json!({
                    "kind": {"type": "string", "enum": ["decision", "commitment", "question"]},
                    "participant": {"type": "string", "description": "Only insights involving this person"},
                    "since_days": {"type": "integer", "description": "Look back this many days (default 30)"},
                    "limit": {"type": "integer", "description": "Maximum results (default 25)"}
                }),
            ),
            decl(
                "research_topic",
                "Cross-meeting research on a topic: related decisions, open intents, recent meetings, and topics.",
                json!({"query": {"type": "string", "description": "Topic or question"}}),
            ),
            decl(
                "get_person_profile",
                "Profile for one person by exact name: recent meetings, open intents, recent decisions, top topics. If it comes back empty, call resolve_person.",
                json!({"name": {"type": "string", "description": "Person's name as spelled in the known-people list"}}),
            ),
            decl(
                "relationship_map",
                "The people Mat talks to most, with meeting counts, last contact, and open commitments.",
                json!({"limit": {"type": "integer", "description": "Maximum people (default 15)"}}),
            ),
            decl(
                "track_commitments",
                "Open commitments and action items, optionally for one person.",
                json!({
                    "person": {"type": "string", "description": "Person name; omit for everyone"},
                    "limit": {"type": "integer", "description": "Maximum results (default 25)"}
                }),
            ),
            decl(
                "consistency_report",
                "Decision conflicts and stale commitments across meetings.",
                json!({
                    "owner": {"type": "string", "description": "Only commitments owned by this person"},
                    "stale_after_days": {"type": "integer", "description": "Flag commitments older than this (default 7)"}
                }),
            ),
            decl(
                "add_note",
                "Save a timestamped note in Mat's words. Ask before calling.",
                json!({"text": {"type": "string", "description": "The note text"}}),
            ),
        ];
        if !self.names.people.is_empty() {
            d.push(decl(
                "resolve_person",
                "Fuzzy-match a spoken, possibly misheard, person name against the people in Mat's meeting history. Call it whenever a person lookup returns nothing or a heard name is not on the known list. Returns candidates with a 0 to 1 score.",
                json!({"name": {"type": "string", "description": "The name as heard"}}),
            ));
        }
        if self.config.voice_live.prep_artifacts {
            d.push(decl(
                "list_preps",
                "List the prep and debrief briefs Mat generated with the /minutes-prep and /minutes-brief skills, newest first. These are his own notes about an upcoming or past conversation, separate from the meeting transcripts. Follow up with get_prep.",
                json!({}),
            ));
            d.push(decl(
                "get_prep",
                "Read one prep or brief by the exact name returned from list_preps.",
                json!({"name": {"type": "string", "description": "File name from list_preps"}}),
            ));
        }
        if self.config.voice_live.calendar && self.config.calendar.enabled {
            d.push(decl(
                "upcoming_meetings",
                "Mat's upcoming calendar events, soonest first, with how many minutes until each one. Use it for what is next, when something starts, or who is attending.",
                json!({"within_minutes": {"type": "integer", "description": "Look-ahead window in minutes (default 720)"}}),
            ));
        }
        if self.config.voice_live.ask_agent {
            if let Some(agent) = delegate_agent(&self.config) {
                let agent_label = Path::new(&agent)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| agent.clone());
                d.push(decl(
                    "ask_agent",
                    &format!("Relay one question to Mat's local {agent_label} agent, which can read his code, files and connected services. Use it for anything outside meeting memory: his codebase, a repository, a document, or a system like a CRM or issue tracker. Ask one self-contained question, including any context from this conversation the agent would need, because it cannot hear you. It takes several seconds, so say you are checking before you call it."),
                    json!({"question": {"type": "string", "description": "A single self-contained question"}}),
                ));
            }
        }
        if self.config.voice_live.screen_on_request {
            d.push(decl_blocking(
                "look_at_screen",
                "Take one frame of whatever is on Mat's screen right now and look at it. Use it only when he asks about his screen, what he is looking at, or something visible in front of him. The frame is delivered as an image; describe what you actually see.",
                json!({}),
            ));
        }
        if let Some(root) = &self.brain_root {
            d.push(decl(
                "search_brain",
                &format!("Search Mat's personal knowledge base of markdown notes (people, companies, projects, areas, daily notes) rooted at {}. Returns matching files newest first with a snippet. Follow up with read_brain.", root.display()),
                json!({
                    "query": {"type": "string", "description": "A word or short phrase, case-insensitive"},
                    "limit": {"type": "integer", "description": "Maximum files (default 12)"}
                }),
            ));
            d.push(decl(
                "read_brain",
                "Read one file, or list one folder, from the knowledge base by the relative path returned from search_brain.",
                json!({"path": {"type": "string", "description": "Relative path inside the knowledge base"}}),
            ));
        }
        d
    }

    /// Execute one tool. Never panics; errors come back as text the model can speak.
    pub fn execute(&self, name: &str, args: &Value) -> ToolOutcome {
        let started = Instant::now();
        if name == "look_at_screen" {
            return self.look_at_screen(started);
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(name, args)));
        let (text, is_error) = match result {
            Ok(Ok(v)) => (v.to_string(), false),
            Ok(Err(e)) => (json!({"error": e}).to_string(), true),
            Err(_) => (
                json!({"error": format!("{name} panicked")}).to_string(),
                true,
            ),
        };
        let text = truncate(text, self.max_chars);
        ToolOutcome {
            text,
            is_error,
            elapsed: started.elapsed(),
            image: None,
        }
    }

    /// Capture one frame of the screen. Handled outside `dispatch` because it
    /// is the only tool that answers with media rather than text.
    fn look_at_screen(&self, started: Instant) -> ToolOutcome {
        let fail = |msg: String| ToolOutcome {
            text: json!({ "error": msg }).to_string(),
            is_error: true,
            elapsed: started.elapsed(),
            image: None,
        };
        if !self.config.voice_live.screen_on_request {
            return fail(
                "screen access is off; set [voice_live] screen_on_request = true in config.toml"
                    .into(),
            );
        }
        if !crate::screen::check_screen_permission() {
            return fail("no Screen Recording permission for this process".into());
        }
        let path = std::env::temp_dir().join(format!(
            "minutes-voice-screen-{}-{}.png",
            std::process::id(),
            started.elapsed().as_nanos()
        ));
        if let Err(e) = crate::screen::capture_screenshot(&path) {
            let _ = std::fs::remove_file(&path);
            return fail(format!("screenshot failed: {e}"));
        }
        let bytes = std::fs::read(&path);
        // The frame is sensitive and already in memory; never leave it on disk.
        let _ = std::fs::remove_file(&path);
        match bytes {
            Ok(bytes) => {
                // Deliberately no frontmost-app field. It named the window
                // running this session, and the model reported that name
                // instead of reading the frame, insisting it could only see a
                // terminal while a browser filled the screen. The image is the
                // only source of truth about what is on screen.
                ToolOutcome {
                    text: json!({
                        "delivered": true,
                        "captured_at": Local::now().to_rfc3339(),
                        "note": "A frame of Mat's screen taken just now was added to this conversation. It replaces any earlier frame: his screen has probably changed since, so describe only this newest one and never answer from a previous one. Say so plainly if it is unreadable.",
                    })
                    .to_string(),
                    is_error: false,
                    elapsed: started.elapsed(),
                    image: Some(bytes),
                }
            }
            Err(e) => fail(format!("could not read the captured frame: {e}")),
        }
    }

    fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        let cfg = &self.config;
        match name {
            "get_status" => {
                let s = crate::pid::status();
                Ok(json!({
                    "recording": s.recording,
                    "processing": s.processing,
                    "processing_stage": s.processing_stage,
                    "processing_title": s.processing_title,
                    "duration_secs": s.duration_secs,
                    // The prompt carries the date from session start; a long
                    // session needs the clock read fresh.
                    "now": Local::now().format("%A %Y-%m-%d %H:%M %Z").to_string(),
                }))
            }
            "list_meetings" => {
                let limit = int_arg(args, "limit", 10).clamp(1, 50);
                let filters = SearchFilters {
                    content_type: str_arg(args, "type"),
                    include_restricted: true,
                    ..Default::default()
                };
                let results = search::search("", cfg, &filters).map_err(|e| e.to_string())?;
                Ok(json!(results
                    .iter()
                    .take(limit)
                    .map(meeting_row)
                    .collect::<Vec<_>>()))
            }
            "search_meetings" => {
                let query = str_arg(args, "query").ok_or("query is required")?;
                let limit = int_arg(args, "limit", 15).clamp(1, 50);
                let filters = SearchFilters {
                    content_type: str_arg(args, "type"),
                    since: str_arg(args, "since"),
                    attendee: str_arg(args, "attendee"),
                    include_restricted: true,
                    ..Default::default()
                };
                let results = search::search(&query, cfg, &filters).map_err(|e| e.to_string())?;
                Ok(json!({
                    "total": results.len(),
                    "results": results.iter().take(limit).map(meeting_row).collect::<Vec<_>>(),
                }))
            }
            "get_meeting" => {
                let path = str_arg(args, "path").ok_or("path is required")?;
                let snap = search::read_authorized_meeting(Path::new(&path), cfg, true)
                    .map_err(|e| e.to_string())?;
                let fm = &snap.frontmatter;
                Ok(json!({
                    "path": snap.path,
                    "title": fm.title,
                    "date": fm.date.to_rfc3339(),
                    "type": serde_json::to_value(fm.r#type).unwrap_or(Value::Null),
                    "duration": fm.duration,
                    "attendees": fm.attendees,
                    "action_items": serde_json::to_value(&fm.action_items).unwrap_or(Value::Null),
                    "decisions": serde_json::to_value(&fm.decisions).unwrap_or(Value::Null),
                    "content": body_of(&snap.content),
                }))
            }
            "get_meeting_insights" => {
                let since_days = int_arg(args, "since_days", 30).clamp(1, 3650) as i64;
                let filter = InsightFilter {
                    kind: str_arg(args, "kind").and_then(|k| serde_json::from_value(json!(k)).ok()),
                    min_confidence: None,
                    participant: str_arg(args, "participant"),
                    since: Some(Local::now() - ChronoDuration::days(since_days)),
                    limit: Some(int_arg(args, "limit", 25).clamp(1, 100)),
                };
                let rows = crate::events::read_insights(&filter);
                Ok(json!(rows
                    .iter()
                    .map(|(at, insight, _)| insight_row(at, insight))
                    .collect::<Vec<_>>()))
            }
            "research_topic" => {
                let query = str_arg(args, "query").ok_or("query is required")?;
                let filters = SearchFilters {
                    include_restricted: true,
                    ..Default::default()
                };
                let r = search::cross_meeting_research(&query, cfg, &filters)
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(&r).map_err(|e| e.to_string())
            }
            "get_person_profile" => {
                let name = str_arg(args, "name").ok_or("name is required")?;
                match crate::graph_worker::run_policy_projection_worker(
                    cfg,
                    PolicyProjectionRequest::PersonProfile {
                        selector: name.clone(),
                    },
                )? {
                    PolicyProjectionResponse::PersonProfile(p) => {
                        let empty = p.recent_meetings.is_empty()
                            && p.open_intents.is_empty()
                            && p.recent_decisions.is_empty();
                        let mut v = serde_json::to_value(&p).map_err(|e| e.to_string())?;
                        if empty {
                            v["hint"] = json!("No history under this exact name. Call resolve_person with the name as heard.");
                        }
                        Ok(v)
                    }
                    _ => Err("unexpected projection response".into()),
                }
            }
            "relationship_map" => {
                let limit = int_arg(args, "limit", 15).clamp(1, 50);
                match crate::graph_worker::run_policy_projection_worker(
                    cfg,
                    PolicyProjectionRequest::RelationshipMap { limit },
                )? {
                    PolicyProjectionResponse::RelationshipMap(people) => {
                        serde_json::to_value(&people).map_err(|e| e.to_string())
                    }
                    _ => Err("unexpected projection response".into()),
                }
            }
            "track_commitments" => {
                let limit = int_arg(args, "limit", 25).clamp(1, 200);
                let selector = str_arg(args, "person");
                match crate::graph_worker::run_policy_projection_worker(
                    cfg,
                    PolicyProjectionRequest::Commitments { selector, limit },
                )? {
                    PolicyProjectionResponse::Commitments(c) => {
                        serde_json::to_value(&c).map_err(|e| e.to_string())
                    }
                    _ => Err("unexpected projection response".into()),
                }
            }
            "consistency_report" => {
                let owner = str_arg(args, "owner");
                let days = int_arg(args, "stale_after_days", 7).clamp(1, 365) as i64;
                let r = search::consistency_report(cfg, owner.as_deref(), days)
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(&r).map_err(|e| e.to_string())
            }
            "add_note" => {
                let text = str_arg(args, "text").ok_or("text is required")?;
                let saved = crate::notes::add_note(&text)?;
                Ok(json!({"saved": true, "detail": saved}))
            }
            "resolve_person" => {
                let heard = str_arg(args, "name").ok_or("name is required")?;
                Ok(json!({"query": heard, "candidates": self.names.resolve(&heard, 5)}))
            }
            "search_brain" => {
                let root = self
                    .brain_root
                    .as_ref()
                    .ok_or("knowledge base not configured")?;
                let query = str_arg(args, "query").ok_or("query is required")?;
                let limit = int_arg(args, "limit", 12).clamp(1, 40);
                Ok(brain_search(root, &query, limit))
            }
            "read_brain" => {
                let root = self
                    .brain_root
                    .as_ref()
                    .ok_or("knowledge base not configured")?;
                let rel = str_arg(args, "path").ok_or("path is required")?;
                brain_read(root, &rel, self.max_chars.saturating_sub(500))
            }
            "ask_agent" => {
                let question = str_arg(args, "question").ok_or("question is required")?;
                let agent =
                    delegate_agent(cfg).ok_or("no coding agent is configured or installed")?;
                let timeout =
                    Duration::from_secs(cfg.voice_live.delegate_timeout_secs.clamp(10, 900));
                let prompt = format!(
                    "You are answering one question relayed from a voice assistant. \
                     Answer from the files, systems and tools you can reach. Be specific and \
                     factual, and say plainly when you could not find something. Answer only: \
                     do not create, edit or delete anything unless the question explicitly asks \
                     you to. Reply in under 120 words of plain prose, no markdown and no code \
                     blocks, because it will be read aloud.\n\nQuestion: {question}"
                );
                crate::summarize::run_agent_prompt(&agent, &prompt, timeout)
                    .map(|answer| json!({ "agent": agent, "answer": answer }))
            }
            "list_preps" => Ok(list_prep_artifacts()),
            "get_prep" => {
                let name = str_arg(args, "name").ok_or("name is required")?;
                read_prep_artifact(&name, self.max_chars.saturating_sub(500))
            }
            "upcoming_meetings" => {
                let minutes = int_arg(args, "within_minutes", 720).clamp(5, 10_080) as u32;
                let events: Vec<Value> = crate::calendar::upcoming_events(minutes)
                    .into_iter()
                    .map(|e| {
                        json!({
                            "title": e.title,
                            "start": e.start,
                            "minutes_until": e.minutes_until,
                            "attendees": e.attendees,
                        })
                    })
                    .collect();
                Ok(json!({ "within_minutes": minutes, "events": events }))
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}

fn decl(name: &str, description: &str, properties: Value) -> Value {
    let required: Vec<&str> = match name {
        "search_meetings" | "research_topic" | "search_brain" => vec!["query"],
        "get_meeting" | "read_brain" => vec!["path"],
        "get_person_profile" | "resolve_person" => vec!["name"],
        "add_note" => vec!["text"],
        _ => vec![],
    };
    let mut params = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        params["required"] = json!(required);
    }
    decl_json(name, description, params, "NON_BLOCKING")
}

/// A tool the model must wait for before it answers.
///
/// Non-blocking is right for reads whose answer is still true a second later.
/// It is wrong for anything about this instant: the model issues the call and
/// answers immediately from what it already had, so a question about the screen
/// gets described from the previous frame and the assistant runs a turn behind.
fn decl_blocking(name: &str, description: &str, properties: Value) -> Value {
    decl_json(
        name,
        description,
        json!({"type": "object", "properties": properties}),
        "BLOCKING",
    )
}

fn decl_json(name: &str, description: &str, params: Value, behavior: &str) -> Value {
    json!({"name": name, "description": description, "parameters": params, "behavior": behavior})
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

fn int_arg(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn meeting_row(r: &search::SearchResult) -> Value {
    json!({"path": r.path, "title": r.title, "date": r.date, "type": r.content_type, "snippet": r.snippet})
}

fn insight_row(at: &chrono::DateTime<Local>, i: &MeetingInsight) -> Value {
    json!({
        "at": at.to_rfc3339(),
        "kind": serde_json::to_value(i.kind).unwrap_or(Value::Null),
        "content": i.content,
        "confidence": serde_json::to_value(i.confidence).unwrap_or(Value::Null),
        "participants": i.participants,
        "owner": i.owner,
        "deadline": i.deadline,
        "topic": i.topic,
        "source_meeting": i.source_meeting,
    })
}

/// Strip YAML frontmatter so the model reads the transcript, not the metadata twice.
fn body_of(content: &str) -> String {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start().to_string();
        }
    }
    content.to_string()
}

/// Truncate to a character budget with a visible marker.
pub fn truncate(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let cut: String = text.chars().take(max_chars).collect();
    let dropped = text.chars().count() - max_chars;
    format!("{cut}\n...[truncated {dropped} chars]")
}

fn expand_home(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

// ---------- Brain: bounded search and read under the knowledge root ----------

const BRAIN_MAX_FILES: usize = 30_000;
const BRAIN_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BRAIN_SCAN_BUDGET: Duration = Duration::from_secs(8);
const BRAIN_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".obsidian",
    ".trash",
    "attachments",
    "Attachments",
];

fn walk_markdown(root: &Path, out: &mut Vec<PathBuf>, deadline: Instant) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= BRAIN_MAX_FILES || Instant::now() > deadline || depth > 12 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || BRAIN_SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if ft.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
    }
}

/// Which agent CLI voice relays to: the voice override, else the assistant
/// Minutes already hands off to, else whatever is installed.
pub fn delegate_agent(config: &Config) -> Option<String> {
    for candidate in [
        config.voice_live.delegate_agent.trim(),
        config.assistant.agent.trim(),
    ] {
        if !candidate.is_empty() {
            // Resolve to a real path: Minutes can run without a login shell, so
            // a bare "claude" fails to spawn even when it is installed.
            return Some(crate::summarize::resolve_agent_path(candidate));
        }
    }
    crate::summarize::detect_agent_cli()
}

/// The two directories the prep and brief skills write to.
fn prep_roots() -> Vec<(&'static str, PathBuf)> {
    let base = Config::minutes_dir();
    vec![("prep", base.join("preps")), ("brief", base.join("briefs"))]
}

fn list_prep_artifacts() -> Value {
    let mut items: Vec<(std::time::SystemTime, Value)> = Vec::new();
    for (kind, root) in prep_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !name.ends_with(".md") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            items.push((
                modified,
                json!({
                    "name": name,
                    "kind": kind,
                    "modified": chrono::DateTime::<Local>::from(modified)
                        .format("%Y-%m-%d")
                        .to_string(),
                }),
            ));
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.0));
    let files: Vec<Value> = items.into_iter().map(|(_, v)| v).collect();
    json!({ "count": files.len(), "files": files })
}

fn read_prep_artifact(name: &str, max_chars: usize) -> Result<Value, String> {
    // A bare file name only. Anything with a separator is a traversal attempt.
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("name must be a plain file name from list_preps".into());
    }
    for (kind, root) in prep_roots() {
        let candidate = root.join(name);
        let Ok(resolved) = candidate.canonicalize() else {
            continue;
        };
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        if !resolved.starts_with(&root) || !resolved.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&resolved).map_err(|e| e.to_string())?;
        return Ok(json!({
            "name": name,
            "kind": kind,
            "content": truncate(body_of(&body), max_chars),
        }));
    }
    Err(format!("no prep or brief named {name}"))
}

fn brain_search(root: &Path, query: &str, limit: usize) -> Value {
    let deadline = Instant::now() + BRAIN_SCAN_BUDGET;
    let needle = query.to_lowercase();
    let mut files = Vec::new();
    walk_markdown(root, &mut files, deadline);
    let scanned = files.len();
    let mut hits: Vec<(PathBuf, std::time::SystemTime, String)> = Vec::new();
    for f in files {
        if Instant::now() > deadline {
            break;
        }
        let Ok(meta) = f.metadata() else { continue };
        if meta.len() > BRAIN_MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let lower = text.to_lowercase();
        let name_hit = f
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains(&needle))
            .unwrap_or(false);
        if let Some(pos) = lower.find(&needle) {
            hits.push((
                f,
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                snippet_around(&text, pos, needle.len()),
            ));
        } else if name_hit {
            hits.push((
                f,
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                String::new(),
            ));
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.1));
    let total = hits.len();
    let rows: Vec<Value> = hits
        .into_iter()
        .take(limit)
        .map(|(f, modified, snippet)| {
            let rel = f.strip_prefix(root).unwrap_or(&f).to_string_lossy().to_string();
            let modified: chrono::DateTime<Local> = modified.into();
            json!({"path": rel, "modified": modified.format("%Y-%m-%d").to_string(), "snippet": snippet})
        })
        .collect();
    json!({"root": root, "scanned_files": scanned, "total_matches": total, "hits": rows})
}

fn snippet_around(text: &str, byte_pos: usize, needle_len: usize) -> String {
    let start = text[..byte_pos]
        .char_indices()
        .rev()
        .nth(90)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end_base = (byte_pos + needle_len).min(text.len());
    let end = text[end_base..]
        .char_indices()
        .nth(90)
        .map(|(i, _)| end_base + i)
        .unwrap_or(text.len());
    text[start..end].replace('\n', " ").trim().to_string()
}

fn brain_read(root: &Path, rel: &str, max_chars: usize) -> Result<Value, String> {
    let candidate = root.join(rel.trim_start_matches('/'));
    let resolved = candidate
        .canonicalize()
        .map_err(|_| format!("no such file in the knowledge base: {rel}"))?;
    if !resolved.starts_with(root) {
        return Err("path is outside the knowledge root".into());
    }
    let meta = resolved.metadata().map_err(|e| e.to_string())?;
    if meta.is_dir() {
        let mut entries: Vec<String> = std::fs::read_dir(&resolved)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| {
                let mut n = e.file_name().to_string_lossy().to_string();
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    n.push('/');
                }
                n
            })
            .collect();
        entries.sort();
        entries.truncate(120);
        return Ok(json!({"path": rel, "directory": true, "entries": entries}));
    }
    if meta.len() > BRAIN_MAX_FILE_BYTES {
        return Err("file too large to read aloud".into());
    }
    let text = std::fs::read_to_string(&resolved).map_err(|e| e.to_string())?;
    let modified: chrono::DateTime<Local> = meta.modified().unwrap_or(std::time::UNIX_EPOCH).into();
    Ok(json!({
        "path": rel,
        "modified": modified.format("%Y-%m-%d").to_string(),
        "chars": text.chars().count(),
        "content": truncate(text, max_chars),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_live::names::{KnownPerson, NameIndex};

    fn ctx_with(brain: Option<PathBuf>) -> ToolContext {
        let mut config = Config::default();
        config.voice_live.max_tool_chars = 2_000;
        let names = Arc::new(NameIndex::from_parts(
            vec![KnownPerson {
                name: "Dan Benamoz".into(),
                meetings: 3,
                last_seen: "2026-08-25".into(),
            }],
            vec![],
        ));
        ToolContext {
            config,
            names,
            brain_root: brain,
            max_chars: 2_000,
        }
    }

    #[test]
    fn declarations_are_valid_and_gated() {
        let d = ctx_with(None).declarations();
        let names: Vec<&str> = d.iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"list_meetings"));
        assert!(names.contains(&"resolve_person"));
        assert!(
            !names.contains(&"search_brain"),
            "brain tools must be hidden without a root"
        );
        assert!(
            !names
                .iter()
                .any(|n| n.contains("recording") || n.contains("dictation")),
            "no capture control by voice"
        );
        for v in &d {
            assert_eq!(v["parameters"]["type"], "object");
            assert_eq!(v["behavior"], "NON_BLOCKING");
            assert!(v["parameters"]["properties"].is_object());
        }
    }

    #[test]
    fn unknown_tool_is_a_spoken_error_not_a_panic() {
        let out = ctx_with(None).execute("launch_missiles", &json!({}));
        assert!(out.is_error);
        assert!(out.text.contains("unknown tool"));
    }

    #[test]
    fn resolve_person_runs_locally() {
        let out = ctx_with(None).execute("resolve_person", &json!({"name": "Dan Bennimos"}));
        assert!(!out.is_error);
        assert!(out.text.contains("Dan Benamoz"));
    }

    #[test]
    fn brain_search_and_read_stay_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("people")).unwrap();
        std::fs::write(
            root.join("people/dan.md"),
            "# Dan Benamoz\nMentor and partner in RxVIP.\n",
        )
        .unwrap();
        std::fs::write(root.join("people/other.md"), "nothing here\n").unwrap();
        let ctx = ctx_with(Some(root.clone()));
        let d = ctx.declarations();
        assert!(d.iter().any(|v| v["name"] == "search_brain"));

        let out = ctx.execute("search_brain", &json!({"query": "rxvip"}));
        let v: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(v["total_matches"], 1);
        assert_eq!(v["hits"][0]["path"], "people/dan.md");
        assert!(v["hits"][0]["snippet"].as_str().unwrap().contains("RxVIP"));

        let out = ctx.execute("read_brain", &json!({"path": "people/dan.md"}));
        assert!(!out.is_error);
        assert!(out.text.contains("Mentor"));

        let out = ctx.execute("read_brain", &json!({"path": "../../etc/passwd"}));
        assert!(out.is_error);
    }

    #[test]
    fn truncation_marks_dropped_chars() {
        let t = truncate("x".repeat(50), 10);
        assert!(t.starts_with("xxxxxxxxxx\n...[truncated 40 chars]"));
    }

    #[test]
    fn the_screen_tool_blocks_so_the_answer_is_not_a_frame_behind() {
        let mut config = Config::default();
        config.voice_live.screen_on_request = true;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let screen = ctx
            .declarations()
            .into_iter()
            .find(|d| d["name"] == "look_at_screen")
            .expect("look_at_screen should be declared");
        assert_eq!(screen["behavior"], "BLOCKING");
    }

    #[test]
    fn a_captured_frame_reports_no_app_name_to_parrot() {
        let mut config = Config::default();
        config.voice_live.screen_on_request = true;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let out = ctx.execute("look_at_screen", &json!({}));
        // Whether or not capture works here, the result never names an app: the
        // model reported that name instead of reading the image.
        assert!(!out.text.contains("frontmost_app"));
    }

    #[test]
    fn delegation_prefers_the_voice_override_then_the_assistant_agent() {
        let mut config = Config::default();
        config.assistant.agent = "codex".into();
        assert!(delegate_agent(&config).unwrap().ends_with("codex"));
        config.voice_live.delegate_agent = "opencode".into();
        assert!(delegate_agent(&config).unwrap().ends_with("opencode"));
    }

    #[test]
    fn prep_names_cannot_escape_their_directory() {
        for bad in ["../config.toml", "../../.ssh/id_rsa", "sub/dir.md", ".."] {
            let err = read_prep_artifact(bad, 500).unwrap_err();
            assert!(
                err.contains("plain file name") || err.contains("no prep or brief"),
                "{bad} gave {err}"
            );
        }
    }

    #[test]
    fn listing_preps_without_the_directories_is_empty_not_an_error() {
        let v = list_prep_artifacts();
        assert!(v["files"].is_array());
        assert_eq!(
            v["count"].as_u64().unwrap(),
            v["files"].as_array().unwrap().len() as u64
        );
    }

    #[test]
    fn look_at_screen_is_refused_while_the_switch_is_off() {
        let mut config = Config::default();
        config.voice_live.screen_on_request = false;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let out = ctx.execute("look_at_screen", &json!({}));
        assert!(out.is_error);
        assert!(out.image.is_none());
        assert!(out.text.contains("screen_on_request"));
    }

    #[test]
    fn body_strips_frontmatter() {
        assert_eq!(body_of("---\ntitle: t\n---\nhello"), "hello");
        assert_eq!(body_of("no frontmatter"), "no frontmatter");
    }
}
