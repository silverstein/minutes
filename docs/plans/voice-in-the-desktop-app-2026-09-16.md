# Voice in the desktop app — placement, form factor and shipping order

Date: 2026-09-16
Status: proposal
Related: `docs/rfcs/0007-voice-live.md` (phase 2), `DESIGN.md`

The voice engine is finished and shipped. It runs today only from `minutes talk`.
This is about where it lives in the Mac app, what it looks like, and the order
that gets something usable soonest.

## The decision that changes the RFC

RFC 0007 assumed voice would be a third hold-to-talk shortcut reusing the
dictation hold/lock machine. The survey says that is the wrong interaction and
the most expensive one to build.

True hold-to-talk on macOS needs a modifier-free key through the CGEventTap
backend, and there are exactly two: Caps Lock and `fn`. Quick Thought and
Dictation already claim them. A third hold slot would have no key to offer, and
the standard-chord fallback has less reliable release events over a long hold,
which is precisely the case voice has.

It is also the wrong shape. Dictation is a burst: hold, say one thing, release,
text lands somewhere. Voice is a conversation with turns, tool calls and replies,
lasting minutes. Holding a key through it is absurd.

**Voice should be a toggle: tap to open the session, tap to close it.** The
existing state machine already does this. `handle_release` returns `StartLocked`
for a short tap with no active session, and `Stop` for a tap while one is
running. Voice uses the locked half of a machine that already exists, and never
needs a scarce key.

This also removes the RFC's stated blocker for other platforms: without an echo
canceller, open mic self-interrupts, but a toggled session that defaults to
push-to-talk within it does not.

## Form factor

**A dedicated HUD window, modelled on the Coach HUD, not the dictation pill.**

The dictation overlay is the wrong parent despite being the obvious one. Its
lifecycle assumes you are typing into another app: it hides the main window on
start, and on teardown it sleeps and restores focus to whatever you were in.
Both are right for dictation and wrong for a conversation you are watching. Its
window label is also a singleton that is destroyed and rebuilt on every use, so
sharing it means a voice session and a dictation pill can never coexist.

The Coach HUD (`tauri/src/copilot-hud.html`) is the better precedent on every
axis: it is a thing you watch rather than type through, it is `data-state`
attribute-driven, and it is already `content_protected(true)` unconditionally,
which is correct for a surface that will render meeting content.

**Copy its patterns; do not inherit its code or its surface.** Coach is set
aside as an experiment. Voice takes the window-creation shape, the attribute
driven state rendering and the capture protection, and writes a fresh
`voice-hud.html` with its own window label and its own tray entry. Inheriting
Coach's implementation would mean debugging Coach and building voice at the same
time, with no way to tell which one was failing. Retiring Coach, if that is
wanted, is a clean standalone change and should not ride along with a launch.

What to copy from the dictation overlay instead of inheriting:

- The snapshot contract: `{state, revision, captureStyle}` published on a
  revisioned event with a replay command on load. This is the single best piece
  of machinery in the app and voice needs exactly it.
- The seven-bar waveform driven by a level event.
- The six-way edge anchor with drag-to-remember, under its own config key.
- The expandable text row, for streamed replies.

## What it looks like

Everything here is already decided by `DESIGN.md`; this is the mapping, not new
design.

| Session state | Indicator | Label (meta mono) | Body |
|---|---|---|---|
| Connecting | dim dot | `connecting` | empty |
| Ready | `--capture` dot, still | `ready` | empty, pill is small |
| Listening | `--capture` dot + live waveform | `listening` | streamed user transcript |
| Thinking | `--capture` dot, gentle pulse | the tool in plain words, plus elapsed | last exchange stays |
| Speaking | `--capture` dot + waveform | `speaking` | streamed reply |
| Closed | none | reason | dissolves |

Notes that follow from the design system rather than taste:

- **No spinner for Thinking.** `DESIGN.md` bans spinners and asks for real
  progress. The engine already emits the tool name and a "still running, 8s"
  status; surface that. "checking your calendar, 8s" is honest and is the same
  thing the CLI shows.
- **Capture blue, never red.** Routine capture is a normal instrument state.
  Red stays for errors.
- **Streamed partials are the reveal.** Assistant transcript arrives partial;
  render it as it lands. This is the sanctioned product-honest motion, not a
  decorative entrance.
- Motion 120–170ms, `prefers-reduced-motion` respected, status whispers.
- One human touch per surface, budgeted: the waveform is it.

Size: pill at rest, roughly the dictation overlay's 320×88, expanding when there
is transcript to show. Anchored top-centre by default, draggable, remembered.

## Shipping order

Three PRs, each independently mergeable and each useful on its own. The
prerequisite is not UI and should land alone.

### PR 1 — make it exist in the app

No new window. Voice starts from the tray and reports there.

- Add a `voice-live` passthrough to `tauri/src-tauri/Cargo.toml` and enable it on
  `minutes-core`. **This is the real prerequisite and is not a one-liner:** it
  pulls `tungstenite`, `coreaudio-rs` and `objc2-audio-toolbox` into the bundle,
  which touches binary size, entitlements and notarization. Do this first and
  alone so a signing surprise does not block UI work.
- `cmd_start_voice`, `cmd_stop_voice`, `cmd_voice_status`.
- `voice_active` atomic on `AppState`, an RAII guard mirroring
  `DictationActiveGuard`, and a `try_acquire_voice`.
- **Widen mutual exclusion to four participants.** Recording, dictation and live
  transcript each enumerate the others by hand. Voice joins all of them, and
  `voice_live::refuse_if_recording` must learn about dictation and live too; it
  currently checks recording only.
- `TrayActivity::Voice` with icon, tooltip, stop label; a `voice-toggle` item
  beside `coach-toggle`; localized strings in both locale files.
- Decide where voice sits for `blocks_capture_controls`. Copilot is the
  precedent for a non-capture-owning activity.
- Gemini key through `secret_store.rs`, mirroring the OpenAI-compatible key and
  hydrated at the same startup point.

Ships as: a tray toggle that starts a real voice session, with tray state. Usable,
and it proves the engine works inside the bundle before any pixels exist.

### PR 2 — the HUD

- New window label, added to `window_base_size` and the state-persistence skip
  list. Capabilities already allow `windows: ["*"]`, so no ACL change.
- `voice-hud.html` modelled on `copilot-hud.html`, `data-state` driven.
- Snapshot publish/replay copied from the dictation overlay pattern.
- Map `VoiceLiveEvent` to the table above. The event set already lines up almost
  one to one, including `Level` for the waveform.
- Its own anchor key in `config.ui`.
- `content_protected(true)` unconditionally.

### PR 3 — the shortcut and settings

- `ShortcutSlot::Voice` and its eight match arms, in toggle mode only.
- `is_slot_session_active_fast` must read the new atomic lock-free; it runs on
  the event-tap thread.
- Startup restore alongside the dictation slot.
- Add `'voice'` to `UNIFIED_SLOTS` plus three DOM ids and a default status
  string. The rest of the recorder UI is generic.
- A voice section in the Capture tab, and the first-run cloud consent, which
  does not exist today and which the RFC requires: microphone audio and tool
  results leave the device.

## Risks worth naming before starting

1. **The bundle change is the schedule risk, not the UI.** Three native
   dependencies entering a signed, notarized app is where surprises live. It is
   why PR 1 is alone.
2. **Overlay guard tests assert literal source strings.** `main.rs` greps
   `commands.rs` and `dictation-overlay.html` for exact snippets. Any shared
   refactor breaks them and must update them deliberately.
3. **Every UI step needs a human click-test** in `Minutes Dev.app` per the
   pre-commit checklist, which no agent can do. Plan the handoff.
4. **Echo cancellation is macOS-only.** The desktop surface should not ship to a
   platform that cannot cancel echo, per RFC 0007's own rule. Toggle-plus-
   push-to-talk inside the session is the way that rule gets satisfied elsewhere
   later.

## What this deliberately excludes

Everything on PR 996: sending messages, desktop control, opening web addresses,
the MCP client, agent relaying. The desktop surface ships reading the user's own
memory and nothing else until that branch has its own decision.
