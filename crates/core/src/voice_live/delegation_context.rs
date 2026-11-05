//! Ephemeral context prepared by the host while voice events arrive. No I/O,
//! inference, archive access or authority. Only the reviewed snapshot leaves
//! this process, through the existing local-agent delegation path.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(super) struct MeetingSource {
    pub path: String,
    pub sha256: String,
}

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
const MAX_ENTRIES: usize = 16;
const MAX_ENTRY_BYTES: usize = 1_024;
pub(super) const MAX_CONTEXT_BYTES: usize = 8_192;
pub(super) const CONTEXT_KEY: &str = "minutes_context";

#[derive(Debug, Clone, Serialize)]
pub(super) struct Entry {
    sequence: u64,
    kind: &'static str,
    source: String,
    text: String,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    meeting_source: Option<MeetingSource>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Snapshot {
    pub v: u8,
    pub session_epoch: u64,
    pub revision: u64,
    pub active: bool,
    pub incomplete: bool,
    pub truncated: bool,
    pub entries: VecDeque<Entry>,
}

impl Snapshot {
    pub fn value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("context contains only bounded text and integers")
    }

    pub fn bytes(&self) -> usize {
        self.value().to_string().len()
    }
}

struct Context {
    snapshot: Snapshot,
    user: String,
    assistant: String,
    user_truncated: bool,
    assistant_truncated: bool,
    sequence: u64,
}

impl Context {
    fn new() -> Self {
        Self {
            snapshot: Snapshot {
                v: 1,
                session_epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
                revision: 0,
                active: true,
                incomplete: false,
                truncated: false,
                entries: VecDeque::new(),
            },
            user: String::new(),
            assistant: String::new(),
            user_truncated: false,
            assistant_truncated: false,
            sequence: 0,
        }
    }

    fn entry(&mut self, kind: &'static str, source: &str, text: &str, truncated: bool) -> Entry {
        self.sequence = self.sequence.saturating_add(1);
        let (text, shortened) = tail(text, MAX_ENTRY_BYTES);
        Entry {
            sequence: self.sequence,
            kind,
            source: source.into(),
            text,
            truncated: truncated || shortened,
            meeting_source: None,
        }
    }

    fn append(&mut self, kind: &'static str, source: &str, text: &str, truncated: bool) {
        if !text.trim().is_empty() {
            let entry = self.entry(kind, source, text, truncated);
            self.snapshot.truncated |= entry.truncated;
            self.snapshot.entries.push_back(entry);
        }
    }

    fn finish_turn(&mut self, interrupted: bool) {
        let user = std::mem::take(&mut self.user);
        let assistant = std::mem::take(&mut self.assistant);
        self.append("user", "voice_transcript", &user, self.user_truncated);
        self.append(
            if interrupted {
                "assistant_interrupted"
            } else {
                "assistant_generated"
            },
            "voice_transcript",
            &assistant,
            self.assistant_truncated,
        );
        self.user_truncated = false;
        self.assistant_truncated = false;
    }

    fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.snapshot.clone();
        for (offset, (kind, text, truncated)) in [
            ("user_draft", &self.user, self.user_truncated),
            (
                "assistant_generated_draft",
                &self.assistant,
                self.assistant_truncated,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if !text.trim().is_empty() {
                snapshot.entries.push_back(Entry {
                    sequence: self.sequence.saturating_add(offset as u64 + 1),
                    kind,
                    source: "voice_transcript".into(),
                    text: text.clone(),
                    truncated,
                    meeting_source: None,
                });
                snapshot.truncated |= truncated;
            }
        }
        // Bound the serialized envelope too: escaping control characters can
        // multiply UTF-8 size. Prefer the newest correction over older context.
        while snapshot.entries.len() > MAX_ENTRIES || snapshot.bytes() > MAX_CONTEXT_BYTES {
            snapshot.entries.pop_front();
            snapshot.truncated = true;
        }
        snapshot
    }
}

/// Producers never wait for the tool worker. A missed update invalidates old
/// context; it must not silently hand a delegate stale instructions/evidence.
pub(super) struct Store {
    context: Mutex<Context>,
    missed_update: AtomicBool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            context: Mutex::new(Context::new()),
            missed_update: AtomicBool::new(false),
        }
    }
}

impl Store {
    pub fn invalidate(&self) {
        self.missed_update.store(true, Ordering::Release);
    }

    fn update(&self, f: impl FnOnce(&mut Context)) {
        let Ok(mut context) = self.context.try_lock() else {
            self.missed_update.store(true, Ordering::Release);
            return;
        };
        if !context.snapshot.active {
            return;
        }
        if self.missed_update.swap(false, Ordering::AcqRel) {
            context.snapshot.entries.clear();
            context.user.clear();
            context.assistant.clear();
            context.snapshot.incomplete = true;
        }
        context.snapshot.revision = context.snapshot.revision.saturating_add(1);
        f(&mut context);
        // Keep storage bounded even if no delegation ever happens.
        while context.snapshot.entries.len() > MAX_ENTRIES {
            context.snapshot.entries.pop_front();
            context.snapshot.truncated = true;
        }
    }

    pub fn user_chunk(&self, chunk: &str) {
        self.update(|c| {
            let (text, truncated) = append_tail(&c.user, chunk);
            c.user = text;
            c.user_truncated |= truncated;
        });
    }

    pub fn assistant_chunk(&self, chunk: &str) {
        self.update(|c| {
            let (text, truncated) = append_tail(&c.assistant, chunk);
            c.assistant = text;
            c.assistant_truncated |= truncated;
        });
    }

    pub fn finish_turn(&self, interrupted: bool) {
        self.update(|c| c.finish_turn(interrupted));
    }

    pub fn typed_user(&self, text: &str) {
        self.update(|c| c.append("user", "typed_input", text, false));
    }

    pub fn shared_text(&self, source: &str, text: &str) {
        self.update(|c| c.append("explicit_host_share", source, text, false));
    }

    pub fn delivered_evidence(
        &self,
        call_id: &str,
        tool: &str,
        text: &str,
        meeting_source: Option<MeetingSource>,
    ) {
        // Explicit read-only sources only. No arbitrary MCP, private /work
        // notes, action receipts, approval payloads, media or recursive agents.
        // Aggregate/knowledge results lack complete source attestations here.
        // Do not turn those cached snippets into another disclosure path.
        if !(tool == "research_public" || (tool == "get_meeting" && meeting_source.is_some())) {
            return;
        }
        let (source, _) = tail(&format!("{tool}:{call_id}"), 160);
        self.update(|c| {
            let mut entry = c.entry("tool_evidence", &source, text, false);
            entry.meeting_source = meeting_source;
            c.snapshot.truncated |= entry.truncated;
            c.snapshot.entries.push_back(entry);
        });
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        let context = self.context.try_lock().ok()?;
        if self.missed_update.load(Ordering::Acquire) || !context.snapshot.active {
            return None;
        }
        Some(context.snapshot())
    }

    pub fn close(&self) {
        // Called only by the owning session after producers have stopped.
        if let Ok(mut context) = self.context.lock() {
            context.snapshot.active = false;
            context.snapshot.entries.clear();
            context.user.clear();
            context.assistant.clear();
        }
    }
}

fn tail(text: &str, budget: usize) -> (String, bool) {
    let mut start = text.len().saturating_sub(budget);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].into(), start != 0)
}

fn append_tail(previous: &str, chunk: &str) -> (String, bool) {
    let (chunk, truncated) = tail(chunk, MAX_ENTRY_BYTES);
    let (text, shortened) = tail(&format!("{previous}{chunk}"), MAX_ENTRY_BYTES);
    (text, shortened || truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfinished_request_is_available_before_turn_complete_and_final_is_not_duplicated() {
        let store = Store::default();
        store.user_chunk("Compare the proposal. ");
        store.user_chunk("Actually, use the revised proposal and focus on delivery.");
        let draft = store.snapshot().unwrap().value();
        assert_eq!(draft["entries"].as_array().unwrap().len(), 1);
        assert_eq!(draft["entries"][0]["kind"], "user_draft");
        assert!(draft["entries"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("focus on delivery."));
        store.finish_turn(false);
        let final_context = store.snapshot().unwrap().value();
        assert_eq!(final_context["entries"].as_array().unwrap().len(), 1);
        assert_eq!(final_context["entries"][0]["kind"], "user");
        assert!(final_context["revision"].as_u64() > draft["revision"].as_u64());
    }

    #[test]
    fn snapshot_is_frozen_and_interrupted_assistant_is_distinct_from_user() {
        let store = Store::default();
        store.user_chunk("Read only.");
        store.assistant_chunk("I could send it");
        store.finish_turn(true);
        let frozen = store.snapshot().unwrap().value();
        store.typed_user("Use a different proposal.");
        assert_eq!(frozen["entries"].as_array().unwrap().len(), 2);
        assert_eq!(frozen["entries"][1]["kind"], "assistant_interrupted");
        assert!(!frozen.to_string().contains("different proposal"));
    }

    #[test]
    fn untrusted_unicode_and_escaped_text_stays_bounded_without_losing_latest_correction() {
        let store = Store::default();
        for _ in 0..100 {
            store.delivered_evidence(
                "fixture",
                "research_public",
                &"\u{0000}音".repeat(2_000),
                None,
            );
        }
        store.user_chunk(&"音".repeat(10_000));
        store.user_chunk(" Actually reject the earlier suggestion.");
        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.bytes() <= MAX_CONTEXT_BYTES);
        assert!(snapshot.truncated);
        assert!(snapshot.value().to_string().contains("Actually reject"));
        assert!(snapshot.entries.len() <= MAX_ENTRIES);
    }

    #[test]
    fn sources_are_allowlisted_and_sessions_and_shutdown_do_not_leak() {
        let store = Store::default();
        store.delivered_evidence("1", "research_public", "authorized excerpt", None);
        for tool in [
            "ask_agent",
            "manage_work",
            "hubspot__read",
            "paste_text",
            "look_at_screen",
            "get_meeting",
            "search_meetings",
            "get_person_profile",
            "read_brain",
        ] {
            store.delivered_evidence("2", tool, "private or unrelated", None);
        }
        let value = store.snapshot().unwrap().value();
        assert!(value.to_string().contains("authorized excerpt"));
        assert!(!value.to_string().contains("private or unrelated"));
        let next = Store::default().snapshot().unwrap();
        assert_ne!(value["session_epoch"], next.session_epoch);
        assert!(next.entries.is_empty());
        store.close();
        store.user_chunk("after stop");
        assert!(store.snapshot().is_none());
    }

    #[test]
    fn contention_invalidates_stale_context_instead_of_waiting() {
        let store = Store::default();
        store.user_chunk("Use the old proposal.");
        let guard = store.context.lock().unwrap();
        store.user_chunk("Correction that could not be recorded.");
        assert!(store.snapshot().is_none());
        drop(guard);
        assert!(store.snapshot().is_none());
        store.user_chunk("New request.");
        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.incomplete);
        assert!(!snapshot.value().to_string().contains("old proposal"));
    }
}
