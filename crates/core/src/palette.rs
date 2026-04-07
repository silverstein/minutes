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
    OpenLatestMeetingFromToday,
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
    /// Rename the meeting that the assistant workspace currently points
    /// at. The new title comes from the palette's PromptText input.
    RenameCurrentMeeting {
        #[serde(default, alias = "new_title", alias = "newTitle")]
        new_title: Option<String>,
    },
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
            ActionId::OpenLatestMeetingFromToday => "open-latest-meeting-from-today",
            ActionId::OpenMeetingsFolder => "open-meetings-folder",
            ActionId::OpenMemosFolder => "open-memos-folder",
            ActionId::OpenAssistantWorkspace => "open-assistant-workspace",
            ActionId::ShowUpcomingMeetings => "show-upcoming-meetings",
            ActionId::SearchTranscripts { .. } => "search-transcripts",
            ActionId::ResearchTopic { .. } => "research-topic",
            ActionId::FindOpenActionItems => "find-open-action-items",
            ActionId::FindRecentDecisions => "find-recent-decisions",
            ActionId::CopyMeetingMarkdown => "copy-meeting-markdown",
            ActionId::RenameCurrentMeeting { .. } => "rename-current-meeting",
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
            id: ActionId::OpenLatestMeetingFromToday,
            title: "Open latest meeting from today",
            description: "Jump to the most recent meeting recorded today",
            keywords: &["today", "latest", "today's"],
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
        Command {
            id: ActionId::RenameCurrentMeeting { new_title: None },
            title: "Rename current meeting",
            description: "Type a new title for the open meeting",
            keywords: &["rename", "title", "edit"],
            section: Section::Search,
            visibility: Visibility {
                requires: StateFlags::MEETING_OPEN,
                forbids: StateFlags::empty(),
            },
            input: InputKind::PromptText,
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
// Recents (forward-compatible store at ~/.minutes/palette.json)
// ──────────────────────────────────────────────────────────────

pub mod recents {
    //! Persistent recent-actions list for the command palette.
    //!
    //! Lives at `~/.minutes/palette.json`. The on-disk format is
    //! intentionally schema-versioned **and** the entry list is parsed as
    //! raw `serde_json::Value` rather than as strongly-typed `ActionId` so
    //! a downgraded older client never silently eats entries written by a
    //! newer client. See D5 of `PLAN.md.command-palette-slice-2`.
    //!
    //! # Format
    //!
    //! ```json
    //! {
    //!   "version": 1,
    //!   "entries": [
    //!     { "id": "search-transcripts", "query": "pricing" },
    //!     { "id": "start-recording" }
    //!   ]
    //! }
    //! ```
    //!
    //! - The `version` field is the writer's schema version. Files with a
    //!   version **higher** than this client understands are kept
    //!   read-only: the file is loaded but never overwritten until the
    //!   client is upgraded.
    //! - `entries` is parsed as `Vec<serde_json::Value>`. Each entry is
    //!   validated **lazily** when the caller asks for visible recents:
    //!   entries that don't deserialize into `ActionId` for this version
    //!   are *hidden* from the UI but *preserved* in memory and on disk.
    //! - Visible entries are capped at `VISIBLE_CAP`. The on-disk total
    //!   (visible + preserved-unknown) is capped at `STORAGE_CAP`.
    //! - Duplicates (full JSON equality) collapse to the most recent
    //!   position.
    //! - Atomic write via tmp + rename. Permissions `0600`.
    //!
    //! # Failure handling
    //!
    //! Parse failures are NEVER fatal. The store always returns a
    //! best-effort `RecentsStore` so the palette can keep opening even if
    //! the file is garbage. A corrupt file is renamed to
    //! `palette.json.broken` so the user can inspect it; the active
    //! recents file is not overwritten until the next successful
    //! `push_and_save` writes a fresh one.
    //!
    //! # Why a separate module
    //!
    //! Keeping recents inside `palette.rs` (not its own crate file) keeps
    //! the registry, the FFI types, and the persistence layer in one
    //! place. Future versions might split this out if recents grow
    //! beyond ~5 entries with metadata.

    use super::ActionId;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    /// Latest schema version this client knows how to write.
    pub const CURRENT_VERSION: u32 = 1;

    /// Maximum number of UI-visible (parseable) recents.
    pub const VISIBLE_CAP: usize = 5;

    /// Maximum number of entries stored on disk total. Larger than
    /// `VISIBLE_CAP` so unknown future-variant entries can ride along
    /// without being evicted by the visible cap.
    pub const STORAGE_CAP: usize = 10;

    /// Suffix appended to corrupt files for forensic recovery.
    pub const BROKEN_SUFFIX: &str = ".broken";

    /// On-disk shape. Public so tests can construct it; callers should
    /// use [`RecentsStore::load`] / [`RecentsStore::push_and_save`].
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RecentsFile {
        pub version: u32,
        #[serde(default)]
        pub entries: Vec<Value>,
    }

    impl Default for RecentsFile {
        fn default() -> Self {
            Self {
                version: CURRENT_VERSION,
                entries: Vec::new(),
            }
        }
    }

    /// In-memory wrapper around the recents file.
    ///
    /// `entries` is the raw on-disk list (preserves unknown variants).
    /// `read_only` is true when the loaded file's `version` is higher
    /// than [`CURRENT_VERSION`] — the store still serves visible recents
    /// from whatever it can parse, but [`Self::push_and_save`] is a
    /// no-op so the newer file is never overwritten.
    #[derive(Debug, Clone, Default)]
    pub struct RecentsStore {
        pub file: RecentsFile,
        pub read_only: bool,
    }

    impl RecentsStore {
        /// Default location: `~/.minutes/palette.json`.
        pub fn default_path() -> PathBuf {
            crate::config::Config::minutes_dir().join("palette.json")
        }

        /// Load recents from `path`. Always returns a best-effort store
        /// even on parse failure (the file is renamed to
        /// `<path>.broken` for forensics and the returned store is empty).
        pub fn load(path: &Path) -> Self {
            if !path.exists() {
                return Self::default();
            }
            let raw = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("[palette/recents] could not read {}: {}", path.display(), e);
                    return Self::default();
                }
            };
            match serde_json::from_str::<RecentsFile>(&raw) {
                Ok(file) => {
                    let read_only = file.version > CURRENT_VERSION;
                    if read_only {
                        tracing::info!(
                            "[palette/recents] {} has version {} > current {}; treating as read-only",
                            path.display(),
                            file.version,
                            CURRENT_VERSION
                        );
                    }
                    Self { file, read_only }
                }
                Err(e) => {
                    tracing::warn!(
                        "[palette/recents] failed to parse {}: {}; quarantining",
                        path.display(),
                        e
                    );
                    quarantine_corrupt_file(path);
                    Self::default()
                }
            }
        }

        /// Visible recents are entries that deserialize into the current
        /// `ActionId` schema. Capped at [`VISIBLE_CAP`]. Unknown-variant
        /// entries are skipped here (still preserved in `self.file`).
        pub fn visible(&self) -> Vec<ActionId> {
            self.file
                .entries
                .iter()
                .filter_map(|v| serde_json::from_value::<ActionId>(v.clone()).ok())
                .take(VISIBLE_CAP)
                .collect()
        }

        /// Push a new action to the front of the list, dedupe by full
        /// JSON equality, enforce caps, and atomically persist to `path`.
        ///
        /// Returns the serialized entry that was prepended (useful for
        /// telemetry / tests). Returns `None` if the store is read-only
        /// (newer version on disk) or serialization fails.
        pub fn push_and_save(
            &mut self,
            action: &ActionId,
            path: &Path,
        ) -> Result<Option<Value>, std::io::Error> {
            if self.read_only {
                return Ok(None);
            }

            let entry = match serde_json::to_value(action) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("[palette/recents] failed to serialize action: {}", e);
                    return Ok(None);
                }
            };

            // Dedupe (collapse to most-recent position).
            self.file.entries.retain(|existing| existing != &entry);
            self.file.entries.insert(0, entry.clone());

            // Trim while preserving unknown entries — visible (parseable
            // for current version) entries up to VISIBLE_CAP, plus any
            // unknown entries until we hit STORAGE_CAP. Unknown entries
            // are kept in their original relative order.
            self.file.entries = trim_entries(&self.file.entries);

            // Update version on every successful write so older files
            // get migrated forward without needing a separate migration
            // step.
            self.file.version = CURRENT_VERSION;

            atomic_write_json(path, &self.file)?;
            Ok(Some(entry))
        }
    }

    fn trim_entries(entries: &[Value]) -> Vec<Value> {
        let mut visible_count = 0usize;
        let mut out = Vec::with_capacity(entries.len().min(STORAGE_CAP));
        for entry in entries {
            if out.len() >= STORAGE_CAP {
                break;
            }
            let parses = serde_json::from_value::<ActionId>(entry.clone()).is_ok();
            if parses {
                if visible_count >= VISIBLE_CAP {
                    continue;
                }
                visible_count += 1;
                out.push(entry.clone());
            } else {
                out.push(entry.clone());
            }
        }
        out
    }

    fn quarantine_corrupt_file(path: &Path) {
        let broken = path.with_extension({
            // path.with_extension replaces the extension; preserve the
            // original by appending instead.
            let original = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_string();
            if original.is_empty() {
                BROKEN_SUFFIX.trim_start_matches('.').to_string()
            } else {
                format!("{}{}", original, BROKEN_SUFFIX)
            }
        });
        if let Err(e) = std::fs::rename(path, &broken) {
            tracing::warn!(
                "[palette/recents] could not quarantine {} to {}: {}",
                path.display(),
                broken.display(),
                e
            );
        }
    }

    fn atomic_write_json(path: &Path, file: &RecentsFile) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(file)
            .map_err(|e| std::io::Error::other(format!("serialize palette recents: {}", e)))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        set_recents_permissions(&tmp)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    #[cfg(unix)]
    fn set_recents_permissions(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }

    #[cfg(not(unix))]
    fn set_recents_permissions(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;
        use tempfile::TempDir;

        #[test]
        fn missing_file_loads_empty() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            let store = RecentsStore::load(&path);
            assert!(store.file.entries.is_empty());
            assert!(!store.read_only);
            assert!(store.visible().is_empty());
        }

        #[test]
        fn round_trips_known_action() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            let mut store = RecentsStore::load(&path);
            store
                .push_and_save(&ActionId::StartRecording, &path)
                .unwrap();

            let reloaded = RecentsStore::load(&path);
            let visible = reloaded.visible();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0], ActionId::StartRecording);
        }

        #[test]
        fn dedupes_to_most_recent_position() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            let mut store = RecentsStore::load(&path);
            store
                .push_and_save(&ActionId::StartRecording, &path)
                .unwrap();
            store
                .push_and_save(&ActionId::OpenLatestMeeting, &path)
                .unwrap();
            store
                .push_and_save(&ActionId::StartRecording, &path)
                .unwrap();

            let reloaded = RecentsStore::load(&path);
            let visible = reloaded.visible();
            // start-recording should be at the front, no duplicate.
            assert_eq!(
                visible,
                vec![ActionId::StartRecording, ActionId::OpenLatestMeeting]
            );
        }

        #[test]
        fn parameterized_actions_preserve_payload() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            let mut store = RecentsStore::load(&path);
            store
                .push_and_save(
                    &ActionId::SearchTranscripts {
                        query: Some("pricing".into()),
                    },
                    &path,
                )
                .unwrap();

            let reloaded = RecentsStore::load(&path);
            assert_eq!(
                reloaded.visible(),
                vec![ActionId::SearchTranscripts {
                    query: Some("pricing".into()),
                }]
            );
        }

        #[test]
        fn corrupt_file_quarantines_and_returns_empty_store() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            std::fs::write(&path, "this is not json").unwrap();

            let store = RecentsStore::load(&path);
            assert!(store.file.entries.is_empty());
            assert!(!store.read_only);

            // Corrupt file should be moved to .broken so the user can
            // inspect it. The original path should no longer exist.
            assert!(!path.exists(), "corrupt file should be quarantined");
            let broken = dir.path().join("palette.json.broken");
            assert!(
                broken.exists(),
                "expected quarantine at {}",
                broken.display()
            );
            assert_eq!(
                std::fs::read_to_string(&broken).unwrap(),
                "this is not json"
            );
        }

        #[test]
        fn unknown_variants_are_hidden_but_preserved() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            std::fs::write(
                &path,
                r#"{
                    "version": 1,
                    "entries": [
                        { "id": "future-command", "weird": true },
                        { "id": "start-recording" }
                    ]
                }"#,
            )
            .unwrap();

            let store = RecentsStore::load(&path);
            // The visible list only includes start-recording.
            assert_eq!(store.visible(), vec![ActionId::StartRecording]);
            // But the on-disk shape preserves the unknown entry.
            assert_eq!(store.file.entries.len(), 2);
            let unknown = &store.file.entries[0];
            assert_eq!(
                unknown.get("id").and_then(|v| v.as_str()),
                Some("future-command")
            );
        }

        #[test]
        fn unknown_variants_round_trip_through_push() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            std::fs::write(
                &path,
                r#"{
                    "version": 1,
                    "entries": [
                        { "id": "future-command", "payload": "still here" }
                    ]
                }"#,
            )
            .unwrap();

            let mut store = RecentsStore::load(&path);
            store
                .push_and_save(&ActionId::StopRecording, &path)
                .unwrap();

            let reloaded = RecentsStore::load(&path);
            // The unknown entry MUST still be on disk after the write.
            assert_eq!(reloaded.file.entries.len(), 2);
            let mut found_unknown = false;
            for entry in &reloaded.file.entries {
                if entry.get("id").and_then(|v| v.as_str()) == Some("future-command") {
                    assert_eq!(
                        entry.get("payload").and_then(|v| v.as_str()),
                        Some("still here")
                    );
                    found_unknown = true;
                }
            }
            assert!(found_unknown, "unknown entry must round-trip unchanged");
            // Visible recents only show the known entry.
            assert_eq!(reloaded.visible(), vec![ActionId::StopRecording]);
        }

        #[test]
        fn newer_version_files_are_read_only() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            std::fs::write(
                &path,
                r#"{
                    "version": 9999,
                    "entries": [
                        { "id": "start-recording" }
                    ]
                }"#,
            )
            .unwrap();

            let mut store = RecentsStore::load(&path);
            assert!(store.read_only);
            // Visible recents are still served from what we can parse.
            assert_eq!(store.visible(), vec![ActionId::StartRecording]);

            // push_and_save is a no-op on read-only stores.
            let original = std::fs::read_to_string(&path).unwrap();
            let pushed = store
                .push_and_save(&ActionId::StopRecording, &path)
                .unwrap();
            assert!(pushed.is_none());
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, original, "read-only file must not be overwritten");
        }

        #[test]
        fn cap_enforces_visible_and_storage_limits() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("palette.json");
            // Seed the file with 7 unknown entries — they should all be
            // preserved up to STORAGE_CAP=10.
            let mut entries = Vec::new();
            for i in 0..7 {
                entries.push(json!({ "id": format!("future-cmd-{}", i) }));
            }
            std::fs::write(
                &path,
                serde_json::to_string(&RecentsFile {
                    version: 1,
                    entries,
                })
                .unwrap(),
            )
            .unwrap();

            let mut store = RecentsStore::load(&path);
            // Push 6 known commands. The visible cap is 5; the storage
            // cap is 10. Expect: 5 known + (10 - 5 = 5) unknown after
            // trim — 7 unknowns truncated to 5.
            for action in [
                ActionId::StartRecording,
                ActionId::StopRecording,
                ActionId::OpenLatestMeeting,
                ActionId::ShowUpcomingMeetings,
                ActionId::OpenMeetingsFolder,
                ActionId::OpenMemosFolder,
            ] {
                store.push_and_save(&action, &path).unwrap();
            }

            let reloaded = RecentsStore::load(&path);
            // Tighter assertions per codex pass 2 P3 #7. The previous
            // `<=` form passed even if unknown entries were dropped
            // aggressively. Spell out the exact mix:
            //   - exactly 5 visible (known) entries
            //   - exactly 5 preserved unknown entries (10 - 5)
            //   - 10 total = STORAGE_CAP
            assert_eq!(
                reloaded.file.entries.len(),
                STORAGE_CAP,
                "exactly STORAGE_CAP entries should remain after trim"
            );
            assert_eq!(
                reloaded.visible().len(),
                VISIBLE_CAP,
                "exactly VISIBLE_CAP visible entries should remain after trim"
            );
            let unknown_count = reloaded
                .file
                .entries
                .iter()
                .filter(|e| serde_json::from_value::<ActionId>((*e).clone()).is_err())
                .count();
            assert_eq!(
                unknown_count,
                STORAGE_CAP - VISIBLE_CAP,
                "trim should preserve exactly STORAGE_CAP - VISIBLE_CAP unknowns"
            );
        }
    }
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
        // Slice 2 grows the registry to 20 commands. The two new
        // entries since slice 1 are OpenLatestMeetingFromToday and
        // RenameCurrentMeeting; both ship with backing dispatchers.
        assert_eq!(
            all.len(),
            20,
            "slice 2 should have exactly 20 commands with backing dispatchers"
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
                    | ActionId::RenameCurrentMeeting { .. }
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
