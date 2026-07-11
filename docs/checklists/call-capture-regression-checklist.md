# Call Capture Regression Checklist

Use this checklist whenever Minutes desktop call detection / native macOS call
capture changes.

## Preconditions

- Test the installed dev app only: `~/Applications/Minutes Dev.app`
- Do not test raw `target/` bundles or the repo symlink for TCC-sensitive flows
- Prefer Chrome + Google Meet for the browser-call path
- Keep `Google Meet detection (experimental)` enabled during the Meet checks

## Detection

- Join a real Google Meet in Chrome.
- Verify the desktop app eventually shows an in-app banner for `Google Meet`.
- Verify the app does not steal focus when the call is detected.
- Verify the generic footer CTA changes to `Start Call Recording`.
- Verify the detector does not mislabel the session as Slack when Meet is the
  active browser call.

## Recording UI

- Start a call recording from the generic footer CTA, not only the banner CTA.
- Verify the recording bar shows `Mic live` / `Call audio live` once capture is
  actually receiving buffers.
- Verify the waveform visibly animates during native call capture.
- Verify the app can be stopped cleanly from the desktop UI.

## Artifacts

- Confirm a fresh capture appears under `~/.minutes/native-captures/`.
- Confirm all three artifacts exist for the session:
  - `...-call.mov`
  - `...-call.voice.wav`
  - `...-call.system.wav`
- Confirm the `.mov` is a valid multi-second file, not a ~2 KB stub.
- Confirm both stem WAVs are multi-second files, not header-only or sub-second.

## Transcript

- Open the saved meeting artifact.
- Verify the transcript is non-empty.
- Verify the transcript content matches the recorded speech.
- In a solo test, verify the transcript does not fabricate a second human
  speaker from system-only noise or startup beeps.
- In a real two-sided call, verify both sides appear in the transcript.

## Speaker Attribution

- Solo test:
  - Expect one effective human speaker, even if system noise/beeps are present.
- Two-sided call:
  - Expect source-aware speaker labeling to separate local voice vs remote call
    audio where the stems diverge.

## Logs

- Inspect `~/.minutes/logs/minutes.log` for:
  - `call_detect` `detected` events
  - `mic_gate_active` / `mic_gate_inactive`
  - browser probe errors or permission warnings
  - `discovered per-source audio stems`
  - `stem-based diarization complete`
  - `stem energies strongly correlated — collapsing to one speaker` in solo /
    bleed-heavy tests

## Permission / TCC

- If Google Meet detection is enabled but no banner appears:
  - Check System Settings → Privacy & Security → Automation
  - Confirm `Minutes Dev` has permission to control `Google Chrome`
- If hotkeys or other desktop affordances regress:
  - Re-run `./scripts/install-dev-app.sh`
  - Re-test the installed dev app identity only

## Follow-up Questions

- Is Google Meet detection fast enough compared to Granola?
- Should the native call waveform use true per-source levels instead of the
  current liveness-based pulse fallback?
- Do tray and command-palette start surfaces need parity with the desktop call
  session UX?
