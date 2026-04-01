# Durable Call Capture Plan

**Date:** 2026-03-31
**Method:** `plan-eng-review` rubric applied to current Minutes architecture
**Goal:** make call recording feel intuitive and trustworthy, not merely "warn better"

## Executive Summary

For the core "record my call" job, Minutes should behave like this:

- infer that the user is in a call
- capture both the local microphone and remote call audio
- show, in real time, whether both sources are actually live
- refuse silent degradation
- preserve recovery paths when capture quality is low

The key architecture decision is this:

**Call capture should be owned by the Tauri desktop app, not by the detached CLI recording path.**

That is the whole game.

The CLI can remain a great engine for:

- memo capture
- room capture
- manual BlackHole workflows
- processing existing audio files

But "just work" system audio capture on macOS depends on app-bundle permissions and platform APIs. The desktop app is the right owner for that job. MCP and CLI should route into that owner when the user intent is "record this call."

One more product stance (updated 2026-03-31):

**Call capture should be explicit, not automatic.** When a known call app is detected, Minutes shows a banner offering to record the call. The user clicks "Record" on the banner to start call capture. Pressing the normal "Start Recording" button always starts a standard mic recording.

The original plan called for aggressive auto-switching to call intent. In practice, process-based detection has too many false positives (Slack and FaceTime run as background processes on every Mac). Aggressive auto-switch caused broken recordings and stuck UIs when the call-capture sources weren't available. The current stance: detect calls, offer to record them, let the user decide.

## Constraints

This plan must satisfy all of these at once:

1. **No silent bad starts.**
   If Minutes cannot hear both sides of a call, it must say so before or during capture, not after.

2. **Simple jobs stay simple.**
   Voice memos and room recordings should not inherit call-capture complexity.

3. **One product contract across surfaces.**
   Tauri, CLI, and MCP should share the same intent model even if their capture backends differ.

4. **Local-first artifacts remain durable.**
   A call capture should still end as markdown plus recoverable raw audio when quality is low.

5. **Platform truth beats wishful parity.**
   The desktop app can do more than CLI because it has permissions and UI. The plan should acknowledge that instead of pretending every surface is equal.

6. **No architecture fork.**
   We should not build one recorder for Tauri and a different product concept for MCP/CLI. Different backends are fine. Different semantics are not.

7. **Recovery remains first-class.**
   The recent `needs-review` work stays in place and becomes part of the normal low-signal path.

## Product Contract

Minutes should expose three user intents:

1. `memo`
   Use for voice notes and quick thoughts.
   Default capture: microphone only.

2. `room`
   Use for in-person conversations in a physical space.
   Default capture: microphone only.

3. `call`
   Use for Zoom, Teams, Webex, and similar video/audio calls.
   Default capture: microphone + system audio.
   Triggered explicitly via the call detection banner or `--intent call`.

This is the product contract. Device routing is an implementation detail below it.

Additional product principle:

**For calls, Minutes should support multiple capture sources internally, while presenting the experience as one simple "record this call" action.**

That means:

- users should not have to manually create or understand aggregate devices in the common case
- the app should capture mic and call/system audio as distinct logical sources when possible
- advanced source selection can exist for power users, but it should not be the main product surface

The user-facing abstraction is `call`, not "select two devices."

## User Experience Contract

When the user starts a `call` capture, they should immediately see:

- `Mic: live` or `Mic: missing`
- `Call audio: live` or `Call audio: missing`
- which backend is in use
- a clear fallback when dual capture is unavailable

If both sources are live, recording proceeds normally.

If one source is missing, Minutes should not quietly record anyway in the default path. It should present a blocking decision:

- fix setup and retry
- continue with degraded capture knowingly

That is the trust boundary.

When Minutes detects an active call (Zoom, Teams, Webex), it shows a call detection banner. The user can:

- click "Record" on the banner to start call capture with dual sources
- click "Dismiss" to ignore the detection
- press the normal "Start Recording" button for a standard mic recording regardless

This gives us reliable detection with explicit user intent. The `auto_call_intent` config flag exists for users who prefer the aggressive auto-switch path.

Default detection apps: Zoom, Teams, Webex. Slack and FaceTime were removed from defaults because they run as background processes on every Mac, producing constant false positives.

### Browser-based call detection policy

Browser-hosted calls are a separate trust and signal-quality problem from native desktop call apps.

Examples:

- Google Meet in Chrome/Safari
- browser-based Teams or Webex sessions
- any future tab-based calling surface

Current product stance:

- **native desktop call apps may be default-on detection candidates**
- **browser-based integrations should be opt-in until proven trustworthy in dogfood**

Why:

1. Browser signals are usually weaker.
   A tab existing is not the same thing as an active call. Scheduled meeting tabs, landing pages, and stale tabs can all stay open in the background and create false positives.

2. Browser inspection often crosses a stronger privacy boundary.
   On macOS, querying tabs via Apple Events / AppleScript requires automation consent and can fail noisily if we probe too aggressively.

3. The product already learned this lesson the hard way.
   We removed Slack and FaceTime from default detection because "process exists" was too weak a signal. Browser integrations should start from an even more conservative posture, not a more aggressive one.

Implementation policy for browser detection:

- browser integrations must not be added to the default `CallDetectionConfig.apps` list until they are proven reliable in real dogfood
- browser integrations should use the strongest available call-specific signal, not just hostname presence
- on macOS, Apple Events / AppleScript based detection must include permission-aware failure handling and backoff so denied automation paths are not retried every poll forever
- browser detections should be treated as lower-confidence signals than native app detections when deciding whether to surface a call banner

Practical implication:

- `Zoom`, `Teams`, and `Webex` remain good default detection candidates
- `Google Meet` detection is worth supporting, but should begin as an opt-in browser integration rather than a default-enabled detector
- cross-platform browser call detection is a future design problem, not something the macOS AppleScript path should define for the whole product

## Architectural Decision

### 1. Split intent from backend

Today, recording is effectively "capture whatever the default input device is." That is too low-level for the user job.

We should introduce two layers:

- `RecordingIntent`
  - `Memo`
  - `Room`
  - `Call`

- `CaptureRoute`
  - `MicOnly`
  - `SystemOnly`
  - `MicPlusSystem`

Intent is user-facing. Route is implementation-facing.

Intent inference policy:

- if the user explicitly chose an intent (e.g. clicked "Record" on call banner), respect it
- if the user presses the normal "Start Recording" button, always use `Room` or `Memo`
- `auto_call_intent` config exists for users who want the aggressive path, but defaults to `false`

The original plan called for aggressive auto-inference. Field testing showed process-based detection is too noisy for that. The architecture supports it (the flag is one config change), but the default is conservative.

### 2. Make the Tauri app the system-audio capture owner

The Tauri app should own native call capture because it can:

- request and retain platform permissions
- show source-level meters and setup state
- react to route changes live
- use app-bundle-only APIs cleanly

The CLI remains valuable, but it should not be the primary owner of "just work" call capture on macOS.

### 3. Route MCP call recordings through the desktop app when available

Today MCP starts capture by spawning the CLI detached from [crates/mcp/src/index.ts](/Users/silverbook/Sites/minutes/crates/mcp/src/index.ts#L521).

That is fine for mic capture. It is the wrong long-term owner for native call capture.

Durable plan:

- if the Tauri app is running, MCP `start_recording(intent=call)` should send a local command to the app
- the app starts the entitled capture session
- MCP becomes a controller, not the recorder

Fallback:

- if the app is not running, MCP can still use CLI for memo/room capture
- for call capture without the app, MCP should either:
  - require explicit degraded-mode confirmation, or
  - require a manual backend like BlackHole

## Backend Strategy

We should design for multiple system-audio backends behind one interface:

```rust
enum RecordingIntent {
    Memo,
    Room,
    Call,
}

enum CaptureRoute {
    MicOnly,
    SystemOnly,
    MicPlusSystem,
}

struct CapturePreflight {
    intent: RecordingIntent,
    route: CaptureRoute,
    mic_ready: bool,
    system_ready: bool,
    selected_mic: Option<String>,
    selected_system_backend: Option<String>,
    blocking_reason: Option<String>,
    warnings: Vec<String>,
}
```

System-audio backend priority on macOS:

1. Native app backend
   Preferred path for Tauri app.
   This should support app-level system audio capture and explicit permission checks.

2. Manual virtual-device backend
   BlackHole and similar.
   Best fallback for CLI/headless/power users.

3. No system-audio backend
   Allowed only with degraded-mode consent.

The important point is not the exact API choice on day one. The important point is that the product contract and preflight model should not care whether the system audio came from a native tap or BlackHole.

Just as important: multi-source capture should remain explicit in the internal architecture.

For `call` intent, the backend should aim to preserve at least two logical inputs:

- `local_mic`
- `remote_or_system_audio`

Whether those become:

- two files
- two tracks in one container
- a mixed artifact plus per-source metadata

is an implementation decision. But we should keep the architecture open to source-aware processing instead of collapsing everything to "whatever the selected input device produced."

Why this matters:

- cleaner speaker separation
- better overlap handling
- better debugging when one side is missing
- future options for source-aware diarization or lighter downstream processing

The first goal is reliability, not compute savings. If multi-source capture later lets us use a smaller model or skip some diarization work, great. That should be a second-order optimization, not the reason we build it.

## Source Health and Live Meters

Minutes should stop pretending there is one audio level for a call.

For `call` intent, the app should show:

- mic level meter
- system-audio level meter
- combined recording level meter

If system audio is flat while the user is in a call, that should be visible within seconds.

This also gives us a much better support story:

- "Mic was live, call audio was dead"
- "System audio was active, mic was muted"
- "Both were active but clipping"

That is actionable. "Blank transcript" is not.

## Start Flow

### Tauri

`cmd_start_recording` should accept a richer payload:

```rust
struct StartRecordingRequest {
    intent: Option<String>,          // memo | room | call
    allow_degraded: Option<bool>,    // default false for call
    mic_device: Option<String>,
    system_backend: Option<String>,  // auto | native | blackhole
}
```

Flow:

1. determine intent from the initiating surface
   - call detection banner → `call`
   - normal "Start Recording" button → `room` or `memo`
   - optional `auto_call_intent` config may override this for users who explicitly opt in
2. run preflight
3. if intent is `call` and dual capture is unavailable:
   - show blocking setup/fallback UI
4. start capture session
5. stream source health into the UI

### CLI

Keep CLI honest:

```bash
minutes record --intent memo
minutes record --intent room
minutes record --intent call
```

For `--intent call`:

- if a system-audio backend is configured, use it
- otherwise stop with a clear explanation unless `--allow-degraded` is passed

CLI does not need to be magical. It needs to be truthful.

### MCP

Add intent and degraded-mode semantics:

```ts
start_recording({
  title?: string,
  intent?: "memo" | "room" | "call",
  allow_degraded?: boolean
})
```

Behavior:

- `intent=call` prefers Tauri delegation when available
- if no desktop owner is available, the tool should not silently fall back to mic-only capture
- if the user omits `intent` but a call app is active, default to `call`

## Recording Pipeline Ownership

We should separate **capture** from **processing** even more clearly:

- Capture session
  - owns source acquisition and live health
- Processing job
  - owns transcription, diarization, summarization, artifact writing

The queue work we already did was the right move.

Next step is to make capture session metadata richer:

- intent
- route
- mic device name
- system backend name
- per-source readiness at start
- per-source dropout events during session

That metadata should flow into frontmatter or a sidecar for debugging and support.

## Artifact Contract

Every call capture should carry enough provenance to explain what happened later.

Recommended frontmatter additions or sidecar fields:

- `capture_intent: call | room | memo`
- `capture_route: mic-only | system-only | mic-plus-system`
- `mic_device: ...`
- `system_backend: native | blackhole | none`
- `capture_health: ok | degraded | needs-review`
- `capture_warnings: [...]`

This matters because support, debugging, and trust all come from being able to answer:
"What exactly did we record?"

## Failure Modes We Must Handle

1. **Call app active, only mic source available**
   Default behavior: block or explicit degraded confirmation.

2. **Mic active, system audio disappears mid-call**
   Show in-session warning. Do not wait for post-processing.

3. **AirPods or output route changes mid-call**
   Re-run system backend readiness and update live health.

4. **Notification sounds get mixed into call audio**
   Warn in setup docs, optionally suppress with Focus recommendation.

5. **Double local voice**
   If the call app feeds local sidetone into system audio, users may hear duplication in the mixed track. This is real, but it is **not a launch blocker** for dual-source call capture. Defer echo cancellation, source separation, or per-source export to post-launch quality work unless it makes transcripts unusable in practice.

6. **Permission denied for system audio backend**
   Treat as setup failure, not as "recording started."

7. **User starts recording from MCP while app is closed**
   MCP should either launch/attach to app for `call` intent or refuse silent fallback.

8. **No-speech or low-signal artifact**
   Already addressed by `needs-review` and preserved raw audio. Keep that path.

## Rollout Plan

### Phase 1A: Product contract and shared preflight

**Status:** Complete (product stance revised)

What shipped:

- first-class `RecordingIntent` (Memo, Room, Call)
- shared `CapturePreflight` with blocking degraded-mode policy
- source-aware wording and behavior across CLI, MCP, and Tauri
- capture provenance in artifacts

Product stance change (v0.9.2):

- `auto_call_intent` defaults to `false` — normal recordings never auto-route into call capture
- Slack and FaceTime removed from default detection list (always-running background processes)
- Call capture is triggered explicitly via the call detection banner or `--intent call`
- The architecture still supports aggressive auto-switch (one config change), but the default is conservative after field testing showed process-based detection produces too many false positives

Exit criteria (met):

- no call recording can start silently in mic-only mode unless the user explicitly accepts degradation

### Phase 1B: Native Tauri system-audio proof of concept

**Status:** Complete at proof-of-concept level

What shipped:

- app-owned native macOS call-capture helper path
- bundled helper packaging in the desktop app
- queue handoff into the existing processing pipeline
- MCP delegation to the running desktop app for `call` intent

What is still not proven at product level:

- real-world call dogfooding across Zoom / Teams / Meet
- broad permission-path validation on multiple macOS setups
- final packaging / release confidence outside local development

Ship:

- app-owned native system-audio capture path
- permission handling for the native path
- proof that a supported macOS setup can record both sides of a call without BlackHole

Exit criteria:

- desktop Minutes can successfully start dual capture on at least one supported native path
- the product no longer feels like "a warning system for a missing feature"

### Phase 2: Tauri source health UI

**Status:** Partially shipped, design under active revision

What shipped:

- source-aware backend liveness for `mic_live` and `call_audio_live`
- health chips showing "Mic live/missing" and "Call audio live/missing" during call capture
- basic status exposure through desktop app state

Design feedback (v0.9.2):

- health chips are only shown during call-capture recordings (not normal mic recordings)
- current chip design was flagged as visually heavy/alarming in design review
- the health UI needs refinement to feel like status, not like errors

What remains:

- dedicated visual meters (not just text chips)
- clearer degraded-state affordances that feel like status, not alarms
- route-drop warnings during capture
- design pass on the call-capture recording bar to match the calmer normal recording bar

Exit criteria:

- a user can tell within 5 seconds whether Minutes hears both sides of the call
- the health UI feels informational, not alarming

### Phase 3: MCP delegation to desktop owner

**Status:** Started early, core handoff implemented

What shipped:

- desktop-control heartbeat and request/response files
- MCP `call` intent delegation to the running desktop app

What remains:

- real dogfood validation of the end-to-end Claude Desktop flow
- stronger acknowledgement / timeout ergonomics after field testing

Ship:

- local handoff from MCP to Tauri app for `call` intent
- status reflection back into MCP tool responses

Exit criteria:

- Claude Desktop call recording uses the same capture owner as the desktop UI

### Phase 4: CLI and power-user fallback

**Status:** Partially complete

What shipped:

- honest `--intent call`
- `--allow-degraded`
- explicit preflight failure before silent bad starts
- `auto_call_intent` config flag for users who want the aggressive auto-switch path

What remains:

- better power-user setup guidance
- more polished backend-selection diagnostics
- future Windows/Linux-specific fallback paths
- smarter call detection that checks for active audio sessions, not just running processes

Exit criteria:

- CLI remains useful and honest even if it is not the "just work" call path

## Testing Plan

### Unit tests

- intent to route selection
- preflight decision matrix
- degraded-mode blocking rules
- state transitions for source dropouts

### Integration tests

- `call` intent without system backend refuses start
- `call` intent with `--allow-degraded` starts and records warning metadata
- `NoSpeech` call capture ends in `needs-review` with preserved raw audio

### Manual verification

- Teams on speakers
- Teams on headphones
- Meet in Chrome
- FaceTime
- AirPods route switch mid-call
- app permission denied
- MCP-initiated call recording with app running
- MCP-initiated call recording with app closed

### Metrics worth logging

- starts by intent
- degraded starts accepted vs blocked
- per-source missing at start
- per-source dropout mid-session
- `needs-review` rate for call captures

If we do not measure degraded starts and missing-source starts, we will not know whether the product is actually getting better.

## What We Should Not Do

1. Do not make "warning better" the end state.
   That is a band-aid, not the product.

2. Do not pretend CLI and Tauri have identical capabilities.
   They do not. That is fine.

3. Do not ship silent fallback for `call` intent.
   That is exactly how trust gets burned.

4. Do not make users think in terms of `BlackHole 2ch` unless they are in fallback land.
   That is implementation leakage.

5. Do not let echo cancellation, local sidetone cleanup, or perfect source mixing block the first shippable version of dual-source call capture.
   Those are quality improvements, not the first unlock.

## Recommendation

Build around **intent-driven call capture owned by the Tauri app**, with shared preflight and truthful fallback across all surfaces.

That gets us to a product that actually feels intuitive:

- simple jobs stay simple
- call capture behaves like a call recorder
- the app tells the truth before it fails
- MCP becomes a smart controller, not a second-class recorder

That is the durable path.
