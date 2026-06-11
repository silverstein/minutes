# Plan — Permission Center v2: Calendar, degradation alerts, contextual asks

Author: Claude, 2026-06-11. Successor to `macos-permission-center-2026-05-01.md`
(epic minutes-o9ua, closed). Triggered by #300: the maintainer's own Calendar
grant was silently downgraded to Add-Only during a macOS migration; meeting
reminders died for weeks and every surface said ready.

## What v1 already gives us (build on, don't rebuild)

- `MacPermissionKind` (Microphone, ScreenRecording, InputMonitoring,
  Accessibility, Automation) + `MacPermissionStatus` with the honest state
  set including `StaleOrRestartNeeded` (`crates/core/src/macos_permissions.rs`).
- A permission monitor with cooldown + wake-grace for transient TCC false
  negatives (`commands.rs`, PERMISSION_MONITOR_*).
- Readiness split from settings; System Settings deep links; the v1 product
  contract ("never say grant when the truth is restart/identity/disabled").
- Onboarding that is model-download-first and does NOT front-load permission
  dialogs (already the right shape).
- #301 shipped a Calendar-specific truth path (helper `--status` probe,
  readiness item, tray click-to-fix) as a parallel mechanism.

## The v2 gaps

### 1. Calendar joins the unified model (close the #300 class)
`MacPermissionKind::Calendar` with a status mapping from the #301 helper
probe (full-access → Granted; write-only → a new `PartiallyGranted` status or
Denied-with-detail; not-determined → NotDetermined). The #301 parallel path
folds into the same model/monitor/readiness machinery every other permission
uses. Add-Only is the macOS trap to call out explicitly in copy: the OS
dialog offers it as the default-looking choice and it breaks reads silently.

### 2. Degradation notifications (permissions are not one-time)
The monitor detects state changes but tells no one. v2: when a previously
Granted permission transitions to a non-granted state (and survives the
wake-grace window), fire one native notification naming the consequence,
not the mechanism: "Meeting reminders stopped working — Calendar access
changed. Click to fix." Per-permission consequence copy. Dedupe to one
notification per regression episode; clear on re-grant. This is the piece
nobody else builds and the direct lesson of #300: a grant that worked in
April silently stopped in May and nothing said so.

### 3. Contextual-ask audit (onboarding stays light)
Confirm and document each permission's request moment, with priming copy
shown by US before the macOS dialog appears:
- Microphone → first recording attempt (exists; verify priming copy).
- Screen Recording → first call-capture use (exists via call banner; verify).
- Calendar → first tray meeting lookup or calendar feature use. Priming must
  say: choose "Full Calendar access" — Add-Only breaks reminders. The macOS
  dialog actively offers the trap; our copy defuses it.
- Input Monitoring / Accessibility / Automation → first hotkey / browser-
  detection setup (exists per v1; verify).
First-run onboarding continues to request nothing beyond what its model
download needs. No five-dialog wall.

### 4. Health parity (CLI)
`minutes health` gains the same per-permission truth rows the app readiness
panel has, so headless/CLI users see the same reality. Calendar landed in
#301 already; mic at minimum should report TCC state, not device existence
(v1 fixed the app surface; verify CLI parity).

## Constraints

- Probes must never prompt (status checks are read-only; the macOS dialog
  fires only at the contextual request moments above).
- Same copy discipline as everything else: name the consequence and the fix
  path; never claim legal/compliance anything.
- Adversarial real-machine exit review (the v1 gate, minutes-mkvg pattern):
  fresh, granted, denied, downgraded-while-running, re-granted, sleep/wake,
  prod-vs-dev identity.

## Out of scope

- Windows/Linux permission models (different OS contracts; separate plan).
- Any change to what permissions Minutes uses.
