# Build Spec Addendum — Consent in Tauri Settings + fix broken --no-default-features

Author: Claude (handed to Codex). Builds on docs/plans/consent-layer-spec-2026-06-04.md
(already implemented). Repo: ~/Sites/minutes (work here).

Same hard constraints as the base spec apply, especially: **copy discipline** (no
"legal/compliant/lawful/no consent required" claims anywhere, including UI strings;
frame as "disclosure aid, not legal advice"), 100% doc comments on new pub items, fmt +
clippy clean.

## Part 1 — Surface consent in the Tauri Settings panel (config parity)

The desktop app shares `Config` with the CLI but does not yet expose the new `[consent]`
section. Add it, mirroring exactly how `privacy.hide_from_screen_share` and the
`transcription` selects are wired today.

### 1a. Backend setter — `tauri/src-tauri/src/commands.rs`, `cmd_set_setting` (~line 8207)

`cmd_set_setting` is match-gated on `(section, key)`. Add a `// Consent` block with three arms:

```rust
("consent", "mode") => {
    let mode = match value.as_str() {
        "off" => ConsentMode::Off,
        "remind" => ConsentMode::Remind,
        "require" => ConsentMode::Require,
        other => return Err(format!("unknown consent mode '{}'. Valid: off, remind, require", other)),
    };
    config.consent.mode = mode;
}
("consent", "disclosure_script") => config.consent.disclosure_script = value.clone(),
("consent", "default_basis") => {
    config.consent.default_basis = parse_optional_string_setting(&value);
}
```
(Import `ConsentMode` if not already in scope. `parse_optional_string_setting` already exists.)

### 1b. Backend getter — `cmd_get_settings` (~line 7927)

Add a `consent` object to the returned JSON, alongside the existing `privacy` block:
```json
"consent": {
  "mode": <"off"|"remind"|"require">,
  "disclosure_script": <string>,
  "default_basis": <string|null>
}
```
Serialize `ConsentMode` to its lowercase string. Match the construction style already
used in that function.

### 1c. Frontend — `tauri/src/index.html` Settings overlay

In the **privacy `settings-section`** (the one containing `#settings-screen-share`,
markup ~line 5336), add a consent control group. A `<select>` for mode + a text field
for the disclosure script, matching the existing `.settings-field` / toggle styling:

```html
<!-- Recording consent (disclosure aid; not legal advice) -->
<label class="settings-field-label" for="settings-consent-mode">Recording consent reminder</label>
<select id="settings-consent-mode" class="settings-field">
  <option value="off">Off</option>
  <option value="remind">Remind me (default)</option>
  <option value="require">Require acknowledgment</option>
</select>
<p class="settings-hint">Shows a local-transcription disclosure before recording a
meeting. A disclosure aid, not legal advice — obtain all-party consent where required.</p>
<label class="settings-field-label" for="settings-consent-disclosure">Disclosure script</label>
<input type="text" id="settings-consent-disclosure" class="settings-field" spellcheck="false">
```
(Use whatever the existing label/hint classes are in that section — match siblings, do
not invent new CSS unless a class is missing.)

**Load** (in the settings-load fn, next to `setSettingsToggle('settings-screen-share', …)` ~line 8628):
```js
const consentMode = s.consent?.mode || 'remind';
const sel = document.getElementById('settings-consent-mode');
if (sel) sel.value = consentMode;
const disc = document.getElementById('settings-consent-disclosure');
if (disc) disc.value = s.consent?.disclosure_script || '';
```

**Onchange handlers** (next to the other `cmd_set_setting` select handlers ~line 9148):
```js
document.getElementById('settings-consent-mode')?.addEventListener('change', async (e) => {
  try { await invoke('cmd_set_setting', { section: 'consent', key: 'mode', value: e.target.value }); }
  catch (err) { console.error('consent mode:', err); }
});
document.getElementById('settings-consent-disclosure')?.addEventListener('change', async (e) => {
  try { await invoke('cmd_set_setting', { section: 'consent', key: 'disclosure_script', value: e.target.value }); }
  catch (err) { console.error('consent disclosure:', err); }
});
```

### 1d. Make the setting non-misleading in the app record flow (non-blocking only)

The desktop record-start path (the `cmd_start_recording`-family commands ~line 4906 and
their frontend caller) must surface the SAME non-blocking disclosure the CLI shows in
`Remind`/`Require` mode, so the setting is not a no-op in the app:
- When a **meeting** recording starts and `config.consent.mode` is `remind` or `require`,
  emit the local-transcribe indicator + the disclosure script to the user via the app's
  EXISTING notification/toast/log surface (find how the app already surfaces transient
  record-start messages — reuse it; do NOT add a new modal/overlay).
- **Do NOT build a blocking confirmation modal.** In the app, `require` behaves like
  `remind` for now. Add a `// TODO(phase 2): blocking require confirmation modal` note.
- Only for meeting captures (not memos/quick-thought), same gating as the CLI.

If wiring 1d cleanly requires a new modal/overlay, STOP and leave a TODO instead — the
settings UI (1a–1c) is the must-have; 1d is best-effort non-blocking only.

## Part 2 — Fix the broken `cargo test -p minutes-core --no-default-features`

The auto-discovered example `crates/core/examples/dogfood_vad_engines.rs` uses
`minutes_core::live_transcript`, which is gated `#[cfg(all(feature="streaming", feature="whisper"))]`.
There are no `[[example]]` declarations in `crates/core/Cargo.toml`, so it compiles under
all feature sets and breaks `--no-default-features`.

Add to `crates/core/Cargo.toml`:
```toml
[[example]]
name = "dogfood_vad_engines"
required-features = ["streaming", "whisper"]
```
Check whether `build_parity_fixtures.rs` (the other example referencing streaming) also
needs gating; if it references gated modules, give it the matching `required-features`.
After this, `cargo test -p minutes-core --no-default-features` must skip the example(s)
and pass cleanly.

## Verification (must all pass; report results)

```
cargo fmt --all
cargo clippy --all --no-default-features -- -D warnings
cargo clippy --all -- -D warnings
cargo test -p minutes-core --no-default-features      # MUST now pass (was broken)
cargo build -p minutes-cli
cargo build -p minutes-app   # confirm the Tauri backend compiles with the new command arms
```
- Do NOT commit.
- UI render verification (dev-app build + click-test of the new settings controls) is a
  SEPARATE manual step handled outside this task — note that it is still required.
- Report the diff (files + insertions/deletions) for review.
