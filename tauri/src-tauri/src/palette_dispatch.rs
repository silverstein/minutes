//! Command palette dispatch boundary.
//!
//! This module is the Tauri-side counterpart to `minutes_core::palette`. The
//! core crate describes what commands exist and owns the FFI shape; this
//! module describes how they run. The separation exists so `minutes-core` can
//! stay Tauri-free and remain usable by the CLI, MCP server, and Prompter.
//!
//! # Single source of truth for the FFI boundary
//!
//! The first slice had a hand-maintained `ActionRequest` mirror in this file.
//! Codex's slice 1 review (2026-04-07) caught that the mirror was the same
//! drift the first review had complained about, dressed up as compile-time
//! coupling that only existed under `#[cfg(test)]`. The mirror is gone.
//! `palette_execute` now matches on `minutes_core::palette::ActionId`
//! directly. Adding a new variant in core forces a new arm here, in
//! production code, or this crate fails to compile.
//!
//! # Authoritative backend state
//!
//! `AppState` atomic flags are a mirror, not the truth — the CLI can own
//! recording/dictation/live-transcript PIDs from outside the app process.
//! `backend_flags()` resolves state the same way `cmd_status`,
//! `cmd_live_transcript_status`, and `cmd_stop_dictation` do: atomic flag OR
//! pid-aware probe. See P0 finding 4 in `PLAN.md.command-palette`.
//!
//! `LIVE_TRANSCRIPT` is set only when a *standalone* live transcript session
//! is active. The recording sidecar transcript does not count, because
//! `cmd_stop_live_transcript` cannot stop a recording-owned sidecar — having
//! the palette show "Stop live transcript" while no separate session exists
//! would surface a row that always errors. See slice 1 finding 2.
//!
//! # Scope cut
//!
//! Slice 1 ships dispatch arms for the 18 commands that have a backing
//! executor (existing `cmd_*` Tauri commands or direct calls into
//! `minutes_core` and `crate::context`). Three more commands from the
//! original seed list (`OpenTodayMeetings`, `ReprocessCurrentMeeting`,
//! `RenameCurrentMeeting`) need new core logic and are deferred to slice 2+.
//! `OpenAssistantWorkspace` and `CopyMeetingMarkdown` are wired now per
//! slice 1 finding 5 — they did not need new core logic after all.

use std::sync::atomic::Ordering;

use serde::Deserialize;
use tauri::{AppHandle, State};

use minutes_core::palette::recents::RecentsStore;
use minutes_core::palette::{visible_commands, ActionId, Command, Context, InputKind, StateFlags};
use minutes_core::Config;

use crate::commands::{
    cmd_add_note, cmd_open_meeting_url, cmd_search, cmd_start_dictation, cmd_start_live_transcript,
    cmd_start_recording, cmd_stop_dictation, cmd_stop_live_transcript, cmd_stop_recording,
    cmd_upcoming_meetings, copy_to_clipboard, dictation_pid_active, open_target, recording_active,
    AppState,
};

// ─────────────────────────────────────────────────────────────────────
// FFI types (UI → backend only — ActionId is the request type itself)
// ─────────────────────────────────────────────────────────────────────

/// UI-local context the backend can't resolve on its own. The frontend
/// populates these from its own state (the assistant workspace tells the UI
/// which meeting is open; the webview knows what text is selected).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteUiContext {
    pub current_meeting: Option<std::path::PathBuf>,
    pub selected_text: Option<String>,
}

/// DTO shape the palette UI consumes when rendering a row. Flattens
/// `Command` into the minimum the frontend needs. Kept separate from the
/// core `Command` struct so the core stays free of serde-tauri concerns and
/// the UI doesn't have to mirror Rust's enum discriminants.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDto {
    /// Stable kebab id matching `ActionId`'s serde tag. The UI uses this to
    /// build an `ActionId` JSON object when invoking `palette_execute`.
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub keywords: Vec<&'static str>,
    pub section: &'static str,
    pub input: &'static str,
}

/// Response shape returned by `palette_list`. Carries the visible
/// command set AND the user's recent actions in one round-trip so the
/// UI can render recents at the top when the query is empty without a
/// second IPC hop.
///
/// Recent entries are full hydrated `ActionId` JSON values (not bare
/// kebab strings) so parameterized variants like
/// `SearchTranscripts { query: "pricing" }` round-trip with their
/// payload intact. Entries that don't deserialize into the current
/// `ActionId` schema are hidden here but preserved on disk — see the
/// recents module for details.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteListResponse {
    pub commands: Vec<CommandDto>,
    pub recents: Vec<serde_json::Value>,
}

/// Tagged response envelope for `palette_execute` return values. The
/// outer enum forces the frontend to discriminate on `kind` so it can
/// never confuse one command's result for another (e.g. AddNote vs
/// StopDictation, both of which previously returned `{detail: ...}`
/// shapes through `serde_json::Value`).
///
/// **Honest framing**: this is a tagged envelope, not a fully-typed
/// response surface. Many inner payloads are still `serde_json::Value`
/// passthroughs to existing Tauri command shapes (`cmd_search`,
/// `cmd_upcoming_meetings`, etc.). Typing those inner DTOs is a slice 3
/// refactor. What this catches is *outer* drift — every dispatch arm
/// has a named variant, so a refactor that returns the wrong shape
/// fails to compile instead of leaking type confusion to the frontend.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ActionResponse {
    /// Fire-and-forget actions (started, stopped, opened, etc.).
    Ok,
    /// AddNote — the rendered note line that was appended.
    NoteAdded { line: String },
    /// StopDictation — confirmation message from the dictation pipeline.
    DictationStopped { detail: String },
    /// ReadLiveTranscript — passthrough JSON array of TranscriptLine.
    LiveLines { lines: serde_json::Value },
    /// SearchTranscripts — passthrough cmd_search shape.
    SearchResults { results: serde_json::Value },
    /// ResearchTopic — passthrough cross_meeting_research shape.
    Research { report: serde_json::Value },
    /// FindOpenActionItems — passthrough action items list.
    Actions { actions: serde_json::Value },
    /// FindRecentDecisions — passthrough decisions list.
    Decisions { decisions: serde_json::Value },
    /// ShowUpcomingMeetings — passthrough cmd_upcoming_meetings shape.
    Upcoming { events: serde_json::Value },
    /// OpenLatestMeetingFromToday / OpenLatestMeeting — metadata about
    /// which meeting was opened.
    MeetingOpened { path: String },
    /// RenameCurrentMeeting — old + new path so the UI can refresh
    /// CURRENT_MEETING.md if needed.
    MeetingRenamed {
        #[serde(rename = "oldPath")]
        old_path: String,
        #[serde(rename = "newPath")]
        new_path: String,
    },
}

impl From<&Command> for CommandDto {
    fn from(c: &Command) -> Self {
        Self {
            id: c.id.as_kebab(),
            title: c.title,
            description: c.description,
            keywords: c.keywords.to_vec(),
            section: section_name(c.section),
            input: input_kind_name(c.input),
        }
    }
}

fn section_name(s: minutes_core::palette::Section) -> &'static str {
    use minutes_core::palette::Section;
    match s {
        Section::Recording => "recording",
        Section::Dictation => "dictation",
        Section::Navigation => "navigation",
        Section::Search => "search",
    }
}

fn input_kind_name(k: InputKind) -> &'static str {
    match k {
        InputKind::None => "none",
        InputKind::InlineQuery => "inline-query",
        InputKind::PromptText => "prompt-text",
    }
}

// ─────────────────────────────────────────────────────────────────────
// Backend state resolution (authoritative, pid-aware)
// ─────────────────────────────────────────────────────────────────────

/// Resolve `StateFlags` from the union of `AppState` atomic flags and
/// pid-aware probes. Must stay in sync with the logic in `cmd_status`,
/// `cmd_live_transcript_status`, and `cmd_stop_dictation`. The two sources
/// disagree when another Minutes process (e.g. the CLI) owns a session.
///
/// `LIVE_TRANSCRIPT` is set only when a *standalone* live transcript pid
/// file exists, NOT when the recording sidecar is active. The sidecar
/// transcript can't be stopped via `cmd_stop_live_transcript` (it would
/// return "No live transcript session active"), so showing a stop row for
/// it would be a lie. See slice 1 finding 2.
pub(crate) fn backend_flags(state: &AppState) -> StateFlags {
    let mut f = StateFlags::empty();

    if recording_active(&state.recording) {
        f = f.union(StateFlags::RECORDING);
    }

    let live_in_app = state.live_transcript_active.load(Ordering::Relaxed);
    let standalone_live_pid =
        minutes_core::pid::check_pid_file(&minutes_core::pid::live_transcript_pid_path())
            .ok()
            .flatten()
            .is_some();
    if live_in_app || standalone_live_pid {
        f = f.union(StateFlags::LIVE_TRANSCRIPT);
    }

    let dict_in_app = state.dictation_active.load(Ordering::Relaxed);
    if dict_in_app || dictation_pid_active() {
        f = f.union(StateFlags::DICTATION);
    }

    f
}

fn merge_context(state: &AppState, ui: PaletteUiContext) -> Context {
    Context {
        flags: backend_flags(state),
        current_meeting: ui.current_meeting,
        selected_text: ui.selected_text,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Commands
// ─────────────────────────────────────────────────────────────────────

/// Return the list of commands visible for the current merged context
/// plus the user's recent action history.
///
/// Takes `PaletteUiContext` inline because only the frontend knows which
/// meeting is currently open in the assistant webview and whether text is
/// selected. Backend state is resolved via `backend_flags`.
///
/// Recents are loaded fresh on every call (cheap — `~/.minutes/palette.json`
/// is small) so multiple processes writing to the file stay consistent.
/// Unknown-variant entries from a future client are filtered out of the
/// returned list but preserved on disk.
#[tauri::command]
pub fn palette_list(
    state: State<'_, AppState>,
    ui: Option<PaletteUiContext>,
) -> PaletteListResponse {
    let ctx = merge_context(&state, ui.unwrap_or_default());
    let commands = visible_commands(&ctx)
        .iter()
        .map(CommandDto::from)
        .collect();

    let recents_path = RecentsStore::default_path();
    let recents_store = RecentsStore::load(&recents_path);
    let recents: Vec<serde_json::Value> = recents_store
        .visible()
        .into_iter()
        .filter_map(|action| serde_json::to_value(&action).ok())
        .collect();

    PaletteListResponse { commands, recents }
}

/// Push a successfully executed action to the recents store. Logged but
/// non-fatal — a recents write failure must never break command
/// execution.
fn record_recent(action: &ActionId) {
    let path = RecentsStore::default_path();
    let mut store = RecentsStore::load(&path);
    if let Err(e) = store.push_and_save(action, &path) {
        eprintln!("[palette] could not persist recents: {}", e);
    }
}

/// Execute an `ActionId` against the live app state. Exhaustive match
/// directly on the core enum — adding a new variant in core forces a new
/// arm here or this crate will not compile.
///
/// Returns a tagged [`ActionResponse`] envelope rather than a free-form
/// `serde_json::Value`. The frontend discriminates on `kind`. See D7 of
/// `PLAN.md.command-palette-slice-2`.
///
/// On a successful execution the action is pushed to the recents store
/// **with its full hydrated payload** (`SearchTranscripts { query:
/// "pricing" }`, not the bare `id`), so a re-run from the recents list
/// retains the user's original input. Recents persistence failures are
/// logged but never returned — they must not break command execution.
#[tauri::command]
pub fn palette_execute(
    app: AppHandle,
    state: State<'_, AppState>,
    ui: Option<PaletteUiContext>,
    action: ActionId,
) -> Result<ActionResponse, String> {
    let ui = ui.unwrap_or_default();
    let action_for_recents = action.clone();

    let result = dispatch_action(app, state, ui, action);
    if result.is_ok() {
        record_recent(&action_for_recents);
    }
    result
}

fn dispatch_action(
    app: AppHandle,
    state: State<'_, AppState>,
    ui: PaletteUiContext,
    action: ActionId,
) -> Result<ActionResponse, String> {
    match action {
        // ── Recording ────────────────────────────────────────────
        ActionId::StartRecording => {
            // Synchronous preflight (D6) so the user sees the rejection
            // reason in the palette instead of a deferred system
            // notification while the background thread bails out.
            //
            // **Honest scope**: preflight only catches process /
            // device / call-routing rejections. Mic permission, disk
            // space, and PID file creation still fail later inside
            // `start_recording`. A real `validate_recording_start`
            // refactor is filed as a follow-up issue.
            let config = Config::load();
            let preflight = minutes_core::capture::preflight_recording(
                minutes_core::CaptureMode::Meeting,
                None,
                false,
                &config,
            )
            .map_err(|e| format!("preflight failed: {}", e))?;
            if let Some(reason) = preflight.blocking_reason.as_ref() {
                let native_call_capture_available = preflight.intent
                    == minutes_core::capture::RecordingIntent::Call
                    && matches!(
                        crate::call_capture::availability(),
                        crate::call_capture::CallCaptureAvailability::Available { .. }
                    );
                if !native_call_capture_available {
                    return Err(format!("recording blocked: {}", reason));
                }
            }
            // Palette launches with pipeline defaults — users who need flags
            // reach for the CLI or the existing tray menu.
            cmd_start_recording(app, state, None, None, None, None, None)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::StopRecording => {
            cmd_stop_recording(state)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::AddNote { text } => {
            // Refuse empty notes to avoid a silently-useless invocation.
            let text = text.unwrap_or_default();
            if text.trim().is_empty() {
                return Err("note text is empty".into());
            }
            let added = cmd_add_note(text)?;
            Ok(ActionResponse::NoteAdded { line: added })
        }
        ActionId::StartLiveTranscript => {
            cmd_start_live_transcript(app, state)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::StopLiveTranscript => {
            cmd_stop_live_transcript(state)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::ReadLiveTranscript => {
            // Read from line 0 = everything the session has produced so
            // far. The palette UI can render this in a popover.
            let lines = minutes_core::live_transcript::read_since_line(0)
                .map_err(|e| format!("failed to read live transcript: {}", e))?;
            let lines = serde_json::to_value(&lines)
                .map_err(|e| format!("serialize live transcript: {}", e))?;
            Ok(ActionResponse::LiveLines { lines })
        }

        // ── Dictation ────────────────────────────────────────────
        ActionId::StartDictation => {
            cmd_start_dictation(app, state)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::StopDictation => {
            let detail = cmd_stop_dictation(state)?;
            Ok(ActionResponse::DictationStopped { detail })
        }

        // ── Navigation ───────────────────────────────────────────
        ActionId::OpenLatestMeeting => {
            let config = Config::load();
            let filters = minutes_core::search::SearchFilters {
                content_type: None,
                since: None,
                attendee: None,
                intent_kind: None,
                owner: None,
                recorded_by: None,
            };
            let results = minutes_core::search::search("", &config, &filters)
                .map_err(|e| format!("list meetings: {}", e))?;
            let latest = results
                .into_iter()
                .next()
                .ok_or_else(|| "no meetings yet".to_string())?;
            let path = latest.path.to_string_lossy().to_string();
            cmd_open_meeting_url(app, path.clone())?;
            Ok(ActionResponse::MeetingOpened { path })
        }
        ActionId::OpenLatestMeetingFromToday => {
            let config = Config::load();
            // Compute today's start in the local timezone, then format
            // as RFC3339 — `SearchFilters::since` is a lexicographic
            // RFC3339 comparison (search.rs:203).
            let today_start = chrono::Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| "could not compute today's start time".to_string())?
                .and_local_timezone(chrono::Local)
                .single()
                .ok_or_else(|| "ambiguous local time at midnight".to_string())?
                .to_rfc3339();
            let filters = minutes_core::search::SearchFilters {
                content_type: None,
                since: Some(today_start),
                attendee: None,
                intent_kind: None,
                owner: None,
                recorded_by: None,
            };
            let results = minutes_core::search::search("", &config, &filters)
                .map_err(|e| format!("list today's meetings: {}", e))?;
            let latest = results
                .into_iter()
                .next()
                .ok_or_else(|| "no meetings yet today".to_string())?;
            let path = latest.path.to_string_lossy().to_string();
            cmd_open_meeting_url(app, path.clone())?;
            Ok(ActionResponse::MeetingOpened { path })
        }
        ActionId::OpenMeetingsFolder => {
            let config = Config::load();
            open_target(&app, &config.output_dir.to_string_lossy())?;
            Ok(ActionResponse::Ok)
        }
        ActionId::OpenMemosFolder => {
            let config = Config::load();
            let memos = config.output_dir.join("memos");
            open_target(&app, &memos.to_string_lossy())?;
            Ok(ActionResponse::Ok)
        }
        ActionId::OpenAssistantWorkspace => {
            let config = Config::load();
            let workspace_root = crate::context::create_workspace(&config)
                .map_err(|e| format!("create assistant workspace: {}", e))?;
            open_target(&app, &workspace_root.to_string_lossy())?;
            Ok(ActionResponse::Ok)
        }
        ActionId::ShowUpcomingMeetings => {
            // Sync command bridges to the async cmd_upcoming_meetings via
            // the blocking runtime helper. Tauri's async runtime is a
            // tokio runtime under the hood; block_on inside a sync command
            // is the documented bridge.
            let events = tauri::async_runtime::block_on(cmd_upcoming_meetings());
            Ok(ActionResponse::Upcoming { events })
        }

        // ── Search / research ────────────────────────────────────
        ActionId::SearchTranscripts { query } => {
            // Empty query => list-style results; let core decide. Match the
            // existing cmd_search behavior (unwraps SearchError to empty).
            let q = query.unwrap_or_default();
            let results = cmd_search(q);
            Ok(ActionResponse::SearchResults { results })
        }
        ActionId::ResearchTopic { query } => {
            let q = query.ok_or_else(|| "research topic requires a query".to_string())?;
            if q.trim().is_empty() {
                return Err("research topic query is empty".into());
            }
            let config = Config::load();
            let filters = minutes_core::search::SearchFilters {
                content_type: None,
                since: None,
                attendee: None,
                intent_kind: None,
                owner: None,
                recorded_by: None,
            };
            let research = minutes_core::search::cross_meeting_research(&q, &config, &filters)
                .map_err(|e| format!("research: {}", e))?;
            let report = serde_json::to_value(&research)
                .map_err(|e| format!("serialize research: {}", e))?;
            Ok(ActionResponse::Research { report })
        }
        ActionId::FindOpenActionItems => {
            let config = Config::load();
            let actions = minutes_core::search::find_open_actions(&config, None)
                .map_err(|e| format!("find open actions: {}", e))?;
            let actions =
                serde_json::to_value(&actions).map_err(|e| format!("serialize actions: {}", e))?;
            Ok(ActionResponse::Actions { actions })
        }
        ActionId::FindRecentDecisions => {
            let config = Config::load();
            let filters = minutes_core::search::SearchFilters {
                content_type: None,
                since: None,
                attendee: None,
                intent_kind: Some(minutes_core::markdown::IntentKind::Decision),
                owner: None,
                recorded_by: None,
            };
            let decisions = minutes_core::search::search_intents("", &config, &filters)
                .map_err(|e| format!("search decisions: {}", e))?;
            let decisions = serde_json::to_value(&decisions)
                .map_err(|e| format!("serialize decisions: {}", e))?;
            Ok(ActionResponse::Decisions { decisions })
        }

        // ── Meeting-context actions ──────────────────────────────
        ActionId::CopyMeetingMarkdown => {
            let path = ui
                .current_meeting
                .ok_or_else(|| "no current meeting in palette context".to_string())?;
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("read meeting {}: {}", path.display(), e))?;
            copy_to_clipboard(&content)?;
            Ok(ActionResponse::Ok)
        }
        ActionId::RenameCurrentMeeting { new_title } => {
            let path = ui
                .current_meeting
                .ok_or_else(|| "no current meeting in palette context".to_string())?;
            let new_title = new_title.ok_or_else(|| "new title is required".to_string())?;
            let new_path = minutes_core::markdown::rename_meeting(&path, &new_title)
                .map_err(|e| format!("rename failed: {}", e))?;
            Ok(ActionResponse::MeetingRenamed {
                old_path: path.to_string_lossy().to_string(),
                new_path: new_path.to_string_lossy().to_string(),
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use minutes_core::palette::commands;

    #[test]
    fn command_dto_converts_from_core_command() {
        let all = commands();
        let dto: CommandDto = (&all[0]).into();
        assert!(!dto.id.is_empty());
        assert!(!dto.title.is_empty());
        assert!(!dto.description.is_empty());
    }

    #[test]
    fn palette_ui_context_deserializes_empty() {
        let ctx: PaletteUiContext = serde_json::from_str("{}").unwrap();
        assert!(ctx.current_meeting.is_none());
        assert!(ctx.selected_text.is_none());
    }

    #[test]
    fn palette_ui_context_deserializes_full() {
        let ctx: PaletteUiContext =
            serde_json::from_str(r#"{"currentMeeting":"/tmp/x.md","selectedText":"foo"}"#).unwrap();
        assert_eq!(ctx.current_meeting.unwrap().to_string_lossy(), "/tmp/x.md");
        assert_eq!(ctx.selected_text.unwrap(), "foo");
    }

    #[test]
    fn action_id_deserializes_from_palette_json() {
        // The core crate already covers serde round-trips; this test asserts
        // the JSON shape the palette UI must produce. Lock the contract in
        // the place the UI engineers will read.
        let req: ActionId = serde_json::from_str(r#"{"id":"start-recording"}"#).unwrap();
        assert_eq!(req, ActionId::StartRecording);

        let req: ActionId =
            serde_json::from_str(r#"{"id":"search-transcripts","query":"pricing"}"#).unwrap();
        match req {
            ActionId::SearchTranscripts { query } => {
                assert_eq!(query.as_deref(), Some("pricing"));
            }
            other => panic!("expected SearchTranscripts, got {:?}", other),
        }

        let req: ActionId = serde_json::from_str(r#"{"id":"add-note","text":"hello"}"#).unwrap();
        match req {
            ActionId::AddNote { text } => {
                assert_eq!(text.as_deref(), Some("hello"));
            }
            other => panic!("expected AddNote, got {:?}", other),
        }
    }

    #[test]
    fn every_registry_command_has_a_dispatch_arm_via_compiler() {
        // The exhaustive `match` in `palette_execute` is what enforces the
        // coupling — this test exists only to make the intent visible. If
        // you delete a variant from the match, this file fails to compile.
        // If you add a variant in core without an arm here, this file
        // fails to compile. The test body is just a smoke check that
        // every registry entry can construct a real ActionId.
        for cmd in commands() {
            let kebab = cmd.id.as_kebab();
            assert!(!kebab.is_empty());
        }
    }

    // ─────────────────────────────────────────────────────────────
    // ActionResponse serialization (D7)
    //
    // One test per variant, asserting the `kind` tag and that the
    // outer shape is what the frontend will receive. These exist so a
    // future refactor that renames a variant or changes its inner
    // payload structure breaks compilation here, not silently in the
    // UI.
    // ─────────────────────────────────────────────────────────────

    fn assert_kind(value: &serde_json::Value, expected_kind: &str) {
        let kind = value
            .get("kind")
            .and_then(|v| v.as_str())
            .expect("ActionResponse must serialize with a `kind` tag");
        assert_eq!(kind, expected_kind);
    }

    #[test]
    fn action_response_ok_has_kind_tag() {
        let v = serde_json::to_value(ActionResponse::Ok).unwrap();
        assert_kind(&v, "ok");
    }

    #[test]
    fn action_response_note_added_has_line_field() {
        let v = serde_json::to_value(ActionResponse::NoteAdded {
            line: "[00:01:23] note text".into(),
        })
        .unwrap();
        assert_kind(&v, "note-added");
        assert_eq!(
            v.get("line").and_then(|x| x.as_str()),
            Some("[00:01:23] note text")
        );
    }

    #[test]
    fn action_response_dictation_stopped_has_detail() {
        let v = serde_json::to_value(ActionResponse::DictationStopped {
            detail: "stopped".into(),
        })
        .unwrap();
        assert_kind(&v, "dictation-stopped");
        assert_eq!(v.get("detail").and_then(|x| x.as_str()), Some("stopped"));
    }

    #[test]
    fn action_response_live_lines_has_lines() {
        let v = serde_json::to_value(ActionResponse::LiveLines {
            lines: serde_json::json!([{"text": "hi"}]),
        })
        .unwrap();
        assert_kind(&v, "live-lines");
        assert!(v.get("lines").unwrap().is_array());
    }

    #[test]
    fn action_response_search_results_has_results() {
        let v = serde_json::to_value(ActionResponse::SearchResults {
            results: serde_json::json!([]),
        })
        .unwrap();
        assert_kind(&v, "search-results");
        assert!(v.get("results").unwrap().is_array());
    }

    #[test]
    fn action_response_research_has_report() {
        let v = serde_json::to_value(ActionResponse::Research {
            report: serde_json::json!({"query": "x"}),
        })
        .unwrap();
        assert_kind(&v, "research");
        assert!(v.get("report").unwrap().is_object());
    }

    #[test]
    fn action_response_actions_has_actions() {
        let v = serde_json::to_value(ActionResponse::Actions {
            actions: serde_json::json!([]),
        })
        .unwrap();
        assert_kind(&v, "actions");
        assert!(v.get("actions").unwrap().is_array());
    }

    #[test]
    fn action_response_decisions_has_decisions() {
        let v = serde_json::to_value(ActionResponse::Decisions {
            decisions: serde_json::json!([]),
        })
        .unwrap();
        assert_kind(&v, "decisions");
        assert!(v.get("decisions").unwrap().is_array());
    }

    #[test]
    fn action_response_upcoming_has_events() {
        let v = serde_json::to_value(ActionResponse::Upcoming {
            events: serde_json::json!([]),
        })
        .unwrap();
        assert_kind(&v, "upcoming");
        assert!(v.get("events").unwrap().is_array());
    }

    #[test]
    fn action_response_meeting_opened_has_path() {
        let v = serde_json::to_value(ActionResponse::MeetingOpened {
            path: "/tmp/m.md".into(),
        })
        .unwrap();
        assert_kind(&v, "meeting-opened");
        assert_eq!(v.get("path").and_then(|x| x.as_str()), Some("/tmp/m.md"));
    }

    #[test]
    fn action_response_meeting_renamed_has_paths() {
        let v = serde_json::to_value(ActionResponse::MeetingRenamed {
            old_path: "/tmp/old.md".into(),
            new_path: "/tmp/new.md".into(),
        })
        .unwrap();
        assert_kind(&v, "meeting-renamed");
        assert_eq!(
            v.get("oldPath").and_then(|x| x.as_str()),
            Some("/tmp/old.md")
        );
        assert_eq!(
            v.get("newPath").and_then(|x| x.as_str()),
            Some("/tmp/new.md")
        );
    }
}
