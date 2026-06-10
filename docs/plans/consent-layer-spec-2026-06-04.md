# Build Spec — Consent Layer (Phase 1 of privacy/sensitive-meeting work)

Author: Claude (handed to Codex for implementation)
Date: 2026-06-04
Repo: ~/Sites/minutes  (cd here first — do NOT work in ~/.minutes/assistant)

## Why

Minutes records full local transcripts. That is the hero feature and stays the hero.
But in all-party-consent jurisdictions (e.g. California PC §632), capturing others'
confidential speech needs consent. Every shipping competitor (Granola, Otter, Gong,
Circleback) handles this with a lightweight consent affordance — it is table stakes,
not a product-killer. This phase adds that affordance to Minutes and records the
consent basis in the meeting artifact.

The compliance *story* leads with data-residency (audio stays on device), NOT with
any claim that local transcription removes the consent requirement. It does not.

## Scope (do ONLY this)

1. A `[consent]` config section.
2. A visible recording/transcribing indicator at `minutes record` start.
3. A consent acknowledgment gate (off / remind / require) that is **non-interactive-safe**.
4. A reusable disclosure script (config-driven, surfaced to the user).
5. Two new frontmatter fields: `consent` and `consent_notice`, written through the pipeline.
6. Tests + docs.

## Explicitly OUT of scope (Phase 2 — do NOT build now)

- The no-capture "Sensitive Meeting" mode (prep panel, quick markers, guided debrief).
- Any Tauri/desktop UI work beyond what compiles.
- Telemetry, Stripe, Discord, hosted anything.
- Video/screen capture.

## Hard constraints (read before coding)

- **Never block non-interactive callers.** `minutes record` is invoked by hooks, the
  desktop app, and automation. The `require` gate may only block when stdin is a TTY.
  When stdin is NOT a TTY, downgrade to `remind` behavior, print a warning, and record
  the basis as `unattested`. Never hang a headless run.
- **Copy discipline — non-negotiable.** No string in code, help text, or docs may say
  "no consent required", "legal", "compliant/compliance", or assert that local
  transcription exempts the user from consent law. Allowed framing only:
  "audio stays on your device", "Minutes transcribes locally",
  "ensure everyone present consents where required". Do NOT render legal conclusions.
- **Back-compat.** Existing meeting files have no `consent` field. New frontmatter
  fields must be `Option`, `#[serde(default, skip_serializing_if = "Option::is_none")]`,
  so old files still deserialize and clean runs emit no extra noise.
- Cross-platform (macOS/Linux/Windows). Gate TTY detection behind `std::io::IsTerminal`.
- 100% doc comments on new `pub` items (repo rule).
- Follow the existing `context` plumbing exactly (see §"Data flow").

## Data model

### Config — `crates/core/src/config.rs`

Add, mirroring `PrivacyConfig` (line ~306) and its `Default` impl (line ~355):

```rust
/// Consent affordance for meeting capture. Minutes records full local
/// transcripts; in all-party-consent jurisdictions capturing others' speech
/// requires consent. This config controls the pre-record reminder/gate and the
/// disclosure script. It is a privacy aid, NOT legal advice and NOT a
/// determination of legality.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsentConfig {
    /// off | remind | require. Default `remind`.
    pub mode: ConsentMode,
    /// One-line script the user can read aloud / paste to disclose recording.
    pub disclosure_script: String,
    /// Optional default basis stamped into frontmatter when the user does not
    /// pass one (e.g. a team with notice baked into every calendar invite).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_basis: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentMode { Off, Remind, Require }
```

- `impl Default for ConsentConfig`: mode = `Remind`, default_basis = `None`,
  disclosure_script = (honest, no legal claim):
  `"Heads up — I'm using Minutes to transcribe this conversation locally on my device for my own notes. Let me know if you'd prefer I didn't."`
- `impl Default for ConsentMode { fn default() -> Self { Self::Remind } }`
- Add `pub consent: ConsentConfig,` to `Config` (struct at line ~18; it is `#[serde(default)]`
  so no other change needed).

### Frontmatter — `crates/core/src/markdown.rs`

Add to `Frontmatter` (struct at line 145), after `recorded_by` (line 187):

```rust
    /// How consent to capture was obtained, if attested. Privacy metadata only —
    /// not a legal determination. See [`crate::config::ConsentConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ConsentBasis>,
    /// The exact disclosure the user gave/used, if any. Free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_notice: Option<String>,
```

New enum in markdown.rs:

```rust
/// Attested basis for capturing a conversation. Privacy metadata, NOT a legal
/// determination of whether recording was lawful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentBasis {
    VerbalAllParties,   // verbal_all_parties
    NoticeInInvite,     // notice_in_invite
    RecordedDisclosed,  // recorded_disclosed
    NotApplicable,      // na  -> use #[serde(rename = "na")]
    Unattested,         // unattested
}
```
(Use `#[serde(rename = "na")]` on `NotApplicable`.)

`Frontmatter` has NO `Default` impl and is built via explicit struct literals. You MUST
add `consent: None, consent_notice: None` to every construction site, or it won't
compile. Known sites (grep `Frontmatter {` to confirm you got all):
- `crates/core/src/pid.rs` ~832 and ~884
- `crates/core/src/daily_notes.rs` ~299
- `crates/core/src/dictation.rs` ~1125
- `crates/core/src/pipeline.rs` (the main meeting build site)
- any test fixtures

## Data flow (mirror `context` exactly)

`minutes record` persists pre-meeting context at start via
`minutes_core::notes::save_context`, and the pipeline reads it back when building
`Frontmatter.context` at finalize (a separate `minutes stop` process does the
processing, so consent MUST be persisted to disk at record start, not held in memory).

Do the same for consent:
1. Add `notes::save_consent(basis: Option<ConsentBasis>, notice: Option<&str>)` that
   writes a small sidecar in the recording session dir (mirror `save_context`).
2. Add `notes::load_consent() -> (Option<ConsentBasis>, Option<String>)`.
3. In the pipeline, where `context` is loaded into the Frontmatter, also load consent
   and set `consent` / `consent_notice`. Clear the sidecar on finalize like context.

## CLI — `crates/cli/src/main.rs`

### New args on `Commands::Record` (struct at line 195)

```rust
/// Attested consent basis for this capture: verbal_all_parties | notice_in_invite |
/// recorded_disclosed | na. Stamped into the meeting's frontmatter.
#[arg(long, value_name = "BASIS")]
consent: Option<String>,
/// Free-text note describing the disclosure you gave.
#[arg(long, value_name = "TEXT")]
consent_notice: Option<String>,
```
Thread them into `cmd_record(...)` (signature at line 1957; call site in
`Commands::Record` handler ~1266).

### Behavior in `cmd_record` (inject around line 2035, before the "Recording meeting..." prints)

Only for capture that records other people — i.e. `CaptureMode::Meeting` (and call/room
intents). Skip the indicator/gate for `QuickThought` and memo/solo dictation.

1. **Indicator (always, for Meeting):** print to stderr:
   `🔴 Recording + transcribing locally — audio stays on your device.`
2. **Resolve basis:** `--consent` flag → else `config.consent.default_basis` → else None.
   Parse the string to `ConsentBasis`; reject unknown values with a clear error.
3. **Gate by `config.consent.mode`:**
   - `Off`: no extra output. basis = resolved (may be None → store `Unattested`).
   - `Remind` (default): print `eprintln!` with the reminder + the configured
     `disclosure_script`. Do not block. basis = resolved or `Unattested`.
   - `Require`:
     - if a basis was provided via flag/default → proceed with it.
     - else if `std::io::stdin().is_terminal()` → interactively prompt:
       `Has everyone present been notified and do they consent? [y/N] `
       (read line; `y`/`yes` → basis = `VerbalAllParties`; anything else → abort with a
       non-judgmental message telling them how to proceed: pass `--consent` or set
       `[consent] mode = "remind"`).
     - else (NOT a TTY) → print a warning ("consent gate skipped: non-interactive
       session; recording as unattested"), basis = `Unattested`, DO NOT block.
4. Call `notes::save_consent(basis, consent_notice.as_deref())`.

Use `use std::io::IsTerminal;`.

## Tests

`crates/core` (`--no-default-features` must pass):
- `ConsentConfig::default()` → mode == Remind, disclosure_script non-empty,
  default_basis None.
- `ConsentMode` and `ConsentBasis` serde round-trip to the exact snake_case strings
  (`verbal_all_parties`, `notice_in_invite`, `recorded_disclosed`, `na`, `unattested`,
  and mode `off`/`remind`/`require`).
- Frontmatter: serialize with `consent: Some(VerbalAllParties)` → YAML contains
  `consent: verbal_all_parties`; with `None` → key absent.
- Frontmatter: deserialize a legacy YAML block with NO consent key → `consent: None`
  (back-compat).
- `notes::save_consent` then `load_consent` round-trips.

CLI (`crates/cli`):
- `--consent verbal_all_parties` ends up in the written frontmatter (can assert via the
  pipeline/notes seam used by existing record tests).
- `require` mode + non-TTY path does not block and yields `Unattested` + a warning.
- unknown `--consent xyz` → clean error, non-zero exit.

## Docs

- README: short `[consent]` subsection + the two frontmatter fields. Lead with
  "audio stays local"; include the explicit disclaimer: "This is a disclosure aid, not
  legal advice; obtain all-party consent where the law requires it."
- Document the `[consent]` TOML in whatever config reference the repo keeps
  (search docs/ for the existing `[transcription]`/`[retention]` reference and match it).

## Verification before you call it done (CLAUDE.md PRE-COMMIT)

```
command -v cargo && rustup which cargo   # must match (toolchain pin)
cargo fmt --all
cargo clippy --all --no-default-features -- -D warnings
cargo clippy --all -- -D warnings
cargo test -p minutes-core --no-default-features
cargo build -p minutes-cli
```
- Confirm no feature-stub parity breakage and no Unix-only API used unguarded.
- No Tauri/dev-app build needed (no UI surface touched). If you find yourself editing
  `tauri/src/index.html` or adding a `cmd_*`, STOP — that's out of scope for this phase.

## Done = 
Config section + indicator + non-blocking gate + disclosure script + `consent`/
`consent_notice` frontmatter written through the pipeline, all tests green, docs updated,
zero legal-conclusion copy. Report the diff back for review before committing.
