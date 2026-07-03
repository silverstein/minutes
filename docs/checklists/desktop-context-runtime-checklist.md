# Desktop Context Runtime Checklist

This checklist is for **real desktop dogfooding** of the meeting-adjacent
desktop-context collectors after the parity collector work merged in PR #168.

Use it when validating that desktop context behaves correctly on an actual:

- Windows desktop session
- Linux desktop session with AT-SPI accessibility enabled

This document is intentionally about **runtime truth**, not CI/build truth.
Compile coverage for the parity tranche already exists in GitHub Actions.

## What success looks like

On a real desktop session, Minutes should:

- create desktop-context events only when `[desktop_context].enabled = true`
- write those events into `~/.minutes/context.db`
- expose them through the existing CLI / MCP / dashboard retrieval surfaces
- capture only the data the product currently claims to capture
- avoid overstating support where the platform collector is partial or brittle

## Common setup

Use a test config like:

```toml
[desktop_context]
enabled = true
capture_window_titles = true
capture_browser_context = false
allowed_apps = []
denied_apps = []
allowed_domains = []
denied_domains = []
```

Then validate both of these session types:

- `minutes record`
- `minutes live`

Useful commands:

```bash
minutes context activity-summary --last 10m
minutes context search "terminal"
minutes context get-moment --last 5m
sqlite3 ~/.minutes/context.db '.tables'
sqlite3 ~/.minutes/context.db 'select source, app_name, bundle_id, window_title, observed_at from context_events order by observed_at desc limit 20;'
```

## Privacy invariants

Verify these on every platform:

1. With `[desktop_context].enabled = false`, new recordings and live sessions do **not** create context sessions or events.
2. With `capture_window_titles = false`, app-focus events may exist but window titles do not.
3. With `capture_browser_context = false`, browser windows do not generate `browser_page` rows.
4. App allow/deny filters actually change what gets stored.
5. Domain allow/deny lists are still deferred policy hooks only. Do not claim URL/domain filtering is active unless browser URL capture exists.

## Windows runtime pass

### Environment

- Windows 11 preferred
- native desktop session, not CI
- run either the CLI build or the desktop app build that matches the shipped collector path

### Happy-path scenarios

1. Start `minutes record`.
2. Switch focus across:
   - File Explorer
   - Windows Terminal / PowerShell
   - Chrome / Edge / Firefox
   - Notepad or another simple text editor
3. Stop recording.
4. Inspect `context.db` and confirm:
   - `app_focus` rows appear as focus changes happen
   - `window_focus` rows show title changes for non-browser windows
   - browser windows only produce `browser_page` when `capture_browser_context = true`

### Questions to answer

- Does `app_name` look stable and human-readable, or does it degrade to raw exe names too often?
- Do elevated apps or system windows disappear from visibility?
- Are window titles missing for common apps that should be visible?
- Does the collector behave differently when started from CLI versus desktop app?

### Minimum support statement to validate

If the pass goes well, the honest Windows support language is roughly:

> Minutes captures foreground app changes and focused window titles on Windows
> during active recording/live sessions. Browser URL capture is not part of
> this slice.

## Linux runtime pass

### Environment

- Ubuntu GNOME preferred for the first validation pass
- real desktop session, not a headless container or Codespace
- AT-SPI accessibility bus available

Codespaces can help with repo work, but they are **not** sufficient evidence for
Linux desktop-context runtime support because they do not provide a normal
interactive Linux desktop accessibility environment.

### Before testing

Check the accessibility bus:

```bash
busctl --user list | rg org.a11y
```

If that does not show the accessibility bus, do not call the Linux collector
"working" yet.

### Happy-path scenarios

1. Start `minutes record`.
2. Switch focus across:
   - GNOME Terminal or another terminal
   - Firefox / Chrome / Chromium
   - Files app
   - a text editor such as gedit or VS Code
3. Stop recording.
4. Inspect `context.db` and confirm:
   - `app_focus` rows reflect actual focus changes
   - `window_focus` rows reflect focused window title changes
   - browsers only emit `browser_page` when browser context capture is enabled

### Linux-specific questions

- Does the AT-SPI collector work across the common apps we care about, or only some of them?
- Does focus detection break under Wayland, XWayland, or specific app toolkits?
- Are app identifiers readable enough to make `allowed_apps` / `denied_apps` practical?
- Is there a material difference between GNOME apps, Electron apps, Firefox, and Chromium-based browsers?

### Minimum support statement to validate

If this pass is successful, the honest Linux support language is closer to:

> Minutes has an AT-SPI-first Linux desktop-context collector. Support depends
> on the desktop accessibility stack and the application exposing usable
> accessibility metadata.

That is intentionally narrower than "Linux fully supported everywhere."

## Support-envelope notes to capture

For each platform, record:

- tested OS version
- tested desktop environment or shell
- tested browsers
- tested terminal/editor apps
- whether titles were present, missing, or inconsistent
- any app classes that should be excluded from current public claims

If runtime behavior is materially narrower than the current docs or UI imply,
update:

- `docs/architecture/config.md`
- `docs/development/desktop-development.md`
- any desktop-context settings copy in the app UI

## Exit criteria for this checklist

This runtime follow-up is complete when:

- Windows has a real desktop validation pass with notes
- Linux has a real desktop validation pass with notes
- the practical support envelope is written down
- any overstated docs/UI claims discovered during dogfooding are corrected
