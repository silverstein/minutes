# FIX SPEC — consent layer (adversarial-review fixes)

Two independent reviews (Codex + a fresh reviewer) both returned FIX-FIRST and converged
on these. Implement in ~/Sites/minutes. Do NOT commit. Same constraints as the prior
specs (no legal-conclusion copy; never block non-interactive callers; back-compat).

## Fix 1 — fabricated consent_notice + Remind skips reminder (flagged by BOTH; high confidence)

File: crates/cli/src/main.rs, `prepare_recording_consent` (~line 192).

Problem: `notice` falls back to `config.consent.disclosure_script`, and the fn early-
`return`s for `--consent` (~206) and `default_basis` (~215) BEFORE `match config.consent.mode`
(~223). Effects: (a) `minutes record --consent na` (or any explicit/default basis with no
`--consent-notice`) stamps `consent_notice = "Heads up — I'm using Minutes…"`, a disclosure
the user never gave — a false attestation; (b) Remind mode prints no reminder when a basis
is present.

Fix — remove the early-return-on-basis; resolve, then switch on mode:
1. `resolved_basis`: parse `--consent` if Some, else parse `config.consent.default_basis`
   if set, else None. Parse error → return Err (unchanged).
2. `explicit_notice`: `--consent-notice` trimmed; Some if non-empty else None.
   **`consent_notice` must ONLY ever be `explicit_notice`. NEVER fall back to
   `disclosure_script` in any mode** — recording the script as the notice-given fabricates
   an attestation. (Spec: "the exact disclosure the user gave/used, if any".)
3. `match config.consent.mode`:
   - Off → basis = resolved_basis.unwrap_or(Unattested); notice = explicit_notice; no reminder; no warning.
   - Remind → reminder = Some(disclosure_script) ALWAYS (even if a basis was supplied);
     basis = resolved_basis.unwrap_or(Unattested); notice = explicit_notice.
   - Require & !stdin_is_tty → basis = resolved_basis.unwrap_or(Unattested);
     reminder = Some(disclosure_script); warning = "consent gate skipped: non-interactive
     session; recording as unattested"; NEVER block.
   - Require & stdin_is_tty → if resolved_basis is Some, use it (no prompt); else prompt;
     "yes" → VerbalAllParties; "no" → bail with the existing helpful message.
     notice = explicit_notice.
Tests: extend the existing consent unit tests to assert (a) `--consent na` with no
`--consent-notice` → `consent_notice == None` (NOT the script); (b) Remind + explicit basis
still produces a reminder.

## Fix 2 — stale consent sidecar leaks to the wrong artifact (treat as MAJOR; cheap)

Files: crates/core/src/pipeline.rs (~2096, `process_with_progress_and_sidecar`);
tauri/src-tauri/src/commands.rs (~3628 `maybe_save_and_show_recording_consent`, ~5246 finalize).

Problem: `process_with_progress_and_sidecar` unconditionally calls `notes::load_consent()`
(global ~/.minutes/current-consent.json). That fn also backs manual `minutes process <file>`
(main.rs:4254). A stale sidecar (crashed/aborted recording, or a concurrent live session)
gets stamped onto an unrelated processed file — a wrong consent attestation (worse than absent).

Fix — establish the invariant: **consent attaches ONLY to the recording it was captured
for; any uncertainty → absent/Unattested, never another meeting's consent.**
- The generic pipeline must NOT read the global consent sidecar. Consent must arrive only
  via the recording's own context/job. The queue path already carries it by value
  (pipeline.rs:1592 `context.consent`). Prefer the SMALLEST change: have
  `process_with_progress_and_sidecar` take consent from the passed context/params and
  delete the global `notes::load_consent()` at ~2096. `minutes process` passes None.
- Clear the consent sidecar at record START (before writing the new one) so a crashed
  prior session can't leave stale data, AND on finalize/cleanup (mirror how the live path
  already removes the notes/context sidecars).
- Desktop (Fix 2b): in `maybe_save_and_show_recording_consent`, if `save_consent` fails,
  do NOT fall through to a stale on-disk read at finalize — clear the sidecar (finalize
  then reads None = Unattested, safe) or carry consent in memory into the queued job.

## Not fixing (leave as-is)
- notes.md — pre-existing unrelated dirt; will be excluded from the commit.
- CLI hard-errors vs Tauri silently-Unattested on an invalid hand-edited `default_basis` —
  acceptable; Unattested is the safe default.

## Verify (all must pass; report results + diff)
```
cargo fmt --all
cargo clippy --all --no-default-features -- -D warnings
cargo clippy --all -- -D warnings
cargo test -p minutes-core --no-default-features
cargo test -p minutes-cli --no-default-features          # consent tests
cargo build -p minutes-cli
cargo build -p minutes-app
```
Do NOT commit. Report the diff for a final review.
