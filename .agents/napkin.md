# Napkin

## Corrections
| Date | Source | What Went Wrong | What To Do Instead |
|------|--------|----------------|-------------------|
| 2026-03-18 | self | Updated `start_recording` signature for processing-state wiring but missed the tray menu call site in `tauri/src-tauri/src/main.rs` | After widening Tauri command/helper signatures, run `rg` for all call sites before testing so the state plumbing stays consistent |
| 2026-03-19 | self | The Tauri live-recording path was still injecting timestamp titles, which quietly bypassed the smart-title pipeline we had already shipped | When adding UX polish around recording labels, verify we are not overriding downstream title generation or artifact heuristics by accident |
| 2026-03-19 | self | Tried a direct `cargo run -p minutes-cli` sanity check without the repo's usual macOS `CXXFLAGS`, which failed in `whisper-rs-sys` even though the targeted tests had already passed | On this machine, use the `CXXFLAGS=\"-I$(xcrun --show-sdk-path)/usr/include/c++/v1\"` prefix for any Rust command that may build `whisper-rs`, not just tests |
| 2026-03-19 | self | Used backticks inside a shell `rg` argument during verification, and `zsh` treated them as command substitution | When grepping for literal backtick-delimited strings in shell commands, wrap the whole pattern safely or avoid backticks in the query altogether |
| 2026-03-19 | self | Assumed parsed `Frontmatter` carried the runtime-style `content_type` field and wired a consistency heuristic to a field that does not exist | When adding report features on top of markdown frontmatter, re-open the actual `Frontmatter` struct and map from `r#type` explicitly instead of assuming it mirrors downstream write results |
| 2026-03-19 | self | Probed `qmd collection add` assuming the old plan syntax and accidentally created a real collection while trying to discover the interface | For external CLIs, inspect the shipped help or source before probing mutating subcommands; for QMD specifically, `collection add` takes `<path> --name <name>` and `collection list` does not include paths, so pair it with `collection show` |
| 2026-03-19 | self | Guessed the Tauri crate package name for `cargo check` instead of reading `tauri/src-tauri/Cargo.toml` first | When verifying a workspace member, read the manifest or use `--manifest-path` before assuming the package name |
| 2026-03-23 | self | Reused browser `keyCode` values for the dictation hotkey picker even though the native macOS monitor expects virtual keycodes, so many custom keys could never trigger | For browser-captured shortcuts that feed native macOS APIs, map from `KeyboardEvent.code` to macOS virtual keycodes explicitly instead of persisting DOM keycodes |
| 2026-03-23 | self | Moving the dictation hotkey startup off the UI thread still left a race where stale monitor callbacks could overwrite the newest runtime state | When a background monitor can be restarted quickly, track a generation/token in shared state and ignore lifecycle updates from older workers |
| 2026-03-23 | self | Reached for `apply_patch` and `python` out of habit while fixing `crates/core/src/pid.rs`; `apply_patch` kept timing out on this file and this shell only has `python3` | If a targeted edit tool is flaky here, switch promptly to `python3` and verify the rewritten file immediately before continuing |
| 2026-03-24 | self | Moved `tauri::AppHandle` into the dictation hotkey closure and then tried to reuse it for later status emission, which broke `cargo test` with a borrow-of-moved-value compile error | When a Tauri callback needs the same app handle in multiple async/closure paths, clone named handles up front (`app_for_events`, `app_for_status`, etc.) before wiring the monitor |
| 2026-03-24 | self | Trusted `Contents/MacOS/minutes-app --diagnose-hotkey` as a faithful TCC check, but the same probe only reflected granted permissions when the app was launched through LaunchServices | For macOS permission debugging, test the installed app through `open -a` or the repo helper instead of directly executing the bundle binary from the shell |
| 2026-03-24 | self | Read a Windows cross-target `can't find crate for core` failure as if it were a repo portability bug before checking which local Rust toolchain actually owned the installed target | When testing cross-platform builds on this machine, verify `which cargo`, `which rustc`, and `rustup target list --toolchain ... --installed` before trusting the failure as a code issue |
| 2026-03-24 | self | Reached for stale mental-model paths like `tauri/src/app.js` and `crates/mcp/src/reader.ts` before checking the live tree, even though the repo has moved those surfaces to `tauri/src/index.html` and `crates/sdk/src/reader.ts` | When reviewing fast-moving product repos, verify the current file layout with `rg --files` before quoting architecture docs or assuming entry points |
| 2026-03-28 | self | Took "made updates" as likely code changes before checking the worktree; the actual changes were new untracked planning docs | For review requests, start with `git status --short` and inspect untracked docs too, not just tracked diffs |
| 2026-03-28 | self | Almost trusted the shell to demonstrate literal `${HOME}` path handling, but zsh expanded it before Node saw the string | When validating env-var expansion bugs in JS/TS code, use a single-quoted heredoc or equivalent so the runtime receives the literal `${HOME}` / `$HOME` string |
| 2026-03-28 | self | v0.8.2 and v0.7.2 GitHub releases shipped with empty bodies because CI (softprops/action-gh-release) created the release before `gh release create` ran with notes | Create the release with notes FIRST via `gh release create --target main`, which also creates the tag. CI then uploads assets to the existing release. Never `git tag` + `git push --tags` before the release exists. |
| 2026-03-29 | self | Passed two positional test filters to `cargo test`, which Rust interprets as an invalid command rather than "run both sets" | For Rust verification, use one filter per `cargo test` invocation or run the broader target once instead of stacking multiple test names |
| 2026-03-29 | self | Tried to run new dictation unit tests without enabling the `streaming` feature, so the `dictation` module wasn't present in the test binary and the tests silently appeared missing | When verifying dictation/live-transcript behavior in `minutes-core`, use `cargo test -p minutes-core --features streaming ...` or inspect `cargo test -- --list` first if tests don't appear |

## User Preferences
- For coding/debugging/testing/review tasks, prioritize technical implementation detail and concrete verification.
- For repo reviews, findings should be the primary output, ordered by severity with file/line references.

## Patterns That Work
- Start by checking repo instructions plus `bd` workflow, then inspect both the Rust crates and the MCP/Tauri surfaces before making claims about app behavior.
- On macOS 26+, Rust tests that compile `whisper-rs` need `CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"`; core tests pass once that is set.
- For native macOS hotkeys in Tauri, keep startup non-blocking and report lifecycle changes back to the webview with explicit `starting/active/failed` status events.
- For releases: create the GitHub release with notes BEFORE pushing the tag (use `gh release create` which creates both). CI workflows only upload assets to an existing release. Never let CI create the release — it produces empty bodies that show up blank in followers' feeds.

## Patterns That Don't Work
- Assuming this repo is only a CLI tool misses the Tauri desktop app and MCP integration surfaces that need review too.
- Trusting `path.resolve(...).startsWith(...)` in Node is not a safe allowlist check here; it misses sibling-prefix and symlink cases.
- In `crates/core/src/pid.rs`, reopening a PID file after taking an `fs2` exclusive lock is not portable; Windows can fail with `os error 33` even though the same flow appears fine on Unix.

## Domain Notes
- `minutes` is a local-first meeting capture app with Rust core/CLI, a Tauri desktop app, and a TypeScript MCP server.
- The worktree may already contain user changes; review around them carefully and do not revert unrelated edits.
- The desktop app mixes in-memory recording state with PID-file-based status, so app restarts and cross-surface recording flows are easy places for desync bugs.
- Live coaching is intentionally split across surfaces: the Tauri UI is the on/off control plus lightweight live status, while the actual coaching/advice is meant to happen in Claude Desktop/Code or another agent reading the live transcript context.
