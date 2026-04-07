//! Command palette registry.
//!
//! This module is the **single source of truth** for the commands exposed
//! through Minutes' command palette (⌘⇧K in the Tauri desktop app). It is
//! intentionally describe-only — it knows what commands exist, what they are
//! called, what input they take, and when they are visible. It does not
//! execute anything. Execution lives in the Tauri dispatch layer, which owns
//! the app state, window handles, and event channels that commands need.
//!
//! # Why a static registry
//!
//! v1 commands are known at compile time. A `&'static [Command]` slice is
//! faster, simpler, and easier to test than a trait-object registry. If a
//! future version needs plugin-contributed commands, a dynamic registry can
//! live alongside this one rather than replacing it.
//!
//! # ActionId is `const`-constructible
//!
//! The first draft of this module had an `ActionIdTemplate` enum mirroring
//! `ActionId` with the parameters stripped, under the false assumption that a
//! `&'static [Command]` slice couldn't hold parameterized variants. Codex's
//! adversarial review (P0, 2026-04-07) caught the mistake: `Option::None` is
//! a unit variant that allocates nothing, so `ActionId::SearchTranscripts {
//! query: None }` is trivially `const`. The template layer was dead weight
//! and has been removed. A second codex review (slice 1, 2026-04-07) then
//! caught that a hand-maintained `ActionRequest` mirror in the Tauri crate
//! recreated the same drift at the FFI boundary — the exhaustive match only
//! existed in `#[cfg(test)]`. That mirror is also gone. `ActionId` itself is
//! now the FFI type, derives `Serialize`/`Deserialize` with `#[serde(tag =
//! "id")]`, and is the one source of truth for every consumer.
//!
//! # Design invariants
//!
//! - `ActionId` is an enum that carries its own parameters where needed. At the
//!   registry level every variant is stored in its "empty" form
//!   (parameter-carrying variants use `None`); the dispatch layer inflates
//!   them with real values pulled from the palette input.
//! - Registry entries are describe-only. Never add a `Command` whose dispatcher
//!   does not yet exist — see finding 3 in the PLAN's findings log.
//! - `visible_when` is still coarse on purpose. The palette is not a rules
//!   engine and not a menu. If a command doesn't satisfy its predicate it is
//!   hidden entirely, not grayed out.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable identifier for a palette command. **This is the FFI type** between
/// the palette UI and the Tauri dispatcher. Each variant carries its own
/// parameters as a struct body so serde can tag it internally. The dispatch
/// layer matches on this enum exhaustively — a new variant here is a compile
/// error in any consumer that doesn't cover it. That is the compile-time
/// coupling the first slice was missing.
///
/// # JSON shape
///
/// `#[serde(tag = "id", rename_all = "kebab-case")]` means a request looks
/// like:
///
/// ```json
/// { "id": "start-recording" }
/// { "id": "add-note", "text": "meeting started" }
/// { "id": "search-transcripts", "query": "pricing" }
/// ```
///
/// The `id` field doubles as both the serde tag and the stable telemetry
/// key. Once v1 ships, these strings are part of the public contract — user
/// recent-list files and CLI logs reference them. Adding a new variant is
/// fine; renaming an existing one requires a migration.
///
/// # Slice 1
///
/// Ships exactly the variants that have a concrete backing executor in
/// `palette_dispatch.rs`. Adding a variant here without a dispatch arm is a
/// compile error in the Tauri crate (see PLAN finding 5). Future slices add:
/// OpenTodayMeetings (date filter), ReprocessCurrentMeeting (pipeline
/// rerun), RenameCurrentMeeting (rename + frontmatter update).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "id", rename_all = "kebab-case")]
pub enum ActionId {
    // Recording
    StartRecording,
    StopRecording,
    AddNote {
        /// Free text the user entered for the note.
        #[serde(default)]
        text: Option<String>,
    },
    StartLiveTranscript,
    StopLiveTranscript,
    ReadLiveTranscript,

    // Dictation
    StartDictation,
    StopDictation,

    // Navigation
    OpenLatestMeeting,
    OpenMeetingsFolder,
    OpenMemosFolder,
    OpenAssistantWorkspace,
    ShowUpcomingMeetings,

    // Search / research — the optional payload is the inline query captured
    // from the palette input, so `> search pricing` can execute in one step.
    SearchTranscripts {
        #[serde(default)]
        query: Option<String>,
    },
    ResearchTopic {
        #[serde(default)]
        query: Option<String>,
    },
    FindOpenActionItems,
    FindRecentDecisions,

    // Meeting-context actions (only visible with current_meeting)
    CopyMeetingMarkdown,
}

impl ActionId {
    /// Stable kebab-case string used for logging and telemetry. Matches the
    /// serde tag exactly — a test asserts this. Do **not** use this as the
    /// recent-list key on its own; recents must persist the full serialized
    /// ActionId, not just the id (see finding 7 in the PLAN's findings log).
    pub fn as_kebab(&self) -> &'static str {
        match self {
            ActionId::StartRecording => "start-recording",
            ActionId::StopRecording => "stop-recording",
            ActionId::AddNote { .. } => "add-note",
            ActionId::StartLiveTranscript => "start-live-transcript",
            ActionId::StopLiveTranscript => "stop-live-transcript",
            ActionId::ReadLiveTranscript => "read-live-transcript",
            ActionId::StartDictation => "start-dictation",
            ActionId::StopDictation => "stop-dictation",
            ActionId::OpenLatestMeeting => "open-latest-meeting",
            ActionId::OpenMeetingsFolder => "open-meetings-folder",
            ActionId::OpenMemosFolder => "open-memos-folder",
            ActionId::OpenAssistantWorkspace => "open-assistant-workspace",
            ActionId::ShowUpcomingMeetings => "show-upcoming-meetings",
            ActionId::SearchTranscripts { .. } => "search-transcripts",
            ActionId::ResearchTopic { .. } => "research-topic",
            ActionId::FindOpenActionItems => "find-open-action-items",
            ActionId::FindRecentDecisions => "find-recent-decisions",
            ActionId::CopyMeetingMarkdown => "copy-meeting-markdown",
        }
    }
}

/// Declares what input the palette UI must gather from the user before the
/// dispatcher runs this command. The registry lists this per-command so the UI
/// knows whether to show an inline text field, a second-step prompt, or
/// nothing at all. See finding 2 in the PLAN's findings log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// Parameter-less action. Invoking the row runs the command immediately.
    None,
    /// Text captured inline from the palette input box (e.g. `> search foo`).
    /// Empty input is valid and dispatched as `None` on the hydrated variant.
    InlineQuery,
    /// Multi-line free text gathered in a follow-up prompt modal (e.g. a note
    /// body). Distinct from `InlineQuery` because the UI workflow is different.
    PromptText,
}

/// Top-level grouping shown in the palette. Section ordering defines the
/// order groups appear when the user's query is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recording,
    Dictation,
    Navigation,
    Search,
}

/// Predicate that decides whether a command should be offered for the current
/// app state. A command is visible iff **all** `requires` flags are true and
/// **no** `forbids` flags are true. Flag composition replaces the earlier
/// single-variant enum because the single-variant model couldn't express
/// "meeting open AND idle" or handle dictation as a conflicting mode (see
/// finding 6 in the PLAN's findings log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Visibility {
    pub requires: StateFlags,
    pub forbids: StateFlags,
}

impl Visibility {
    /// Shorthand: always visible.
    pub const fn always() -> Self {
        Self {
            requires: StateFlags::empty(),
            forbids: StateFlags::empty(),
        }
    }

    /// Shorthand: only visible when no audio session is active.
    pub const fn when_idle() -> Self {
        Self {
            requires: StateFlags::empty(),
            forbids: StateFlags::ANY_SESSION,
        }
    }

    /// Shorthand: only visible during a normal recording.
    pub const fn when_recording() -> Self {
        Self {
            requires: StateFlags::RECORDING,
            forbids: StateFlags::empty(),
        }
    }

    /// Shorthand: only visible during a live transcript session.
    pub const fn when_live_transcript() -> Self {
        Self {
            requires: StateFlags::LIVE_TRANSCRIPT,
            forbids: StateFlags::empty(),
        }
    }

    /// Shorthand: only visible during a dictation session.
    pub const fn when_dictation() -> Self {
        Self {
            requires: StateFlags::DICTATION,
            forbids: StateFlags::empty(),
        }
    }
}

/// Bitmask of mutually-observable app states. Kept as a hand-rolled bitflag
/// struct to avoid pulling in the `bitflags` crate for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StateFlags(u8);

impl StateFlags {
    pub const RECORDING: StateFlags = StateFlags(1 << 0);
    pub const LIVE_TRANSCRIPT: StateFlags = StateFlags(1 << 1);
    pub const DICTATION: StateFlags = StateFlags(1 << 2);
    pub const MEETING_OPEN: StateFlags = StateFlags(1 << 3);

    /// Any long-running audio session.
    pub const ANY_SESSION: StateFlags =
        StateFlags(Self::RECORDING.0 | Self::LIVE_TRANSCRIPT.0 | Self::DICTATION.0);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Snapshot of the app state the palette uses to filter commands.
///
/// Built once per palette open by the Tauri dispatch layer, which merges two
/// sources (see finding 4 in the PLAN's findings log):
///
/// 1. **Backend state** (recording/live/dictation) — resolved using the same
///    pid-aware logic as `tauri::commands::cmd_status`, not just `AppState`
///    atomic flags, because external processes (CLI) can also own these PIDs.
/// 2. **UI state** (`current_meeting`, `selected_text`) — passed by the
///    frontend because only the UI knows which meeting is open in the
///    assistant webview and whether the user has text selected.
///
/// This module never constructs a `Context` on its own. Tests build them
/// directly for filter assertions.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub flags: StateFlags,
    pub current_meeting: Option<PathBuf>,
    pub selected_text: Option<String>,
}

impl Context {
    /// True when no long-running audio session is active.
    pub fn is_idle(&self) -> bool {
        !self.flags.intersects(StateFlags::ANY_SESSION)
    }

    /// Compute the full flag set including `MEETING_OPEN` for predicate
    /// evaluation. Kept private because callers should use `is_visible` or
    /// `visible_commands` rather than poking at flags directly.
    fn effective_flags(&self) -> StateFlags {
        let mut f = self.flags;
        if self.current_meeting.is_some() {
            f = f.union(StateFlags::MEETING_OPEN);
        }
        f
    }
}

/// A single action a user can invoke from the palette.
///
/// Every field is `&'static` or `Copy` so the registry can live in a `const`
/// slice. Parameterized actions carry their args through `ActionId`, not
/// through this struct.
#[derive(Debug, Clone)]
pub struct Command {
    /// The action this row triggers. For parameterized variants, the
    /// registry stores the "empty" form (e.g. `SearchTranscripts(None)`); the
    /// dispatcher hydrates it from palette input at invocation time.
    pub id: ActionId,
    /// Human-facing title shown in the palette row.
    pub title: &'static str,
    /// Secondary description shown under the title when the row is focused.
    pub description: &'static str,
    /// Extra search tokens beyond `title`/`description`. Synonyms only.
    pub keywords: &'static [&'static str],
    pub section: Section,
    pub visibility: Visibility,
    pub input: InputKind,
}

/// The full registry of v1 palette commands. Order matters — it is the
/// default ordering shown when the user opens the palette with an empty
/// query. Sections are interleaved; within a section, the most common action
/// comes first.
///
/// Every new command added after v1 must earn its slot and must have a
/// concrete dispatcher before it ships. If this slice grows past ~30 entries
/// without a deliberate decision, the palette is becoming a menu, not a
/// launchpad.
pub fn commands() -> Vec<Command> {
    vec![
        // ── Recording ────────────────────────────────────────────────
        Command {
            id: ActionId::StartRecording,
            title: "Start recording",
            description: "Begin capturing audio to a new meeting",
            keywords: &["record", "capture", "meeting", "begin", "transcribe"],
            section: Section::Recording,
            visibility: Visibility::when_idle(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::StopRecording,
            title: "Stop recording",
            description: "Finish the current recording and process it",
            keywords: &["stop", "finish", "end"],
            section: Section::Recording,
            visibility: Visibility::when_recording(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::AddNote { text: None },
            title: "Add note to current recording",
            description: "Insert a timestamped note into the active session",
            keywords: &["annotate", "mark", "highlight", "remember"],
            section: Section::Recording,
            visibility: Visibility::when_recording(),
            input: InputKind::PromptText,
        },
        Command {
            id: ActionId::StartLiveTranscript,
            title: "Start live transcript",
            description: "Real-time transcription for mid-meeting AI coaching",
            keywords: &["live", "realtime", "coaching", "stream"],
            section: Section::Recording,
            visibility: Visibility::when_idle(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::StopLiveTranscript,
            title: "Stop live transcript",
            description: "End the live transcript session",
            keywords: &["stop", "end", "live"],
            section: Section::Recording,
            visibility: Visibility::when_live_transcript(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::ReadLiveTranscript,
            title: "Read live transcript",
            description: "Show the current live session's text",
            keywords: &["read", "view", "show", "live"],
            section: Section::Recording,
            visibility: Visibility::when_live_transcript(),
            input: InputKind::None,
        },
        // ── Dictation ────────────────────────────────────────────────
        Command {
            id: ActionId::StartDictation,
            title: "Start dictation",
            description: "Speak → clipboard + daily note",
            keywords: &["dictate", "speech", "voice", "type"],
            section: Section::Dictation,
            visibility: Visibility::when_idle(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::StopDictation,
            title: "Stop dictation",
            description: "End the dictation session",
            keywords: &["stop", "end", "dictate"],
            section: Section::Dictation,
            visibility: Visibility::when_dictation(),
            input: InputKind::None,
        },
        // ── Navigation ───────────────────────────────────────────────
        Command {
            id: ActionId::OpenLatestMeeting,
            title: "Open latest meeting",
            description: "Jump to the most recently processed meeting",
            keywords: &["last", "recent", "open"],
            section: Section::Navigation,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::ShowUpcomingMeetings,
            title: "Show upcoming meetings",
            description: "Calendar-aware preview of what's next",
            keywords: &["calendar", "next", "upcoming", "schedule"],
            section: Section::Navigation,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::OpenMeetingsFolder,
            title: "Open meetings folder",
            description: "Reveal ~/meetings in Finder",
            keywords: &["folder", "finder", "files"],
            section: Section::Navigation,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::OpenMemosFolder,
            title: "Open memos folder",
            description: "Reveal ~/meetings/memos in Finder",
            keywords: &["memo", "voice memo", "folder", "finder"],
            section: Section::Navigation,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        // ── Search / research ────────────────────────────────────────
        Command {
            id: ActionId::SearchTranscripts { query: None },
            title: "Search transcripts…",
            description: "Full-text search across meetings and memos",
            keywords: &["find", "grep", "lookup"],
            section: Section::Search,
            visibility: Visibility::always(),
            input: InputKind::InlineQuery,
        },
        Command {
            id: ActionId::ResearchTopic { query: None },
            title: "Research topic…",
            description: "Cross-meeting research with decisions and follow-ups",
            keywords: &["research", "topic", "cross-meeting"],
            section: Section::Search,
            visibility: Visibility::always(),
            input: InputKind::InlineQuery,
        },
        Command {
            id: ActionId::FindOpenActionItems,
            title: "Find open action items",
            description: "Unresolved commitments across all meetings",
            keywords: &["action", "todo", "tasks", "followup"],
            section: Section::Search,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::FindRecentDecisions,
            title: "Find recent decisions",
            description: "All recorded decisions, newest first",
            keywords: &["decisions", "choices", "recent"],
            section: Section::Search,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        // ── Assistant / meeting-context actions ──────────────────────
        // OpenAssistantWorkspace is always visible; CopyMeetingMarkdown
        // requires a meeting to be open in the assistant (the UI passes
        // `current_meeting` in PaletteUiContext).
        Command {
            id: ActionId::OpenAssistantWorkspace,
            title: "Open assistant workspace",
            description: "Reveal the assistant's current meeting folder",
            keywords: &["ai", "assistant", "chat", "claude", "workspace"],
            section: Section::Navigation,
            visibility: Visibility::always(),
            input: InputKind::None,
        },
        Command {
            id: ActionId::CopyMeetingMarkdown,
            title: "Copy meeting markdown",
            description: "Copy the current meeting's markdown to clipboard",
            keywords: &["copy", "clipboard", "export"],
            section: Section::Search,
            visibility: Visibility {
                requires: StateFlags::MEETING_OPEN,
                forbids: StateFlags::empty(),
            },
            input: InputKind::None,
        },
    ]
}

/// Return the commands that are visible for the given app state. Ordering is
/// preserved from `commands()`.
pub fn visible_commands(ctx: &Context) -> Vec<Command> {
    let flags = ctx.effective_flags();
    commands()
        .into_iter()
        .filter(|c| is_visible(c.visibility, flags))
        .collect()
}

/// Pure predicate evaluation against a resolved flag set. Extracted for
/// direct testing without constructing `Command` values.
pub fn is_visible(v: Visibility, flags: StateFlags) -> bool {
    flags.contains(v.requires) && !flags.intersects(v.forbids)
}

// ──────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_ctx() -> Context {
        Context::default()
    }

    fn recording_ctx() -> Context {
        Context {
            flags: StateFlags::RECORDING,
            ..Context::default()
        }
    }

    fn live_ctx() -> Context {
        Context {
            flags: StateFlags::LIVE_TRANSCRIPT,
            ..Context::default()
        }
    }

    fn dictation_ctx() -> Context {
        Context {
            flags: StateFlags::DICTATION,
            ..Context::default()
        }
    }

    fn meeting_open_idle_ctx() -> Context {
        Context {
            current_meeting: Some(PathBuf::from("/tmp/fake-meeting.md")),
            ..Context::default()
        }
    }

    fn kebabs(cmds: &[Command]) -> Vec<&'static str> {
        cmds.iter().map(|c| c.id.as_kebab()).collect()
    }

    #[test]
    fn registry_has_seed_commands() {
        let all = commands();
        // Slice 1 ships the 18 commands with concrete dispatch arms in
        // tauri/src-tauri/src/palette_dispatch.rs. New commands MUST land
        // with a backing executor — no scaffolding entries.
        assert_eq!(
            all.len(),
            18,
            "slice 1 should have exactly 18 commands with backing dispatchers"
        );
    }

    #[test]
    fn all_action_ids_have_unique_kebab() {
        let mut seen = std::collections::HashSet::new();
        for cmd in commands() {
            let kebab = cmd.id.as_kebab();
            assert!(seen.insert(kebab), "duplicate kebab id: {}", kebab);
        }
    }

    #[test]
    fn all_titles_are_non_empty() {
        for cmd in commands() {
            assert!(!cmd.title.is_empty(), "empty title for {:?}", cmd.id);
            assert!(
                !cmd.description.is_empty(),
                "empty description for {:?}",
                cmd.id
            );
        }
    }

    #[test]
    fn parameterized_commands_are_stored_in_empty_form() {
        // Registry entries must hold `None` for parameterized variants; real
        // input is injected at dispatch time.
        let all = commands();
        let search = all
            .iter()
            .find(|c| matches!(c.id, ActionId::SearchTranscripts { .. }))
            .unwrap();
        assert_eq!(search.id, ActionId::SearchTranscripts { query: None });
        assert_eq!(search.input, InputKind::InlineQuery);

        let research = all
            .iter()
            .find(|c| matches!(c.id, ActionId::ResearchTopic { .. }))
            .unwrap();
        assert_eq!(research.id, ActionId::ResearchTopic { query: None });
        assert_eq!(research.input, InputKind::InlineQuery);

        let add_note = all
            .iter()
            .find(|c| matches!(c.id, ActionId::AddNote { .. }))
            .unwrap();
        assert_eq!(add_note.id, ActionId::AddNote { text: None });
        assert_eq!(add_note.input, InputKind::PromptText);
    }

    #[test]
    fn input_kind_set_for_every_parameter_bearing_action() {
        // If a command's ActionId variant carries a payload, its InputKind
        // must not be None. Prevents silent regressions.
        for cmd in commands() {
            let parameterized = matches!(
                cmd.id,
                ActionId::SearchTranscripts { .. }
                    | ActionId::ResearchTopic { .. }
                    | ActionId::AddNote { .. }
            );
            if parameterized {
                assert_ne!(
                    cmd.input,
                    InputKind::None,
                    "parameterized command {} must not have InputKind::None",
                    cmd.id.as_kebab()
                );
            }
        }
    }

    #[test]
    fn copy_meeting_markdown_only_when_meeting_open() {
        let idle = visible_commands(&idle_ctx());
        assert!(!kebabs(&idle).contains(&"copy-meeting-markdown"));

        let meeting = visible_commands(&meeting_open_idle_ctx());
        assert!(kebabs(&meeting).contains(&"copy-meeting-markdown"));
    }

    #[test]
    fn action_id_serializes_with_id_tag() {
        // The serde tag IS the public contract. If this changes, every
        // user's recent-list file silently breaks.
        let v = serde_json::to_value(&ActionId::StartRecording).unwrap();
        assert_eq!(v, serde_json::json!({ "id": "start-recording" }));

        let v = serde_json::to_value(&ActionId::AddNote {
            text: Some("hello".into()),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({ "id": "add-note", "text": "hello" }));

        let v = serde_json::to_value(&ActionId::SearchTranscripts {
            query: Some("pricing".into()),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "id": "search-transcripts", "query": "pricing" })
        );
    }

    #[test]
    fn action_id_deserializes_from_id_tag() {
        let id: ActionId =
            serde_json::from_value(serde_json::json!({ "id": "start-recording" })).unwrap();
        assert_eq!(id, ActionId::StartRecording);

        let id: ActionId = serde_json::from_value(
            serde_json::json!({ "id": "search-transcripts", "query": "pricing" }),
        )
        .unwrap();
        assert_eq!(
            id,
            ActionId::SearchTranscripts {
                query: Some("pricing".into())
            }
        );

        // Missing optional field deserializes to None.
        let id: ActionId = serde_json::from_value(serde_json::json!({ "id": "add-note" })).unwrap();
        assert_eq!(id, ActionId::AddNote { text: None });
    }

    #[test]
    fn kebab_matches_serde_tag_for_every_variant() {
        // For each registry entry, serialize → re-read the "id" field →
        // compare to as_kebab(). If anyone renames a variant in either
        // direction, this fails.
        for cmd in commands() {
            let json = serde_json::to_value(&cmd.id).unwrap();
            let serialized_id = json
                .get("id")
                .and_then(|v| v.as_str())
                .expect("every action serializes with an id field");
            assert_eq!(
                serialized_id,
                cmd.id.as_kebab(),
                "as_kebab() drifted from serde tag for {:?}",
                cmd.id
            );
        }
    }

    #[test]
    fn idle_hides_stop_commands() {
        let visible = visible_commands(&idle_ctx());
        let ids = kebabs(&visible);
        assert!(ids.contains(&"start-recording"));
        assert!(ids.contains(&"start-dictation"));
        assert!(ids.contains(&"start-live-transcript"));
        assert!(!ids.contains(&"stop-recording"));
        assert!(!ids.contains(&"stop-dictation"));
        assert!(!ids.contains(&"stop-live-transcript"));
        assert!(!ids.contains(&"add-note"));
    }

    #[test]
    fn recording_swaps_start_for_stop_and_exposes_add_note() {
        let visible = visible_commands(&recording_ctx());
        let ids = kebabs(&visible);
        assert!(!ids.contains(&"start-recording"));
        assert!(ids.contains(&"stop-recording"));
        assert!(ids.contains(&"add-note"));
        // Start-dictation is forbidden while any session is active.
        assert!(!ids.contains(&"start-dictation"));
    }

    #[test]
    fn live_transcript_exposes_stop_and_read() {
        let visible = visible_commands(&live_ctx());
        let ids = kebabs(&visible);
        assert!(ids.contains(&"stop-live-transcript"));
        assert!(ids.contains(&"read-live-transcript"));
        assert!(!ids.contains(&"start-live-transcript"));
    }

    #[test]
    fn dictation_exposes_stop_not_start() {
        let visible = visible_commands(&dictation_ctx());
        let ids = kebabs(&visible);
        assert!(ids.contains(&"stop-dictation"));
        assert!(!ids.contains(&"start-dictation"));
        // Recording starts are also blocked.
        assert!(!ids.contains(&"start-recording"));
    }

    #[test]
    fn is_idle_is_true_only_with_no_session_flags() {
        assert!(idle_ctx().is_idle());
        assert!(!recording_ctx().is_idle());
        assert!(!live_ctx().is_idle());
        assert!(!dictation_ctx().is_idle());
        // meeting open alone is still idle — MEETING_OPEN is not a session
        assert!(meeting_open_idle_ctx().is_idle());
    }

    #[test]
    fn state_flags_union_and_contains() {
        let both = StateFlags::RECORDING.union(StateFlags::MEETING_OPEN);
        assert!(both.contains(StateFlags::RECORDING));
        assert!(both.contains(StateFlags::MEETING_OPEN));
        assert!(!both.contains(StateFlags::LIVE_TRANSCRIPT));
        assert!(both.intersects(StateFlags::ANY_SESSION));
    }

    #[test]
    fn kebab_ids_are_stable_strings() {
        // If any of these strings change, we break user recent-list files.
        // Add new ids; do not rename existing ones without a migration.
        assert_eq!(ActionId::StartRecording.as_kebab(), "start-recording");
        assert_eq!(ActionId::StopRecording.as_kebab(), "stop-recording");
        assert_eq!(ActionId::StartDictation.as_kebab(), "start-dictation");
        assert_eq!(ActionId::StopDictation.as_kebab(), "stop-dictation");
        assert_eq!(
            ActionId::SearchTranscripts { query: None }.as_kebab(),
            "search-transcripts"
        );
        assert_eq!(
            ActionId::SearchTranscripts {
                query: Some("x".into())
            }
            .as_kebab(),
            "search-transcripts"
        );
        assert_eq!(
            ActionId::AddNote {
                text: Some("hi".into())
            }
            .as_kebab(),
            "add-note"
        );
        assert_eq!(
            ActionId::ReadLiveTranscript.as_kebab(),
            "read-live-transcript"
        );
        assert_eq!(
            ActionId::ResearchTopic { query: None }.as_kebab(),
            "research-topic"
        );
        assert_eq!(
            ActionId::CopyMeetingMarkdown.as_kebab(),
            "copy-meeting-markdown"
        );
        assert_eq!(
            ActionId::OpenAssistantWorkspace.as_kebab(),
            "open-assistant-workspace"
        );
    }
}
