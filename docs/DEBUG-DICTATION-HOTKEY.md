# DEBUG: Dictation Hotkey Freezes App

## Current status

This document started as a freeze-debugging note. That original UI-thread
freeze path has since been refactored away.

The current macOS blocker is different:

- the native hotkey uses `CGEventTap`, which requires Input Monitoring
- macOS TCC gets confused if we test from multiple app identities
- repo-local bundles, raw `target/` binaries, and freshly rebuilt ad-hoc apps
  are not reliable identities for permission debugging

Use the dedicated dev app install flow instead:

```bash
./scripts/install-dev-app.sh
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app"
```

See [docs/DESKTOP-DEVELOPMENT.md](/docs/DESKTOP-DEVELOPMENT.md)
for the current canonical workflow.

## Problem
When the user toggles the dictation hotkey "On" in Settings > Dictation, the Minutes Tauri app freezes completely (macOS shows "Non-responsive application"). This happens every time.

## What We're Trying To Do
Add a native macOS hotkey (Caps Lock by default, user-configurable) for dictation mode. Uses CGEventTap to intercept key events at the HID level. The hotkey toggle in Settings should:
1. Create a CGEventTap on a background thread
2. Monitor for the configured keycode (default: 57 = Caps Lock)
3. On press: start dictation, on second press: stop dictation

## Architecture
```
Settings toggle "On" click
  → JS: invoke('cmd_enable_dictation_hotkey', { enabled: true, keycode: 57 })
  → Rust: cmd_enable_dictation_hotkey()  [tauri/src-tauri/src/commands.rs ~line 2480]
    → stop_dictation_hotkey()            [acquires DICTATION_HOTKEY_MONITOR mutex]
    → start_dictation_hotkey_with_keycode()  [acquires same mutex again]
      → HotkeyMonitor::start(keycode)    [crates/core/src/hotkey_macos.rs ~line 97]
        → spawns thread → run_event_tap()
        → thread sends true/false via mpsc channel
        → start() waits up to 2s on rx.recv_timeout()
      → stores monitor in DICTATION_HOTKEY_MONITOR static mutex
    → checks DICTATION_HOTKEY_MONITOR.lock() to see if it started
    → if None: opens System Settings + returns Err
```

## What We've Tried
1. **First attempt**: Used `is_accessibility_trusted()` (AXIsProcessTrustedWithOptions) as upfront check. This returned false even though Minutes was enabled in Accessibility — because CGEventTap needs Input Monitoring, not Accessibility.

2. **Second attempt**: Removed upfront check, just tried CGEventTapCreate and checked result. Added mpsc channel so `HotkeyMonitor::start()` waits for the thread to report success/failure. Still freezes.

3. **Permission status**: Minutes IS added and enabled in Input Monitoring (confirmed in screenshots). But each rebuild changes the code signature, which may invalidate the TCC grant.

## Likely Root Causes (investigate these)

### 1. Tauri command blocks main thread
`cmd_enable_dictation_hotkey` is a synchronous `#[tauri::command]`. It calls `HotkeyMonitor::start()` which does `rx.recv_timeout(Duration::from_secs(2))` — this blocks the calling thread for up to 2 seconds. If Tauri runs this on the main/UI thread, the app freezes for 2 seconds (or forever if the channel never sends).

**Fix idea**: Make `cmd_enable_dictation_hotkey` async, or spawn the work on a background thread and return immediately.

### 2. Mutex deadlock
`DICTATION_HOTKEY_MONITOR` is a `std::sync::Mutex` (not reentrant). The flow acquires it in `stop_dictation_hotkey()`, releases, then acquires again in `start_dictation_hotkey_with_keycode()`. If any other code path holds this mutex (e.g., a previous failed attempt that panicked), it's permanently locked.

**Fix idea**: Use `try_lock()` instead of `lock()`, or use a `parking_lot::Mutex` which handles poisoning better.

### 3. CGEventTapCreate hangs instead of returning NULL
On some macOS versions/configurations, `CGEventTapCreate` might hang if called from a thread that doesn't have a proper run loop, or if Input Monitoring permission is in an intermediate state (enabled in UI but TCC cache stale due to rebuild).

**Fix idea**: Add a timeout around the CGEventTapCreate call itself, not just the channel receive.

### 4. CFRunLoop blocks the wrong thread
`run_event_tap()` calls `CFRunLoopGetCurrent()` and `CFRunLoopRunInMode()`. If this somehow runs on the main thread instead of the spawned thread, it blocks the Tauri UI.

**Fix idea**: Verify the thread is actually spawned and running independently.

## Key Files
- `crates/core/src/hotkey_macos.rs` — CGEventTap FFI, HotkeyMonitor, event callback
- `tauri/src-tauri/src/commands.rs` — search for "Dictation commands" and "Native hotkey"
  - `cmd_enable_dictation_hotkey` (~line 2480)
  - `start_dictation_hotkey_with_keycode` (~line 2530)
  - `DICTATION_HOTKEY_MONITOR` static mutex (~line 2510)
- `tauri/src/index.html` — search for "settings-dictation-hotkey" for the JS toggle handler

## Quick Test
1. Build: `export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1" && cargo tauri build --bundles app`
2. Install: `pkill -9 -f "minutes-app"; rm -rf /Applications/Minutes.app && cp -R target/release/bundle/macos/Minutes.app /Applications/Minutes.app && open /Applications/Minutes.app`
3. Ensure Minutes is in System Settings > Input Monitoring and enabled
4. Open Minutes > Settings > Dictation > toggle hotkey On
5. App should NOT freeze

## What Works Fine
- The entire dictation pipeline (stream → VAD → whisper → clipboard) works via CLI: `minutes dictate`
- The Tauri app works fine until you try to enable the native hotkey
- All other settings toggles work
- The CGEventTap code compiles and the FFI declarations are correct

## Environment
- macOS 26.3 (Tahoe)
- Tauri v2
- Rust edition 2021
- Apple Silicon (M-series)
- Ad-hoc signed binary (not notarized — local dev build)
