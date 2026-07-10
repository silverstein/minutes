# macOS Permission Center Plan - 2026-05-01

Scope: `minutes-o9ua` and child beads.

## Problem

The `fn` dictation shortcut bug exposed a broader permission story problem:
Minutes currently mixes OS permission state, runtime usability, feature
readiness, and stale settings UI paths.

The user-visible failure mode is worse than a missing permission. Minutes can
ask the user to grant something that System Settings already shows as enabled,
or describe a feature as ready because a device exists while the actual TCC
permission has not been proven.

Current evidence:

- `tauri/src/index.html` still contains a legacy dictation hotkey block for
  `settings-dictation-hotkey*` DOM paths after the unified shortcut UI replaced
  that surface. The recent null guard prevents a crash, but the dead path is
  still a maintenance hazard.
- `tauri/src-tauri/src/commands.rs` has `microphone_status()` reporting
  "Microphone & audio input" readiness from device enumeration, not from
  microphone authorization.
- `crates/core/src/hotkey_macos.rs` has legacy naming where
  `prompt_accessibility_permission()` opens Input Monitoring settings.
- Patternwork's Screenpipe fork has a better permission shape: explicit
  permission states, a runtime monitor, transition dedupe, cooldowns, and
  wake-grace handling for transient TCC false negatives.

## Product Contract

Minutes should never say "grant permission" when the true condition is one of:

- the permission is granted but the running process needs a restart
- the user granted a different app identity
- the required device or backend is missing
- the feature is disabled
- macOS has returned a transient false negative after wake

Every permission row must answer:

- What does this permission unlock?
- Is it required or optional for the selected feature?
- What does macOS currently report?
- Can this exact running process actually use the API?
- What is the safest next action?

## Permission Model

Add one typed backend surface for macOS-sensitive permissions.

Target permissions:

- Microphone
- Screen Recording
- Input Monitoring
- Accessibility
- Automation / Apple Events where it affects browser/calendar/call detection

Suggested serialized row:

```json
{
  "kind": "inputMonitoring",
  "label": "Input Monitoring",
  "status": "granted",
  "runtimeUsable": true,
  "optional": true,
  "requiredFor": ["Caps Lock or fn dictation shortcut"],
  "detail": "Minutes can observe the selected shortcut.",
  "settingsUrl": "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
  "canOpenSettings": true,
  "canRequest": false,
  "restartRecommended": false,
  "restartBlockedBy": []
}
```

Status values:

- `granted`
- `denied`
- `not_determined`
- `not_needed`
- `unsupported`
- `stale_or_restart_needed`
- `unknown`

The key distinction is `status` versus `runtimeUsable`. A row can be
`granted` but not runtime-usable if macOS has not refreshed the current process
or if the app identity in System Settings is not the identity currently running.

## Readiness Split

Readiness checks must not impersonate permission checks.

Separate these pairs:

| Permission truth | Feature readiness |
| --- | --- |
| Microphone TCC authorization | input device exists and selected device is available |
| Screen Recording TCC authorization | ScreenCaptureKit backend available for this OS/build |
| Input Monitoring TCC authorization | selected shortcut can actually register/probe |
| Accessibility authorization | desktop context enabled, title/browser context requested |
| Automation authorization | browser/calendar probe enabled and not backed off |

The Settings pane can still group related rows, but the data model should keep
the distinction strict.

## Restart Policy

No automatic restart for permission recovery.

Restart is a user action, not an implementation detail. The app may recommend a
restart only when the permission model reports stale runtime state or identity
mismatch, and the user must explicitly choose it.

Before offering restart, compute a restart safety view from existing runtime
state:

- recording or recording startup: block restart
- live transcript active: block restart
- dictation active: block restart
- native call capture active: block restart
- processing/transcription active: block restart or offer "restart after this
  finishes" if that queueing exists
- update/install active: block restart
- Recall/assistant PTY active: allow only after explicit confirmation that the
  session/window will close
- idle: allow restart

The existing state signals are close to enough:

- `AppState.recording`
- `AppState.starting`
- `AppState.processing`
- `AppState.live_transcript_active`
- `AppState.dictation_active`
- `AppState.pty_manager.assistant_session_id()`
- updater install state around `cmd_install_update`

Do not hide the Restart button entirely when blocked. Show the reason so the
user learns what is protecting their work.

## UI Policy

Permission UI copy must be state-specific:

- Missing permission: "Open System Settings and enable Minutes."
- Granted but stale: "macOS may not refresh this permission until Minutes
  restarts."
- Wrong identity suspected: "Make sure the enabled app is this installed
  Minutes app, not a raw build or another copy."
- Device missing: "No input device detected."
- Backend unavailable: "This Mac/build does not support this capture backend."
- Feature disabled: "Turn on this feature to use the permission."

For Input Monitoring, include reveal-app guidance because macOS may make the
user choose an app bundle manually. Prefer the signed dev/prod app identity,
not raw `target/` binaries.

## Architecture Slices

1. `minutes-nt6i` - Add structured macOS permission model

   Build the typed permission rows and command surface. Add focused tests for
   serialization, status classification, and runtime-usability flags.

2. `minutes-hkhu` - Split permission state from readiness in Settings

   Refactor Settings/readiness to consume the model. Keep device/model/backend
   checks as separate readiness rows.

3. `minutes-wx2y` - Add session-safe restart and permission recovery actions

   Add restart safety computation and explicit restart/recovery actions. Block
   or confirm restart based on active work.

4. `minutes-frc7` - Remove legacy dictation hotkey settings paths

   Remove active frontend dependencies on old `settings-dictation-hotkey*`
   DOM paths and clean up misleading Accessibility/Input Monitoring naming.

5. `minutes-isep` - Add permission monitor cooldown and wake-grace behavior

   Add polling/reconciliation only after the static model is in place. Dedup
   transitions, emit restoration promptly, and suppress wake flapping.

6. `minutes-mkvg` - Adversarial review macOS permission flows

   Exit gate after implementation. Do not call the permission story stable
   until this passes.

## Adversarial Review Matrix

Run with `~/Applications/Minutes Dev.app`, installed through:

```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
./scripts/install-dev-app.sh --no-open
```

Exercise:

- fresh/no grants
- already granted before launch
- denied permission
- grant while app is running
- revoke while app is running
- re-grant while app is running
- sleep/wake
- dev app identity versus production app identity
- Caps Lock shortcut
- `fn` shortcut
- recording active while restart is recommended
- live transcript active while restart is recommended
- call capture active while restart is recommended
- processing active while restart is recommended
- Recall/assistant PTY active while restart is recommended

Pass criteria:

- no permission row tells the user to grant a permission that runtime proof
  shows is already granted
- stale state is labeled as restart/identity mismatch, not denial
- restart never occurs without explicit user action
- restart cannot interrupt recording, live transcript, call capture,
  dictation, processing, or update install
- Recall/assistant restart path requires explicit confirmation
- all P1/P2 findings are fixed or filed before `minutes-o9ua` closes

## Non-Goals

- Do not solve TCC by adding entitlements that do not apply. Runtime TCC grants
  remain runtime grants.
- Do not make raw-key dictation the default path. Standard shortcuts should
  remain the low-permission default.
- Do not reset a user's TCC database automatically.
- Do not restart automatically after permission changes.
- Do not require Screen Recording for hotkeys.

## Open Questions

- Should Automation / Apple Events be part of the first backend model, or land
  as a follow-up once the four core TCC permissions are stable?
- Should processing get a "restart after current job finishes" queue, or should
  the first pass simply disable restart until processing is idle?
- Should the permission monitor live in `minutes-core` for CLI reuse, or in the
  Tauri app where event emission and restart actions live?

