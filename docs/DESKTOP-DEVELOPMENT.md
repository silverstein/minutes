# macOS Desktop Development and App Identity

This guide is specifically for the macOS desktop app identity and TCC-sensitive
development workflow.

If you are looking for Windows desktop packaging or release behavior, use
[docs/RELEASE-WINDOWS.md](/Users/silverbook/Sites/minutes/docs/RELEASE-WINDOWS.md)
instead.

This project has two distinct desktop app identities on macOS:

- Production app:
  - Name: `Minutes.app`
  - Bundle id: `com.useminutes.desktop`
  - Canonical install path: `/Applications/Minutes.app`
- Development app:
  - Name: `Minutes Dev.app`
  - Bundle id: `com.useminutes.desktop.dev`
  - Canonical install path: `~/Applications/Minutes Dev.app`

The split is intentional. macOS TCC permissions such as Microphone, Screen
Recording, Accessibility, Apple Events, and Input Monitoring attach to the
app identity macOS sees, not just to "the code in this repo."

## Why this matters

Testing TCC-sensitive features from multiple app paths or signatures leads to
confusing macOS state:

- permissions appear enabled in System Settings, but the active build still
  gets prompted
- Input Monitoring looks granted for one bundle, but `CGEventTap` still fails
- Screen Recording prompts recur because the process identity changed after a
  rebuild or re-sign

The main causes are:

- launching the raw binary in `target/`
- launching ad-hoc signed bundles
- launching the repo symlink `./Minutes.app`
- mixing `/Applications/Minutes.app` with fresh local build outputs

## Canonical dev workflow

For any desktop work that touches TCC-sensitive features, use exactly one app:

```bash
./scripts/install-dev-app.sh
```

That script:

- builds the desktop bundle with the dev overlay config
- signs it with a configured local identity when available
- otherwise falls back to ad-hoc signing so contributors can still run it
- installs it to `~/Applications/Minutes Dev.app`
- runs the native hotkey diagnostic from the installed app identity
- launches `Minutes Dev.app`

## Dictation shortcut paths

Minutes now has two distinct dictation shortcut paths:

- `Standard shortcut (recommended)`
  - uses the normal global shortcut system
  - default choice: `Cmd/Ctrl + Shift + D`
  - should be the primary path we validate and ship
- `Raw key hotkey (advanced)`
  - uses low-level macOS keyboard monitoring
  - intended for keys like `Caps Lock` and `fn`
  - requires the more fragile permission-heavy path and remains advanced/experimental

When validating dictation for normal users, prefer the standard shortcut path first.

### Signing modes

For open-source contributors, the script supports two modes:

- configured identity:
  - set `MINUTES_DEV_SIGNING_IDENTITY` (preferred) or `APPLE_SIGNING_IDENTITY`
  - best for stable TCC-sensitive testing across rebuilds
- ad-hoc:
  - no signing identity configured
  - good enough to run the app and work on most features
  - less reliable for Input Monitoring / Screen Recording / repeated TCC prompts

Example with an explicit local signing identity:

```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
./scripts/install-dev-app.sh
```

If you do not have a Developer ID certificate, any consistent local codesigning
identity in your keychain is better than ad-hoc for TCC-sensitive work.

When testing desktop permissions, do not launch:

- `./Minutes.app`
- `target/release/minutes-app`
- `target/release/bundle/macos/Minutes.app`
- older copies of `Minutes Dev.app` from other locations

## Native hotkey diagnostic

The desktop binary has a built-in diagnostic mode that checks whether the
current app identity can start the macOS `CGEventTap` monitor used by the
dictation hotkey:

```bash
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app"
```

Optional keycode override:

```bash
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app" 63
```

Interpretation:

- exit `0`: the native hotkey monitor started successfully
- exit `2`: macOS identity / Input Monitoring still blocked the hotkey

This diagnostic is the fastest way to answer "can this exact app identity
create the native hotkey?" without going through the UI.

Important:

- the helper launches the app via LaunchServices using `open -a`
- do not invoke `Contents/MacOS/minutes-app --diagnose-hotkey` directly from
  the shell for TCC debugging
- direct shell execution can produce a false negative even when the same app
  succeeds when launched normally as an app

## Permission map

- Microphone:
  - needed for recording and dictation audio capture
- Screen Recording:
  - needed for screen-context screenshots and some visual desktop testing
  - not required for the dictation hotkey itself
- Input Monitoring:
  - needed for the dictation hotkey `CGEventTap` path
- Accessibility:
  - useful for GUI automation, but not the actual hotkey permission

## Repeated permission prompts

If macOS keeps prompting even though the toggle already looks enabled:

1. Quit all `Minutes` and `Minutes Dev` copies.
2. Reinstall the dev app with `./scripts/install-dev-app.sh`.
3. Launch only `~/Applications/Minutes Dev.app`.
4. Re-run `--diagnose-hotkey` from that installed app.
5. Re-check the specific permission pane for `Minutes Dev`.

If you still see repeated prompts, assume macOS is treating the current build
as a different identity until proven otherwise.

For contributors using ad-hoc signing, repeated prompts are more likely. That
is expected until you switch to a stable local signing identity.

## Guidance for AI agents

When working in this repo:

- treat `~/Applications/Minutes Dev.app` as the canonical desktop dev target
- do not claim a TCC-sensitive feature is fixed based on a raw `target/`
  binary or repo-local bundle
- prefer the built-in `--diagnose-hotkey` probe before speculating about
  Input Monitoring state
- distinguish Screen Recording issues from Input Monitoring issues explicitly

## Desktop Context Build Rules

For meeting-adjacent desktop-context work, keep the platform and packaging
contract explicit:

- macOS-first implementation is acceptable; do not fake cross-platform parity
- if you add a macOS-only helper or resource, compile or stage it from
  `tauri/src-tauri/build.rs`
- declare macOS-only bundled resources in
  `tauri/src-tauri/tauri.macos.conf.json`, not the shared
  `tauri/src-tauri/tauri.conf.json`
- if the capability is feature-gated, keep the CLI and desktop app aligned on
  `MINUTES_BUILD_FEATURES`
- keep desktop context in `~/.minutes/context.db`; do not move meetings/memos
  out of markdown or overload `graph.db` with raw desktop events

That combination is what keeps a useful macOS-only slice from accidentally
breaking Windows builds or local build scripts.
