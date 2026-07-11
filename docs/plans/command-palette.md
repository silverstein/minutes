# PLAN: Command Palette for Minutes (Tauri)

> **Single source of truth** for the command palette work.
> Update this file at every step. If the conversation context is lost, this file
> should be enough to resume.

**Started**: 2026-04-07
**Status**: 🟡 drafting
**Goal**: Ship a keyboard-first command palette (⌘⇧K) inside the Minutes Tauri app, backed by a single command registry in `minutes-core` that is structured well enough to later feed CLI help and MCP tool descriptions without drift.

---

## Vision

Minutes has three command surfaces today:

1. **CLI** — ~40 clap subcommands in `crates/cli/src/main.rs`
2. **MCP** — 26 tools in `crates/mcp/src/index.ts`
3. **Tauri menu bar / singleton assistant** — hand-wired Tauri commands in `tauri/src-tauri/src/commands.rs` (5226 lines)

Every new feature currently means editing all three. Worse, the Tauri app has no keyboard-first way to reach the features users already know from the CLI. The palette is the **fourth surface**, but the first one that is structured around a shared registry. v1 ships the palette only. v2+ can refactor CLI and MCP to read from the same registry.

This is a deliberate worse-is-better move: prove the registry pattern by building one consumer, not by refactoring three.

## Non-goals (v1 scope cut)

- ❌ System-wide Raycast-style launcher (separate project if ever)
- ❌ Plugin commands from third parties
- ❌ Refactoring CLI or MCP to consume the registry
- ❌ Browser `navigator.commands` API advocacy (separate blog post, not code)
- ❌ Context-aware "smart suggestions" beyond explicit state predicates
- ❌ Localization / i18n of command titles
- ❌ Natural-language command parsing ("record a meeting about X" → start + title)

v1 is: open with hotkey, fuzzy search a curated list, pick one, do the thing.

## Opinionated design

### Registry lives in `minutes-core`

Module: `crates/core/src/palette.rs` (implemented P1, then rewritten after
codex P0 review).

```rust
pub enum ActionId {
    StartRecording, StopRecording,
    AddNote(Option<String>),
    StartLiveTranscript, StopLiveTranscript, ReadLiveTranscript,
    StartDictation, StopDictation,
    OpenLatestMeeting, OpenTodayMeetings, ShowUpcomingMeetings,
    OpenMeetingsFolder, OpenMemosFolder, OpenAssistantWorkspace,
    SearchTranscripts(Option<String>), ResearchTopic(Option<String>),
    FindOpenActionItems, FindRecentDecisions,
    ReprocessCurrentMeeting, RenameCurrentMeeting(Option<String>), CopyMeetingMarkdown,
}

pub enum InputKind { None, InlineQuery, PromptText, RenameCurrentMeeting }

pub struct StateFlags(u8);  // RECORDING | LIVE_TRANSCRIPT | DICTATION | MEETING_OPEN
pub struct Visibility { requires: StateFlags, forbids: StateFlags }

pub struct Command {
    pub id: ActionId,        // stored in "empty" form — parameters injected at dispatch
    pub title: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub section: Section,
    pub visibility: Visibility,
    pub input: InputKind,
}

pub struct Context { flags: StateFlags, current_meeting: Option<PathBuf>, selected_text: Option<String> }
pub fn commands() -> Vec<Command>;
pub fn visible_commands(ctx: &Context) -> Vec<Command>;
pub fn is_visible(v: Visibility, flags: StateFlags) -> bool;
```

**Key design decisions (all post-codex):**

- **No template indirection.** The first draft had `ActionIdTemplate` mirroring `ActionId` because of a false belief that a static slice couldn't hold parameterized variants. In fact `Option::None` is `const`-constructible, so `ActionId::SearchTranscripts(None)` sits in a static fine. The template was dead weight and is gone.
- **`InputKind` is explicit, not implicit per-variant.** Any command whose `ActionId` carries a payload has a matching non-`None` `InputKind`. A test enforces this so future drift fails at CI, not in production.
- **`Visibility` is `requires`/`forbids` flags, not a single enum variant.** The earlier `WhenMeetingOpen` predicate couldn't express "meeting open AND idle" or "dictation active", which would have left meeting-mutating commands reachable mid-session. The flag model does it in a test (`meeting_open_while_recording_hides_mutating_but_allows_copy`).
- **Recents must persist the full hydrated `ActionId`, not a kebab string.** Otherwise "search transcripts for 'pricing'" collapses to "search transcripts" and the query is lost.

### Execution lives in Tauri (with typed requests, not strings)

The core module does **not** execute commands — it only describes them. Tauri
owns execution because Tauri is the one with the app state, event channels,
and window handles. But the dispatch boundary is **typed end-to-end**, not
stringly-typed through `serde_json::Value`. The earlier draft in this PLAN
was wrong; codex P0 caught it.

```rust
// tauri/src-tauri/src/palette_dispatch.rs (NEW FILE — do not bloat commands.rs)

use tauri::{AppHandle, State};
use minutes_core::palette::{commands, ActionId, Context, StateFlags};
use crate::commands::AppState;

/// Exact serde-tagged form of ActionId for the FFI boundary. Lives here
/// rather than in core so core stays Tauri-free.
#[derive(serde::Deserialize)]
#[serde(tag = "id", rename_all = "kebab-case")]
pub enum ActionRequest {
    StartRecording,
    StopRecording,
    AddNote { text: String },
    StartLiveTranscript,
    StopLiveTranscript,
    ReadLiveTranscript,
    StartDictation,
    StopDictation,
    OpenLatestMeeting,
    OpenTodayMeetings,
    ShowUpcomingMeetings,
    OpenMeetingsFolder,
    OpenMemosFolder,
    OpenAssistantWorkspace,
    SearchTranscripts { query: Option<String> },
    ResearchTopic { query: Option<String> },
    FindOpenActionItems,
    FindRecentDecisions,
    ReprocessCurrentMeeting,
    RenameCurrentMeeting { new_title: String },
    CopyMeetingMarkdown,
}

/// What the palette UI knows that the backend doesn't.
#[derive(serde::Deserialize)]
pub struct PaletteUiContext {
    pub current_meeting: Option<PathBuf>,
    pub selected_text: Option<String>,
}

#[tauri::command]
pub async fn palette_list(
    state: State<'_, AppState>,
    ui: PaletteUiContext,
) -> Result<Vec<CommandDto>, String> {
    let flags = backend_flags(&state);
    let ctx = Context {
        flags,
        current_meeting: ui.current_meeting,
        selected_text: ui.selected_text,
    };
    Ok(visible_commands(&ctx).into_iter().map(CommandDto::from).collect())
}

#[tauri::command]
pub async fn palette_execute(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ActionRequest,
) -> Result<(), String> {
    use crate::commands::*;
    match action {
        ActionRequest::StartRecording => cmd_start_recording(app, state, /*...*/).await,
        ActionRequest::StopRecording  => cmd_stop_recording(state).await,
        ActionRequest::AddNote { text } => cmd_add_note(text, /*...*/).await,
        ActionRequest::StartDictation => cmd_start_dictation(app, state).await,
        ActionRequest::StopDictation  => cmd_stop_dictation(state).await,
        // ... one arm per ActionRequest variant; exhaustive match, compiler-enforced.
    }
}

/// Backend half of Context resolution. Must use the same pid-aware logic as
/// cmd_status / cmd_live_transcript_status / cmd_stop_dictation — atomic
/// flags alone are wrong because the CLI can own these PIDs from outside the
/// app process.
fn backend_flags(state: &AppState) -> StateFlags {
    let mut f = StateFlags::empty();
    if minutes_core::pid::status().is_recording() { f = f.union(StateFlags::RECORDING); }
    if minutes_core::pid::live_transcript_active()  { f = f.union(StateFlags::LIVE_TRANSCRIPT); }
    if minutes_core::pid::dictation_pid_active()    { f = f.union(StateFlags::DICTATION); }
    f
}
```

- **`palette_dispatch.rs` is a new file.** Do not add any of this to
  `tauri/src-tauri/src/commands.rs` (already 5226 lines).
- **Exhaustive match** on `ActionRequest` means a new `ActionId` variant in
  core forces a corresponding `ActionRequest` variant and dispatch arm, or
  the Tauri crate fails to compile. This is the compile-time coupling codex
  asked for.
- **Every registry entry must have a backing dispatch arm before the entry
  ships.** Do not register a command in `commands()` until the matching
  `ActionRequest` variant and dispatch arm exist. See finding 3.
- **`palette_get_context` is gone** as a separate command. `palette_list`
  takes `PaletteUiContext` directly, uses backend state resolved via
  pid-aware helpers, and hands the merged `Context` to `visible_commands`.

### UI: lightweight overlay window

New Tauri window: `palette` (webview). Not part of the assistant webview — a separate overlay window so it can open fast and doesn't steal assistant focus. Files:

- `tauri/src/palette/index.html` — markup
- `tauri/src/palette/palette.ts` — vanilla TS, ~200 lines
- `tauri/src/palette/palette.css` — minimal

**No framework.** Minutes' existing Tauri UI uses vanilla HTML + a little TS. Bringing in React/Vue just for a palette is a mistake.

**Fuzzy matching**: implement in-module or pull `fuzzy-matcher` crate and expose a Tauri command `palette_fuzzy_search(query, ctx_json)`. Probably server-side: the core already knows the full registry and can score without round-tripping the list to JS.

### Hotkey

- **⌘⇧K on macOS** (primary target)
- Registered through existing `crates/core/src/hotkey_macos.rs` CGEventTap path
- Tauri v1: conflicts with nothing Minutes already owns (dictation is ⌃⌥⌘ or similar)
- Fallback: user-configurable in `config.toml` under `[palette] hotkey = "cmd+shift+k"`

### Context-aware filtering, flag-based

v1 uses a `StateFlags` bitmask + `Visibility { requires, forbids }`:

Flags: `RECORDING | LIVE_TRANSCRIPT | DICTATION | MEETING_OPEN | ANY_SESSION`
(where `ANY_SESSION = RECORDING | LIVE_TRANSCRIPT | DICTATION`).

Shorthand visibilities:
- `always()` — no constraints
- `when_idle()` — forbids `ANY_SESSION`
- `when_recording()` / `when_live_transcript()` / `when_dictation()` — requires that flag
- `when_meeting_open_and_idle()` — requires `MEETING_OPEN`, forbids `ANY_SESSION`

If visibility is not satisfied, the command is hidden entirely. No "grayed out" states in v1.

### Recent list

- Last 5 successfully executed `ActionId`s
- Persisted to `~/.minutes/palette.json`
- Floats to top when query is empty
- Pinned commands (v1.1, not v1) can later promote commands above recents

## Commands in v1 (the seed list)

Post-codex revision: dropped the 3 admin commands (`EnrollMyVoice`,
`RunHealthCheck`, `OpenConfig`) — first-week keyboard users don't reach for
those. Added `StopDictation` (the `StartDictation`-only dead-end),
`ReadLiveTranscript`, `ShowUpcomingMeetings`, and `ResearchTopic`.

**Recording (6)** — `Start`, `Stop`, `AddNote` (text), `StartLive`, `StopLive`, `ReadLive`

**Dictation (2)** — `Start`, `Stop`

**Navigation (5)** — `OpenLatestMeeting`, `OpenTodayMeetings`, `ShowUpcomingMeetings`, `OpenMeetingsFolder`, `OpenMemosFolder`, `OpenAssistantWorkspace`

**Search / research (4)** — `SearchTranscripts` (inline), `ResearchTopic` (inline), `FindOpenActionItems`, `FindRecentDecisions`

**Meeting actions (3)** — only when meeting open AND (for mutating ones) idle: `Reprocess`, `Rename` (text), `CopyMarkdown`

Total: **21 commands**. Every new command after v1 must earn its slot **and**
have a backing dispatch arm before it lands in `commands()`.

## Risks (adversarial, post-codex)

| # | Risk | Mitigation |
|---|---|---|
| R1 | Hotkey ⌘⇧K conflicts with an app the user cares about | Audit common shortcut holders before ship; make the binding configurable in `config.toml` from day one |
| R2 | Palette overlay window hits the Tauri v2 capability permission gotcha (see memory `feedback_tauri_capabilities.md`) | First build step: explicitly add window-event permissions to `capabilities/default.json`. Don't debug this in hour 4 |
| R3 | **Backend state is not authoritative via `AppState` alone** — CLI can own recording/live/dictation PIDs from outside the app process | Resolve flags in `backend_flags()` using `minutes_core::pid::*` helpers the same way `cmd_status`, `cmd_live_transcript_status`, and `cmd_stop_dictation` already do. `AppState` bool fields are a stale mirror |
| R4 | **No authoritative `current_meeting` source exists** — today it lives in the assistant workspace `CURRENT_MEETING.md` file and nowhere in `AppState` | `PaletteUiContext` passes it from the frontend. If no meeting is selected in the assistant, meeting-mutating rows simply don't appear. Do not invent a new AppState field |
| R5 | **Registry entries ship without a backing dispatcher** — the v1 seed list could list 21 commands with only 12 executors wired | Enforce at PR review: every `Command` in `commands()` must have a matching `ActionRequest` variant AND a concrete dispatch arm. A test in `palette_dispatch.rs` should iterate `commands()` and deserialize each into an `ActionRequest` to catch drift |
| R6 | **Recent list loses parameterized payloads** — persisting kebab strings collapses "search for pricing" to "search" | Persist the full hydrated `ActionRequest` serde JSON in `~/.minutes/palette.json`. Treat parse failure as "no recents", never fatal |
| R7 | Fuzzy matcher crate (`fuzzy-matcher`) adds a dep to core that leaks into CLI and MCP builds | Gate behind a `palette` Cargo feature so CLI-only consumers stay lean |
| R8 | **The "four surfaces, one registry" framing is aspirational in v1** — only the palette consumes it | Be honest in the PR description and in module docs. Don't rename to "commands" until CLI or MCP actually consume the registry |
| R9 | The palette window is a TCC-permission-sensitive surface (global hotkey) | Use `./scripts/install-dev-app.sh` from day one per `CLAUDE.md`. Never test against `/Applications/Minutes.app` |
| R10 | **Dispatch can grow sprawling once v1.1 adds 10 more commands** | The exhaustive `match` on `ActionRequest` is compile-time enforced. When it grows past 40 arms, refactor into `action_request.dispatch(app, state)` on the enum — but not before |
| R11 | **Input-bearing commands under-modeled at UI level** — `InputKind::RenameCurrentMeeting` implies a pre-filled modal the palette doesn't yet have | P3 must ship all four `InputKind` UI handlers or cut the commands. Do not ship rename without the modal |
| R12 | Two palette windows can race if the hotkey is hit while the overlay is already showing | Palette window singleton — second trigger should focus, not spawn |

## 10/10 acceptance criteria

**Slice 1 (core registry + typed dispatcher, no UI yet):**
- [x] PLAN file exists and is updated through every phase (this file)
- [x] `crates/core/src/palette.rs` module compiles, has doc comments, 16 unit tests passing
- [x] `ActionId` enum carries parameters where needed (struct variants with serde tag)
- [x] Static registry has 18 seed commands, all with backing dispatchers
- [x] `visible_commands(ctx)` returns correct filtered set for all predicates including composition (meeting open AND recording)
- [x] `palette_dispatch.rs` is a new file, not a bloat of `commands.rs` (361 lines vs commands.rs at 5226)
- [x] `ActionId` is the FFI type — exhaustive match in production, not `#[cfg(test)]`
- [x] `backend_flags()` resolves state with pid-aware probes, not stale `AppState` mirrors
- [x] Pre-commit checklist items pass on both crates: `cargo fmt`, `cargo clippy -p minutes-app --no-deps -- -D warnings`, `cargo test -p minutes-core --no-default-features palette::`, `cargo test -p minutes-app palette_dispatch::`
- [x] Codex adversarial review run 3 times (PLAN P0, dispatcher slice 1, fixes confirmation)

**Slice 2 (UI + hotkey + dogfood):**
- [ ] Tauri palette window opens via ⌘⇧K, shows commands, executes them
- [ ] Fuzzy matcher is deterministic and has unit tests (including edge cases: empty query, no matches, ties)
- [ ] Recent list persists across restarts and survives corrupted-file recovery
- [ ] `capabilities/default.json` explicitly grants window events for the palette window
- [ ] Built with `./scripts/install-dev-app.sh` and tested as `~/Applications/Minutes Dev.app` (per TCC rules)
- [ ] `StartRecording` preflight runs synchronously before the spawn (fix P2 #3)
- [ ] `palette_execute` returns a typed `ActionResponse`, not `serde_json::Value` (fix P2 #6)
- [ ] README mentions the palette in the features section
- [ ] Release notes drafted before version bump

## Phases

### P0. Codex adversarial review of this PLAN — PENDING

Before writing any Rust: hand this PLAN to codex and ask for holes. Record findings in the "Findings log" section. Iterate the PLAN until codex can't find anything worth fixing.

### P1. Scaffold `crates/core/src/palette.rs` — PENDING

- [ ] New module with `Command`, `ActionId`, `Context`, `VisiblePredicate`, `Section`
- [ ] Static slice of 20 seed commands
- [ ] `visible_commands(ctx)` function with tests for all predicates
- [ ] `fuzzy_match(query, ctx)` behind a `palette` feature flag (so CLI doesn't pay for `fuzzy-matcher`)
- [ ] 10+ unit tests
- [ ] Doc comments on everything pub
- [ ] Module wired into `crates/core/src/lib.rs`
- [ ] `cargo test -p minutes-core --no-default-features` passes
- [ ] `cargo test -p minutes-core --features palette` passes

### P2. Codex adversarial review of the core module — PENDING

After scaffolding compiles and tests pass, hand `palette.rs` to codex and ask for API design holes. Record findings.

### P3. Tauri integration — PENDING

- [ ] New file `tauri/src-tauri/src/palette_dispatch.rs` — `palette_list`, `palette_execute`, `palette_get_context`, `palette_fuzzy_search`
- [ ] Register commands in `main.rs`
- [ ] New Tauri window `palette` with its own `capabilities/palette.json` or explicit entry in `default.json`
- [ ] Vanilla TS frontend: `tauri/src/palette/{index.html,palette.ts,palette.css}`
- [ ] ⌘⇧K binding through `hotkey_macos.rs`
- [ ] Recent list persistence at `~/.minutes/palette.json`
- [ ] All 20 seed commands executable end-to-end

### P4. Dogfood via dev app — PENDING

- [ ] `./scripts/install-dev-app.sh --no-open` produces working `~/Applications/Minutes Dev.app`
- [ ] Hotkey works without TCC prompts after first-run approval
- [ ] Recording / live transcript / dictation all invocable from palette
- [ ] Recent list behaves correctly across restarts
- [ ] Known-ugly cases logged as issues (not in-scope fixes)

### P5. Codex adversarial review of the full diff — PENDING

Run `/codex review` on the branch before opening the PR. Record findings. Fix the must-fixes, log the nice-to-haves as follow-ups.

### P6. Pre-commit checklist + PR — PENDING

Walk the full pre-commit checklist in `CLAUDE.md`:
- [ ] cargo fmt
- [ ] cargo clippy
- [ ] cargo test (core, no-default-features)
- [ ] cargo test (core, palette feature)
- [ ] MCP rebuild if any index.ts touched (should be none in v1)
- [ ] README updated — add palette to features list
- [ ] No release warranted for v1 unless paired with other work (this is a feature addition, not a bug fix — bundle with next release)

### P7. Ship — PENDING

- [ ] Open PR with the full context, R1–R10 risk table, and a link to this PLAN
- [ ] Merge via GitHub merge flow (not cherry-pick)
- [ ] Memory note: record the single-registry pattern decision and the "four surfaces, one registry" framing
- [ ] MEMORY.md index updated
- [ ] This file marked DONE with timestamp

---

## Findings log (append-only)

### 2026-04-07 — Codex adversarial review P0 (session `019d68ad-bf02-7372-b848-bf72865382d0`)

Codex ran in consult mode with adversarial framing, read `crates/core/src/palette.rs` (comitted-style but untracked), `crates/core/src/lib.rs`, `tauri/src-tauri/src/commands.rs`, `tauri/src-tauri/src/main.rs`, `tauri/src-tauri/src/context.rs`, and `crates/cli/src/main.rs`. Token usage: 1.84M.

**P1 critical findings:**

1. **`ActionIdTemplate` is dead weight.** `Rust can hold `ActionId::SearchTranscripts(None)` directly in a `static` slice — `None` is a unit variant and doesn't allocate, so the whole `enum ActionId` is `const`-constructible. The template layer is a premature abstraction built on a false constraint. **Fix:** delete `ActionIdTemplate`, change `Command` to hold `pub id: ActionId`, move `hydrate` responsibility to an explicit `InputKind` field (see finding 2).

2. **Parameter story is incoherent.** Only `SearchTranscripts` was treated as input-bearing, but `AddNote` and `RenameCurrentMeeting` also need user input. The registry has no way to describe that, so dispatch would have to special-case each command. **Fix:** add `pub input: InputKind` on `Command`, with variants like `None`, `InlineQuery`, `PromptText`, `CurrentMeetingPath`.

3. **No compile-time coupling between registry and dispatcher.** The PLAN proposed `palette_execute(action_id: String, args: serde_json::Value)` — exactly the stringly-typed drift it claimed to prevent. The invoke list has no handlers for `RenameCurrentMeeting`, `ReprocessCurrentMeeting`, `CopyMeetingMarkdown`, `OpenConfig`, or voice enrollment. **Fix:** derive `Serialize`/`Deserialize` for a tagged `ActionRequest` enum; `palette_execute` matches exhaustively; **do not add registry entries until a concrete backend executor exists**.

4. **`palette_get_context()` cannot produce the `Context` as defined.** `AppState` has `recording`/`live_transcript_active`/`dictation_active` flags but no authoritative `current_meeting` and no `selected_text`. Real session truth is pid-aware and external-process-aware, not just atomic flags — see `cmd_status` merging `state.recording` with `minutes_core::pid::status()`, and `cmd_stop_dictation` checking `dictation_pid_active()`. **Fix:** split context into two sources. `BackendContext` derives only backend-owned state using the same pid-aware logic as `cmd_status`. `UiContext` passes `selected_text` and current meeting path explicitly from the frontend. The palette module should take the union as a `Context` at the filter boundary, not own both halves.

5. **Seed list is wrong.** `StartDictation` with no `StopDictation` is a dead-end. Admin/setup commands (`EnrollMyVoice`, `RunHealthCheck`, `OpenConfig`) are bloat — first-week keyboard users don't reach for these. Missing high-frequency actions already exposed elsewhere: transcript reading (`cmd_live_transcript_read`), research, upcoming meetings. **Fix:** cut the three admin commands, add `StopDictation`, `ReadLiveTranscript`, and one high-frequency navigation/query action like `UpcomingMeetings` or `ResearchTopic`.

**P2 important findings:**

6. **Predicate model underspecified.** `Context` carries `is_dictation_active` but there's no matching `WhenDictationActive` predicate. `WhenMeetingOpen` ignores recording/live/dictation composition — meeting-mutating commands remain visible in conflicting states. **Fix:** add `WhenDictationActive`, or better, replace the single predicate enum with `requires`/`forbids` flag bitmask so "meeting open AND idle" is expressible.

7. **Recent list is lossy.** Plan says to persist executed `ActionId`s, but `as_kebab` erases payloads and `find_by_kebab` reconstructs with `None`. A search recent for "pricing" would come back as "search transcripts" with no query. **Fix:** persist full execution records: `{ "id": "search-transcripts", "query": "pricing" }`.

8. **Several risks are fake.** R3, R5, R10 are restatements of the architecture, not mitigations. Real missing risks: no authoritative current-meeting source, parameterized recents can't round-trip, input-bearing commands are underspecified, half the seed list has no backend executor. **Fix:** replace the fakes with these concrete ones (see updated risk table).

9. **P3 is not drop-in.** Need specific state and handle injection. `AppState` is already managed, so access is available, but `palette_execute` must accept both `AppHandle` and `State<AppState>` because existing commands are not uniform: `cmd_start_recording(app, state, ...)`, `cmd_spawn_terminal(app, state, ...)`, `cmd_start_dictation(app, _state)`. **Fix:** in the plan, stop saying "thin dispatch entry points" and write exact signatures: `palette_execute(app: AppHandle, state: State<AppState>, action: ActionRequest)` and `palette_get_context(ui: PaletteUiContext) -> Context`.

**Verdict:** ActionIdTemplate must go. Dispatcher typing must be serde-tagged enums. Seed list must shrink. Findings 1, 3, and 4 are the ones that change the shape of the code — addressing them before going further is mandatory.

### 2026-04-07 — Codex adversarial review of dispatcher slice (slice 1)

Codex re-reviewed after the rewrite + dispatcher landed. ~3.27M tokens. Six legitimate findings:

**P1 critical:**

1. **Compile-time coupling is still fake.** `ActionRequest` is a hand-maintained mirror of `ActionId`. A new `ActionId` variant can land in core without breaking the Tauri build because `palette_execute` matches on `ActionRequest`, not `ActionId`. The exhaustive ActionId match exists only in `#[cfg(test)]` and uses a duplicated kebab table in a `#[cfg(test)] Serialize` impl. **Fix:** delete `ActionRequest` entirely. Make `ActionId` the FFI type. Convert tuple variants (`AddNote(Option<String>)`) to struct variants (`AddNote { text: Option<String> }`). Add `#[derive(Serialize, Deserialize)] #[serde(tag = "id", rename_all = "kebab-case")]` to `ActionId`. `palette_execute` matches on `ActionId` directly. The exhaustive match is now compiler-enforced in production.

2. **`backend_flags()` lies about stoppable live transcript.** During a normal recording, `live_transcript::session_status().active` returns true because it treats the recording sidecar as "active." `backend_flags` then sets `LIVE_TRANSCRIPT`, which makes the palette show "Stop live transcript" — but `cmd_stop_live_transcript` only checks `state.live_transcript_active` and the standalone live PID, so clicking it returns `"No live transcript session active"`. Visibility and executor disagree. **Fix:** in `backend_flags`, only set `LIVE_TRANSCRIPT` when `lt_process_pid` is real (standalone), not when `sidecar_active` is the source.

**P2 important:**

3. **`StartRecording` returns success before preflight runs.** `cmd_start_recording → launch_recording → std::thread::spawn(...)`. `palette_execute` returns `Ok(Null)` immediately while `preflight_recording` runs on the background thread and may fail (mic missing, permissions wrong, call capture blocked). User sees success in the palette, recording silently fails later. **Fix (deferred to slice 2):** run preflight synchronously and return its error, OR return a typed `"starting"` status and emit a separate success/failure event.

4. **`FindRecentDecisions` doesn't filter by 7 days.** Registry says `"Decisions captured in the last 7 days"` but dispatcher passes `since: None`. Returns every decision in the corpus. **Fix:** change registry text to `"Find recent decisions"` (no 7-day claim) — adding the date filter is a separate plumbing job for slice 2.

5. **Scope-cut rationale overstated.** `OpenAssistantWorkspace` and `CopyMeetingMarkdown` don't actually need new core logic. `crate::context::create_workspace(config)` already exists, `open_target` opens paths, `PaletteUiContext` already carries `current_meeting`, and `copy_to_clipboard` already exists in `commands.rs`. **Fix:** wire both now, OR be honest in the dispatcher docs that they were deferred for time, not blocked on core logic.

6. **`palette_execute` returns `Result<serde_json::Value, String>`.** Same function returns `Null`, `{"added":...}`, `{"stopped":...}`, transcript lines, action lists, decision lists, calendar events. Untyped on the response side. **Fix (deferred to slice 2):** add a tagged `ActionResponse` enum to type both sides of the boundary.

**P3 nit:**

7. **Docs stale.** PLAN still says "20 seed commands" and "16 commands" inconsistently in places. README has no palette mention. The only release notes file points to v0.6.0 dictation, not slice 1. **Fix:** update plan numbers and acceptance criteria to slice 1, add a README feature stub (only after slice 2 ships the UI — without UI, palette is invisible to users), draft slice 1 release notes when paired with the next bundled release.

**Verdict on slice 1 fixes-in-this-loop:** Address findings 1, 2, 4, and 5 now (they're surgical). Defer findings 3 and 6 to slice 2 (they need preflight extraction and a typed response enum). Update docs (finding 7) at the end.

### 2026-04-07 — Fixes applied (slice 1 iteration)

| # | Fix | Files |
|---|---|---|
| 1 | Deleted `ActionRequest` mirror entirely. `ActionId` is now the FFI type, derives `Serialize`/`Deserialize` with `#[serde(tag = "id", rename_all = "kebab-case")]`. Tuple variants (`AddNote(Option<String>)`) converted to struct form (`AddNote { text: Option<String> }`). `palette_execute` matches on `ActionId` directly — exhaustive match is production code, not `#[cfg(test)]`. New test `kebab_matches_serde_tag_for_every_variant` asserts `as_kebab()` and the serde tag never drift. | `crates/core/src/palette.rs`, `tauri/src-tauri/src/palette_dispatch.rs` |
| 2 | `backend_flags()` now probes `pid::check_pid_file(&pid::live_transcript_pid_path())` directly instead of `live_transcript::session_status().active`, which included the recording sidecar. Palette no longer surfaces a "Stop live transcript" row that always errors during a regular recording. | `tauri/src-tauri/src/palette_dispatch.rs` |
| 4 | Registry text for `FindRecentDecisions` changed from "Decisions captured in the last 7 days" to "All recorded decisions, newest first". Adding an actual 7-day `since` filter is deferred. | `crates/core/src/palette.rs` |
| 5 | `OpenAssistantWorkspace` and `CopyMeetingMarkdown` wired using existing helpers. `OpenAssistantWorkspace` calls `crate::context::create_workspace` and opens the returned path via `open_target`. `CopyMeetingMarkdown` reads `ui.current_meeting` from `PaletteUiContext`, reads the file, and calls `copy_to_clipboard` (bumped to `pub(crate)`). Registry is now **18 commands**. | `crates/core/src/palette.rs`, `tauri/src-tauri/src/palette_dispatch.rs`, `tauri/src-tauri/src/commands.rs` |
| 3 | **Deferred to slice 2.** Needs extraction of `preflight_recording` from `launch_recording` so it can run synchronously before the spawn. Until then, `StartRecording` in the palette returns immediate `Ok(Null)` and failures surface via the existing notification path. | — |
| 6 | **Deferred to slice 2.** `palette_execute` still returns `Result<serde_json::Value, String>`. A tagged `ActionResponse` enum is the right shape but would drag tests and the (not-yet-existing) UI contract along with it. | — |

**Test status after fixes:** 16 core palette tests (was 12), 5 dispatcher tests (was 5). Core additions: `copy_meeting_markdown_only_when_meeting_open`, `action_id_serializes_with_id_tag`, `action_id_deserializes_from_id_tag`, `kebab_matches_serde_tag_for_every_variant`. The dispatcher's `every_registry_command_has_a_dispatch_arm_via_compiler` test is now redundant-by-design — the compiler enforces exhaustiveness on the real production match — but kept as an intent-documenting smoke test.
