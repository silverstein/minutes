# Agent-driven desktop QA harness

Status: design / proposed. Author note: ideas borrowed from Vercel Labs'
Native SDK (`vercel-labs/native`) — its agent-driven UI automation
(accessibility snapshots + widget control) and its deterministic replay
"verified frame by frame against state fingerprints." We are not adopting that
framework (we ship on Tauri); we are borrowing the two patterns.

## Problem

Every TCC-sensitive desktop feature can only be QA'd by the maintainer, by hand,
on his own Mac: dictation hotkeys, Screen Recording, Input Monitoring,
Accessibility, native call capture. CI cannot validate desktop render or
behavior at all (type checks and Rust unit tests do not catch UI render bugs,
per the pre-commit checklist). This has been the bottleneck behind a string of
recent issues — the dictation double-paste, the TCC-record poisoning, the native
call-capture live-transcript gap — each of which needed a manual real-machine
pass before it could be confirmed or shipped.

The cost is not just latency. It means no regression net: a desktop behavior
that works today can silently break, and nobody notices until the maintainer
next dictates or records.

## The borrow

1. **Agent-driven UI automation.** A structured accessibility snapshot of the
   running app is the assertion surface, and a widget-control channel drives it
   (click, type, press hotkey). An agent (Claude/Codex over the tailnet, exactly
   how desktop debugging already happens in this project) can then drive a
   feature and assert against the snapshot, removing the human from routine
   desktop QA.
2. **Deterministic replay against state fingerprints.** Record a session as
   (input events + an accessibility-snapshot fingerprint at each step). Replay it
   headlessly and assert each fingerprint matches. A drift is a regression. This
   turns a manual "does dictation still paste once?" check into a stored,
   re-runnable oracle.

## What already exists to build on

- `crates/core/src/desktop_control.rs`: a file-based request/response control
  plane (`write_request` / `write_response`, request/response dirs,
  `desktop_app_status`). This is already the spine of a control channel — the
  harness extends it rather than inventing one.
- `scripts/diagnose-desktop-hotkey.sh`: existing native hotkey sanity check.
- The dev-app identity (`Minutes Dev.app`, `com.useminutes.desktop.dev`): the
  canonical, TCC-stable surface for this work, so the harness never touches the
  production app.
- The tailnet + `ssh silverbook-mac`: agents already drive the Mac remotely;
  the harness formalizes what is currently ad-hoc.

## Architecture (phased)

1. **Accessibility snapshot + fingerprint.** Extract the app's AX tree (macOS AX
   API; the Swift-helper pattern already used for call capture) into a
   normalized JSON snapshot. Fingerprint = a stable hash over the
   behavior-relevant subset (focused element, visible text, control states),
   deliberately excluding volatile pixels/timestamps. This is the assertion
   primitive; everything else builds on it.
2. **Widget control.** Extend `desktop_control.rs`'s request/response channel
   with click/type/hotkey/quit primitives targeting AX elements by stable id.
   Driven from Rust or from an agent over the tailnet.
3. **Record / replay.** `record` journals (input, fingerprint) per step;
   `replay` re-drives the inputs and asserts fingerprints, failing on drift.
   Scenarios stored as fixtures.
4. **Agent-driven QA scenarios.** Encode the TCC-sensitive flows (dictation
   press-and-paste, hotkey delivery, call-detect start, menu-bar state) as
   scenarios an agent runs on the dev app and asserts, so a routine desktop pass
   is "run the scenarios" instead of "ask the maintainer to try it."

## Crossover: the Sidekick eval harness

The fingerprint-replay layer is the same primitive the Sidekick eval harness
(the no-Mat-required replay + fault-injection rig) needs to become a regression
oracle rather than a re-run: fingerprint Sidekick's decision state per replay
step and assert against it, so a prompt or provider change that drifts behavior
is caught deterministically. Build the fingerprint layer once; use it for both
desktop QA and Sidekick eval.

## Honest constraints

- The harness still runs on a real Mac with real TCC grants — TCC cannot be
  faked. What changes is that an *agent* drives it (remotely, deterministically,
  repeatably) instead of the maintainer driving it by hand each time.
- AX-tree access needs the Accessibility grant the app already requests.
- This is a real multi-phase build, not a quick add. Phase 1 (snapshot +
  fingerprint) is the load-bearing piece and is independently useful.

## Why this is worth it

The maintainer is the single bottleneck for every desktop change. This is the
one investment that removes him from the routine loop and adds a regression net
where there is currently none — and it pays a second dividend by hardening the
Sidekick eval. It is the highest-leverage borrow from the Native SDK for a
Tauri-based shipping product.
