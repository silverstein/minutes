# Consent Phase 2 Wave 1 Build Report

Branch: `feat/consent-phase2-wave1`

## Built

- Added desktop Require-mode round trip for meeting recording starts. `cmd_start_recording` now returns either `started` or `consentRequired`, and the Tauri UI shows a blocking disclosure modal before reserving or spawning capture.
- Added the sensitive meeting frontmatter contract across core, reader, and SDK:
  - `capture: none`
  - `sensitivity: restricted`
  - `debrief: pending`
- Added no-capture sensitive sessions in core via `minutes_core::sensitive`, with durable session state, lock-protected start/marker/stop mutations, recording-vs-sensitive exclusivity, and no capture/streaming imports in the sensitive path.
- Added `minutes sensitive start --title ...` and `minutes sensitive stop`. TTY stop prompts for a debrief; non-TTY stop writes immediately with `debrief: pending`.
- Routed `minutes note` to sensitive-session markers while a sensitive meeting is active and emitted `sensitive.marker` events.
- Added desktop palette/tray sensitive meeting start/stop entry points, desktop status reporting for active sensitive sessions, and Recall handoff with a `/minutes-debrief` prompt.
- Updated README/config/frontmatter docs, JSON schema snapshot, SDK reader test coverage, and site generated release metadata.

## Constraint Check

- Copy discipline: scanned touched runtime/docs files for the forbidden capture-claim wording from the spec; no hits remain.
- Non-interactive callers: CLI sensitive stop checks `stdin().is_terminal()` and saves immediately when non-TTY.
- Agents/frontmatter: docs state the new fields are written by Minutes itself and assistant tools should preserve them rather than invent them.
- Sensitive path audio isolation: scanned `crates/core/src/sensitive.rs` for capture/streaming imports and audio object names; no hits.
- Public docs: new public Rust items in the core sensitive module, frontmatter enums, reader enums, and Tauri result/commands have doc comments.
- Skill mirrors: no skill source or generated skill file changed, so no mirror rebuild was needed.

## Test Results

- `cargo fmt --all`: passed.
- `cargo clippy --all --no-default-features -- -D warnings`: passed.
- `cargo check -p minutes-app --tests`: passed.
- `cargo test -p minutes-core --no-default-features`: blocked in this sandbox by environment limits.
  - First exact run failed before final snapshot acceptance and because repo tests attempted to write state under the real home directory, which is outside this sandbox's writable roots.
  - Retried with `HOME`, `USERPROFILE`, and `XDG_CONFIG_HOME` under `/tmp` while preserving `CARGO_HOME`/`RUSTUP_HOME`. Parallel run passed 768 tests, failed 5, ignored 1. The remaining failures were cross-test local state/TCP issues unrelated to this wave.
  - Serial retry with the same writable temp home passed 770 tests, failed 3, ignored 1. The remaining three failures are OpenAI-compatible summarize tests that call `TcpListener::bind("127.0.0.1:0")`; this sandbox denies localhost binding.
- Focused core coverage:
  - `cargo test -p minutes-core --no-default-features sensitive::tests`: passed, 4 tests.
  - `cargo test -p minutes-core --no-default-features frontmatter_sensitive_fields_are_optional_and_serialize_when_present`: passed, 1 test.
- `cd crates/mcp && npx vitest run`: blocked. `npx` attempted to fetch `vitest` from `registry.yarnpkg.com` and network is disabled (`ENOTFOUND`). `npm ci --offline --ignore-scripts` also failed because the npm cache is missing `zod-to-json-schema`.
- `node scripts/sync_site_release_version.mjs`: passed; updated `site/lib/release.ts`.
- `node scripts/generate_llms_txt.mjs`: passed; updated `site/public/llms.txt`.
- `git diff --check`: passed.

## Desktop Verification

- Attempted the required dev app build/install:
  - `MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Mathieu Silverstein (63TMLKT8HN)" ./scripts/install-dev-app.sh`
  - Result: blocked because that signing identity is not present in this keychain.
- Because the signed dev app could not be built, I could not click-test `~/Applications/Minutes Dev.app` in this environment.

## Issue Tracker

- `bd ready --json` was attempted, but beads could not start its local Dolt server because this sandbox denies localhost binding (`listen tcp 127.0.0.1:0: bind: operation not permitted`).

## Deviations

- Did not push, per instruction.
- Did not open a PR, per instruction.
- Did not update skill mirrors because no skill files changed.
- Required test gates that depend on network, localhost binding, or the missing signing identity could not complete in this sandbox; details are above.
- The commit hook exported a root `issues.jsonl` file during the first commit. I attempted to remove it, but staging that tracked-file removal repeatedly hit the worktree-gitdir index lock permission issue for this sandbox. The canonical beads state remains `.beads/issues.jsonl`.
