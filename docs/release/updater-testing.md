# Updater Dev Testing

Use this note when you need to validate the Tauri updater flow against a real
release without touching the production install in `/Applications/Minutes.app`.

## Safe local target

Always use the signed development identity:

```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Mathieu Silverstein (63TMLKT8HN)"
./scripts/install-dev-app.sh --no-open
open -a "$HOME/Applications/Minutes Dev.app"
```

Verify the rebuilt Dev app still includes Parakeet symbols:

```bash
strings "$HOME/Applications/Minutes Dev.app/Contents/MacOS/minutes-app" | grep -c "transcribe_parakeet\\|parakeet_helper"
```

The count should be non-zero.

## Real updater test procedure

1. Build a local app with an older version string than the live release.
2. Install that build as `~/Applications/Minutes Dev.app`.
3. Launch the Dev app and confirm it points at the normal updater endpoint.
4. Let the app detect the newer live release.
5. Walk through:
   - update available preamble
   - phase transitions
   - download progress
   - signature verification
   - install and restart
6. Confirm the restarted app reports the new version and clears the pending
   update banner.

## Fast UI simulation

For UI-only work, use the debug command from DevTools console:

```js
window.__minutesDebugSimulateUpdate("checking")
window.__minutesDebugSimulateUpdate("downloading")
window.__minutesDebugSimulateUpdate("verifying")
window.__minutesDebugSimulateUpdate("installing")
window.__minutesDebugSimulateUpdate("error")
```

These emit the same typed updater events the real flow uses, so you can verify
the banner and capture screenshots without performing a real update.
