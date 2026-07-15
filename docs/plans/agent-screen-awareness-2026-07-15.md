# Plan — Agent-Aware Live Screen Context

Status: implemented on `feat/agent-screen-awareness`; tracked by `minutes-id3j`;
rebased onto `origin/main` at `ebca4e1` on 2026-07-15. Signed-Mac dogfood
verified the honest `permission-unavailable` path; the Minutes Dev identity
still needs a refreshed Screen Recording grant before a real PNG can be
captured with that identity.

Implementation decisions resolved on 2026-07-15:

- cleanup hard-deletes screenshot events and directory links after removing files;
- MCP returns direct, bounded image content after CLI and server path validation;
- the dynamic PTY breadcrumb is `CURRENT_SESSION.md`;
- native Claude Recall uses the read-only MCP image tool, while providers without
  image-tool support degrade explicitly rather than receiving ambient attachments;
- v1 is tool-on-demand and does not add an “include latest screen” UI affordance.

Origin: live dogfood during an external partner meeting. Minutes was recording and
successfully writing PNGs under `~/.minutes/screens/current/`, but the active
Codex assistant did not know those images existed until it manually inspected
the config, source tree, recording metadata, and filesystem. Once the latest
PNG was opened explicitly, the assistant could see and reason about the Google
Meet presentation. The model capability worked; the product contract did not.

## Outcome

When screen context is enabled for an active recording, every supported Minutes
agent surface should be able to answer four questions from observed state:

1. Is screen context configured for this session?
2. Is it actually capturing, waiting for its first image, unavailable, or
   stopped?
3. What is the latest relevant screenshot for this session or moment?
4. Has the agent actually inspected an image, or does it only have desktop
   metadata such as app and window titles?

The system must make those answers available without automatically exposing the
user's screen to every query or conflating screenshots with the separate
`desktop_context` collector.

## Product contract

### Capability versus observed reality

The agent must distinguish:

- `screen_context.enabled = true`: user intent to capture recording-time
  screenshots.
- permission/tool readiness: the current process identity can attempt capture.
- active capture state: the current recording started a screenshot worker.
- image availability: at least one screenshot was successfully written and is
  still readable.
- visual awareness: the model received or opened a specific image.

Only the final state permits language such as “I can see the slide.” Config
presence, permission readiness, a directory path, or an app/window event does
not.

### Screen context versus desktop context

The two existing features remain separate and should be named precisely:

- `screen_context`: periodic PNG screenshots during an active recording.
- `desktop_context`: event-first app focus, window title, and opted-in browser
  title metadata during recordings and live sessions.

An agent with only desktop context may say “Google Meet was focused” or quote a
captured window title. It may not infer the slide contents.

### Privacy boundary

- No ambient or 24/7 screen recording.
- No screenshot capture when `screen_context.enabled` is false.
- No image is automatically attached to every Recall message.
- Screen retrieval is read-only, local-first, bounded, and tied to a known
  Minutes context session.
- Retrieval defaults to the nearest one image and caps requests at three.
- Paths must resolve to screenshot artifacts linked to the selected context
  session; arbitrary filesystem reads are not accepted.
- Provider delivery follows the user's configured agent/provider. The UI and
  agent contract must not imply that a local screenshot stays local if the
  configured model is remote.
- `keep_after_summary = false` removes both image files and live retrieval
  references so tools never advertise stale or deleted images.

## Current implementation and verified gaps

### Capture exists

`crates/core/src/capture.rs::start_screen_capture_if_enabled` checks config and
permission, derives the screenshot directory, and calls
`crates/core/src/screen.rs::start_capture`. The capture thread writes
timestamped PNG files and logs success or failure.

The capture function receives the config, audio output path, and stop flag. It
does not receive the active `context_session_id`, a context-store handle, or a
callback that can register successful captures.

### The context schema anticipates screenshots but production does not use it

`crates/core/src/context_store.rs` defines:

- `ContextEventSource::ScreenshotRef`
- `ContextLinkKind::ScreenshotDirectory`
- an `artifact_path` on context events

The only verified `ScreenshotRef` append is in a context-store unit test. A
repo-wide search found no production append and no production screenshot-
directory link. The architecture document says screenshot references live in
`context.db`, but actual recording-time PNGs currently bypass that contract.

### Full PTY assistant discovery is incomplete

`tauri/src-tauri/src/context.rs::generate_assistant_context` generates the
workspace `AGENTS.md` and `CLAUDE.md`. Its CLI inventory includes recording,
transcript, search, and QMD commands, but omits:

- `minutes context activity-summary`
- `minutes context search`
- `minutes context get-moment`
- screen-context state and retrieval
- the distinction between screenshot and desktop metadata
- the requirement to verify an observed image before claiming sight

The live-coaching instruction only controls response style. It supplies no
screen-awareness workflow.

### Native Recall is a separate surface

Native Recall deliberately runs from a neutral chat workspace rather than the
full PTY assistant workspace. It writes its own static
`CHAT_WORKSPACE_CLAUDE_MD`, injects focused meeting text plus keyword search
excerpts, and gives Claude a read-only MCP allowlist.

`crates/core/src/summarize.rs::build_chat_invocation` explicitly passes an empty
screenshot list with the comment that chat has no screenshots to deliver.
Changing only the generated PTY `AGENTS.md` therefore cannot fix native Recall.

### Existing MCP context tools are text-first

`activity_summary`, `search_context`, and `get_moment` expose app/window events
through CLI and MCP. Their MCP responses contain text and structured JSON, but
no image content and, because capture does not register screenshot events, no
reliable screenshot artifact path.

### Existing provider adapters can deliver screenshots

The batch summarization path already knows how to deliver image files to
different agent CLIs. In particular:

- Claude receives the `Read` tool plus an allowed screenshot directory.
- Codex receives native image attachments.

This machinery should be reused or factored into a shared image-delivery helper
instead of creating a second provider-specific implementation.

## Architecture

### 1. Canonical session-linked screen state

Add a core screen-context state model owned by the recording lifecycle:

```text
off
configured
permission-unavailable
waiting-for-first-capture
capturing
capture-degraded
stopped
cleaned
```

The state record should include:

- `context_session_id`
- recording mode and source
- screenshot directory link, when created
- configured interval
- worker start/stop timestamps
- last attempt timestamp
- last successful capture timestamp
- successful capture count
- most recent error, when any
- retention state (`ephemeral`, `retained`, or `cleaned`)

This is observed runtime state, not a restatement of config. It must be written
by the same process identity that starts capture so macOS TCC identity
differences cannot produce false readiness claims.

### 2. Context-store linkage

Thread the active `context_session_id` into screen capture startup. At worker
start:

1. Create the screenshot directory with existing owner-only permissions.
2. Link it to the context session with
   `ContextLinkKind::ScreenshotDirectory`.
3. Set runtime state to `waiting-for-first-capture`.

After each successful PNG write, append a context event:

```text
source: ScreenshotRef
observed_at: wall-clock capture time
artifact_path: canonical PNG path
privacy_scope: Normal or the active filtered scope
metadata:
  capture_index
  elapsed_seconds
  width/height when cheaply available
  byte_size
```

If current app/window metadata can be joined by timestamp without blocking the
capture loop, enrich the event. Failure to enrich must never prevent the image
from being registered.

Do not write image bytes into SQLite. `context.db` remains an index and linkage
surface; PNGs remain owner-only files.

The capture thread should send lightweight success/failure messages to its
owner. Context-store writes and state-file updates should happen outside the
platform screenshot call so SQLite latency cannot stall capture.

### 3. Honest cleanup

When `keep_after_summary = false`, cleanup must be one logical operation:

1. Stop and join the capture worker.
2. Delete the PNG files and screenshot directory using the existing cleanup
   path.
3. Remove screenshot-reference events and the screenshot-directory link, or
   atomically mark them unavailable with no readable artifact path.
4. Set runtime state to `cleaned` and retain only non-sensitive aggregate facts
   needed for diagnostics, such as count and cleanup timestamp.

The implementation should choose one store contract—hard deletion or an
explicit tombstone—and test it end to end. Retrieval must never return a path
that it describes as readable after cleanup.

When `keep_after_summary = true`, retained refs remain linked to the completed
session and can be resolved later by meeting path, session ID, or timestamp.

### 4. Core retrieval contract

Add a provider-neutral core query, conceptually:

```text
get_screen_context(
  session_id | linked_path | timestamp,
  limit = 1,
  before/after window
) -> ScreenContextResult
```

`ScreenContextResult` includes:

- observed screen state
- selected context session and time window
- zero to three verified, readable screenshot refs
- capture timestamps and distance from the requested moment
- whether each file still exists and passed path validation
- a clear reason when no image is available

Selection should favor the nearest successful capture at or before the anchor,
then the nearest after it. This is more useful for questions such as “what was
on screen when we made that decision?” than simply returning the newest file.

### 5. CLI and MCP surfaces

Add or extend small, opinionated retrieval surfaces rather than a generic file
API:

- `minutes context status --json`
  reports current observed desktop and screen-context state.
- `minutes context screen --session <id> --at <time> --limit 1 --json`
  returns bounded metadata and local paths for trusted local clients.
- MCP `get_screen_context`
  returns a concise text state plus structured metadata and image content for
  verified refs when the client supports images.

MCP validation must reject:

- paths not linked to the selected session
- paths outside the expected screenshot root after canonicalization
- missing files
- unsupported file types
- requests above the maximum image count

If an MCP client cannot consume images, it should still receive an honest text
state and timestamps, not a claim that the image was visually inspected.

### 6. Full PTY agent contract

Extend generated `AGENTS.md` and `CLAUDE.md` with a section named
`Live Screen and Desktop Context`. It should tell the agent:

- the features are opt-in and separate;
- how to check current observed state;
- how to retrieve screen context for the active session or a timestamp;
- to inspect the image before describing visible content;
- to use visual context when the user explicitly references the screen, asks
  whether the feed is visible, or requests live meeting strategy that depends
  on the presentation;
- to avoid describing unrelated private material visible elsewhere;
- how disabled, waiting, permission-denied, degraded, stopped, and cleaned
  states should be reported.

Add a lightweight dynamic workspace artifact such as `CURRENT_SESSION.md`,
updated by recording start, first successful capture, failure, stop, and
cleanup. The generated instructions tell PTY agents to read it when present.
It should contain state and safe references, not image bytes or transcript
content.

This file solves “right off the bat” discovery for an agent that opens halfway
through a meeting. The CLI remains the authoritative refresh path if the file
is stale or absent.

### 7. Native Recall contract

Native Recall must receive the same observed state even though it intentionally
does not load the PTY workspace instructions.

Update `CHAT_WORKSPACE_CLAUDE_MD` to advertise the bounded screen-context tool
and the visual-claim rule. Inject a short current-session state block into each
message while a recording is active.

Preferred v1 behavior:

- The prompt tells the model that a screen image is available without embedding
  it automatically.
- The model calls the read-only `get_screen_context` tool when the question
  depends on visible content.
- An explicit Recall affordance may later let the user choose “include latest
  screen,” but that is not required for the first implementation.

If provider/client limitations make MCP image content unreliable, pass only the
selected verified files into the existing agent invocation image-delivery path.
That fallback must be gated by an explicit screen-related user request or UI
action, not every Recall message.

### 8. Copilot integration boundary

The first-class real-time copilot has its own runtime and prompt construction.
It should consume the same core `ScreenContextResult` and observed state rather
than tailing the screenshot directory or inventing a second session resolver.

Copilot integration can land after the core, PTY, and native Recall slices, but
the core interface should be stable enough that copilot does not need a
filesystem-specific adapter.

## Implementation slices

### Slice A — observed state and session linkage

Primary files:

- `crates/core/src/capture.rs`
- `crates/core/src/screen.rs`
- `crates/core/src/context_store.rs`
- recording/job cleanup code that honors `keep_after_summary`

Deliverable: successful captures become canonical session-linked context, and
runtime state distinguishes waiting, capturing, degraded, stopped, and cleaned.

### Slice B — bounded retrieval and adapters

Primary files:

- context-store/core query modules
- CLI `context` commands and capability advertisement
- `crates/mcp/src/index.ts`
- MCP tool tests and manifest/capability fixtures

Deliverable: local CLI and MCP clients can retrieve the correct image nearest a
session moment without receiving arbitrary filesystem access.

### Slice C — agent discovery

Primary files:

- `tauri/src-tauri/src/context.rs`
- workspace state-file lifecycle
- generated `AGENTS.md` / `CLAUDE.md` tests

Deliverable: a newly opened full PTY assistant immediately knows screen context
may exist, sees current observed state, and has exact retrieval instructions.

### Slice D — native Recall

Primary files:

- `tauri/src-tauri/src/commands.rs`
- `crates/core/src/summarize.rs`
- native Recall integration tests

Deliverable: native Recall sees the active state and can obtain a relevant image
through the read-only path without broad file access or unconditional image
attachment.

### Slice E — real-machine UX and privacy validation

Use the signed `~/Applications/Minutes Dev.app` identity. Do not replace the
production app with ad-hoc builds for TCC-sensitive testing.

Deliverable: visible, truthful transitions and no silent gap between enabled
intent and agent availability.

## Acceptance scenarios

1. Enabled, permission granted, before first interval:
   the agent reports `waiting for first capture`; it does not claim sight.
2. Enabled, permission granted, first PNG succeeds:
   state becomes `capturing`; the session has a directory link and screenshot
   event; “can you see the live feed?” retrieves and inspects that image.
3. Enabled, permission denied or stale TCC grant:
   the desktop and agent report `unavailable` with a concrete fix; recording
   continues without screenshots.
4. Disabled:
   no worker, directory, image event, attachment, or misleading capability
   claim is produced.
5. Desktop context enabled, screen context disabled:
   the agent can report app/window metadata and explicitly says it has no visual
   image.
6. A transient screenshot failure after prior success:
   state becomes degraded while the last successful image remains identifiable
   with its timestamp; recovery returns state to capturing.
7. Recording stops before the first screenshot:
   worker joins promptly and state becomes stopped with zero images.
8. `keep_after_summary = false`:
   files and live refs are cleaned; later retrieval reports cleaned/unavailable,
   never a stale readable path.
9. `keep_after_summary = true`:
   a completed meeting can resolve its retained screenshot nearest a requested
   timestamp.
10. Full PTY assistant starts halfway through a recording:
    generated instructions plus current-session state make the capability
    discoverable without source or filesystem archaeology.
11. Native Recall asks a screen-dependent question:
    it receives the verified image through the bounded path and distinguishes
    what it saw from transcript-derived facts.
12. Native Recall asks an unrelated historical question:
    no current screen image is automatically attached.
13. Path traversal or arbitrary image request:
    CLI/MCP rejects it even if the file exists and is readable by the process.
14. Multi-agent parity:
    generated Claude and agent-agnostic instructions remain identical where
    intended, and provider adapters either support images or degrade explicitly.

## Verification

Automated coverage should include:

- screen worker success/failure messages and shutdown behavior
- directory-link creation exactly once per session
- screenshot events with correct session, timestamps, and artifact paths
- nearest-image selection before/after an anchor
- cleanup behavior for both retention settings
- missing-file and canonical-path rejection
- CLI JSON state contracts
- MCP text plus image response contracts and maximum limits
- generated `AGENTS.md` / `CLAUDE.md` screen-context section and parity
- native Recall instructions and allowlist
- native chat invocation behavior when zero, one, and multiple images are
  selected

Real-Mac dogfood should cover:

- production and signed-dev TCC identities separately
- permission granted, denied, and stale-grant repair
- enabled-to-waiting-to-capturing transition
- live “what is on this slide?” question in full PTY and native Recall
- stop during interval sleep
- cleanup after summarization
- a screen containing unrelated private content to verify bounded, intentional
  use and non-disclosure in the response

Required repo gates follow `docs/checklists/pre-commit.md`, including Rust fmt,
clippy/tests, MCP checks, generated manifest/capability parity, and dev-app click
testing for any changed Tauri surface.

## Rollout and compatibility

- Keep both context features off by default unless a separate product decision
  changes that policy.
- No migration is needed for old meetings without screenshot refs; retrieval
  returns `not captured` or `unavailable` honestly.
- Existing retained screenshot directories may be importable later, but
  backfilling them is not required for v1 because session identity may be
  ambiguous.
- Add capability flags before advertising new MCP tools to mixed-version
  clients.
- Treat image-return support as additive. Text-only clients retain current
  desktop-context behavior.

## Explicit non-goals

- Ambient screen recording or a Screenpipe-style timeline.
- OCR/indexing of every screenshot.
- Continuous video capture or streaming the meeting feed.
- Autonomous clicking or control of the user's screen.
- Accessibility-tree ingestion.
- Solving Linux/Windows capture parity in this issue.
- Replacing transcript-first live coaching; screen context is an additional
  evidence lane.

## Decisions to make before implementation

1. Cleanup representation: hard-delete screenshot events/links or retain an
   explicit tombstone with no artifact path. Privacy and non-stale retrieval are
   non-negotiable either way.
2. MCP image transport: direct image content versus a protected local resource
   URI, validated against the actual clients Minutes supports.
3. Dynamic state artifact name: `CURRENT_SESSION.md` versus extending another
   existing current-focus artifact. It must not overload `CURRENT_MEETING.md`
   with runtime-only capture state unless that lifecycle is made explicit.
4. Native non-Claude Recall parity: whether each supported CLI gets the MCP
   image tool, direct provider attachment, or an explicit unsupported state in
   the first release.
5. Whether the Recall UI needs an “include latest screen” affordance in v1 or
   tool-on-demand is sufficient after dogfood.

These decisions should be resolved in the implementation spec or first slice,
not guessed independently by each adapter.
