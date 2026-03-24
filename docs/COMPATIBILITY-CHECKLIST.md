# Minutes Compatibility Checklist

This document captures the expected user-facing semantics across the main
Minutes surfaces:

- Tauri menu bar app
- CLI
- MCP server
- Claude plugin skills/hooks

The goal is not pixel parity. The goal is **behavioral parity**:

- the same recording should have one truth
- empty states should be explicit, not ambiguous
- recovery should remain possible across surfaces
- durable markdown artifacts should remain the source of truth

## Core State Model

Minutes has three meaningful states:

1. `idle`
2. `recording`
3. `processing`

Shared state sources:

- `~/.minutes/recording.pid`
- `~/.minutes/processing-status.json`
- `~/.minutes/last-result.json`
- durable markdown output in `~/meetings/`

Compatibility rule:

- No surface should claim `idle` while another trustworthy surface would still call the same recording `processing`.

## Start Recording

Expected semantics:

- If no capture is active, starting recording should succeed and create exactly one live session.
- If recording is already active, the surface should return an explicit “already recording” state rather than spawning a second session.

Per surface:

- Tauri:
  - shows recording UI immediately
  - plays start cue if enabled
  - tray icon reflects active recording
- CLI:
  - prints recording start guidance to `stderr`
  - blocks until stop/interrupt
- MCP:
  - returns a clear textual confirmation with PID if recording started
  - returns “already recording” if another surface already owns the session
- Plugin:
  - must route through CLI or MCP semantics, not invent its own session model

## Stop Recording

Expected semantics:

- Stopping a live recording transitions to `processing`, not directly to `idle`, until the markdown artifact is written or a recoverable failure is recorded.

Per surface:

- Tauri:
  - transitions from recording bar to processing bar
  - plays stop cue if enabled
  - keeps user informed until artifact or preserved capture exists
- CLI:
  - signals the active PID
  - waits for shutdown
  - emits JSON result on `stdout`
- MCP:
  - returns saved-path output or explicit failure text
  - must not return a blank response on empty/edge cases
- Plugin:
  - should inherit CLI/MCP stop semantics exactly

## Status

Expected semantics:

- `recording` when live capture is active
- `processing` when capture has ended but artifact creation is still underway
- `idle` when neither of the above is true

Per surface:

- Tauri:
  - polls and renders recording vs processing distinctly
- CLI:
  - `minutes status` emits JSON with recording/processing state
- MCP:
  - `get_status` reports recording vs processing vs idle in human-readable form
- Plugin:
  - any “is Minutes working?” skill should respect the same distinctions

## Success / Output Clarity

Expected semantics:

- Every successful capture flow should expose the saved markdown path.
- If a recoverable audio file is preserved instead of a markdown artifact, the preserved path should be shown explicitly.

Per surface:

- Tauri:
  - shows output notice with open / reveal-in-folder actions
- CLI:
  - prints structured JSON to `stdout`
- MCP:
  - includes saved path in response text
- Plugin:
  - should surface the artifact path or quote it from CLI/MCP output

## Empty States

Expected semantics:

- Search/list/actions should produce explicit empty states.
- Machine-readable consumers should still get an empty collection, not silence.

Per surface:

- CLI:
  - `search`, `list`, and `actions` emit `[]` on `stdout` when empty
  - human-readable empty copy goes to `stderr`
- MCP:
  - returns explicit empty-state text such as “No meetings or memos found.”
- Tauri:
  - renders an intentional empty state or the onboarding-first-artifact flow
- Plugin:
  - should treat empty results as valid outcomes, not failures

## Recovery

Expected semantics:

- Failed or stale captures should remain recoverable.
- Minutes should never silently discard the only useful artifact from a failed flow.

Per surface:

- Tauri:
  - recovery center lists preserved captures, stale live recordings, and failed watcher files
- CLI:
  - failures should clear active-state files while preserving recoverable audio where possible
- MCP:
  - should expose preserved-capture outcomes as explicit failures-with-recovery, not as silent no-ops
- Plugin:
  - should prefer CLI/MCP recovery semantics and not hide preserved paths

## User-Controlled Signals

These optional feedback systems should remain coherent:

- completion notifications
- capture cues

Compatibility rule:

- Optional signals may differ in presentation by surface, but they must never contradict state.

## Current Audit Outcome

As of the latest parity pass:

- shared `processing` state exists across CLI / Tauri / MCP
- empty-state behavior is explicit across CLI and MCP
- saved-artifact clarity exists in desktop, CLI, and MCP
- recovery paths exist and are visible

Residual parity work still worth doing:

- plugin-specific verification against this checklist
- a lightweight scripted smoke pass that exercises these transitions automatically
