# OpenAI Build Week: Codex Sidekick

Codex Sidekick turns a live Minutes recording into an interactive meeting strategist. It follows the bounded live transcript and exact-session screen context, reasons across details that were never stated together, and gives concise, decision-oriented coaching while the conversation is still happening.

## Eligibility and project boundary

Minutes is an existing open-source conversation-memory project. It was **not** created during OpenAI Build Week. The qualifying Build Week work is a meaningful new extension built on July 21, 2026: the Sidekick product surface, its live evidence path, deterministic evaluation harness, and a persistent Codex app-server reasoning backend behind a provider-neutral runtime.

The last commit before this work is [`146d062f`](https://github.com/silverstein/minutes/commit/146d062f6ff9ab232c789712890ec734360493fe), committed July 21, 2026 at 11:27:30 PDT. The Build Week extension begins at [`115739b6`](https://github.com/silverstein/minutes/commit/115739b6) at 13:25:52 PDT. The complete timestamped sequence is below.

### What existed before

- Local meeting recording, transcription, diarization, and screen capture.
- Recall and terminal-agent workflows over saved Minutes artifacts.
- Local markdown meeting memory and CLI/MCP access to it.
- An earlier Coach experiment, which established the problem but did not deliver the grounded, persistent Sidekick experience.

### What was built during Build Week

- A first-class **Sidekick** control in the active Recording and Live surfaces, plus a dedicated persistent native Sidekick window.
- Per-turn grounding over a bounded transcript window and the latest image from the **same capture session**. Visual claims are rejected unless that exact image was retrieved for the turn.
- A live-session reliability fix for sandboxed process checks: `kill(pid, 0)` returning `EPERM` now correctly means “alive”; only `ESRCH` means “dead.”
- Read-only and WAL-safe access to live context from the Sidekick sandbox, binding to the bundled Minutes CLI, and clean recovery/replacement of stale Recall or Sidekick sessions.
- The Meridian golden evaluation: a deterministic multi-speaker negotiation fixture whose required insight combines separate facts into an unstated `$800K/month` consequence, followed by a procurement-lead role reversal. The scorer fails summaries, wrong arithmetic, unsupported evidence, and self-sabotaging advice.
- A provider-neutral persistent reasoning runtime. Minutes owns the session reducer, bounded evidence window, corrections, intervention policy, memory, and publish decision. A backend only performs a steerable, streaming reasoning turn.
- A hardened **Codex app-server adapter** as the first backend: persistent thread, streamed deltas, foreground steering, interruption, Codex Fast, no model-callable tools/MCP lanes, and an isolated workspace. The interface can also be implemented by Claude-via-MCP or a local Ollama/Apple Foundation Models backend without moving vendor logic into core.
- A fully automated native UI/provider acceptance gate that drives the real Recording, Sidekick, and cloud-consent controls; verifies room-mic signal and exact-session screen capture; stages two transcript turns; scores strategic quality; rejects adversarial false-green mutations; and proves bounded teardown. Minutes now owns the exact PNG bytes sent to a provider, so Codex receives a validated inline image instead of reopening a mutable file path.

## How Codex collaborated in building it

Codex was both the implementation collaborator and the first reasoning backend:

1. It compared the weak Coach iteration with the existing terminal Sidekick behavior and distilled the useful loop: prepared context once, then fresh bounded transcript and exact-session screen evidence on every user turn.
2. It diagnosed live-path failures from real Silverbook rehearsals, including the `EPERM` PID false negative, stale Recall session conflict, sandbox database access, WAL persistence, wrong CLI binding, and dead-terminal recovery.
3. It implemented the Rust provider contract and session engine, the native Tauri Sidekick window, the Codex app-server adapter, and the JavaScript reference/evaluation harness.
4. It used adversarial review loops to catch background-to-foreground publication races, late completions after stop, evidence provenance gaps, a golden-scorer false positive, and provider-specific concepts leaking into Minutes core.
5. It ran the Meridian scenario through the real Codex backend. The best recorded run passed all 15 quality checks; first-token p95 was 2.821 seconds and total-turn p95 was 8.544 seconds.

Codex session ID for the core Sidekick implementation session:

```text
019f85e9-3f1b-7962-b58b-045e4433504b
```

## Judge setup

The desktop demo requires macOS 15 or newer, Rust/CMake build prerequisites already described in this README, and the Codex CLI installed and signed in.

```bash
git clone --branch feat/codex-sidekick-demo https://github.com/silverstein/minutes.git
cd minutes
./scripts/install-dev-app.sh
```

The install script creates and launches `~/Applications/Minutes Dev.app` with a stable local development identity. Grant Microphone permission. Grant Screen & System Audio Recording if you want screen grounding.

### Use Sidekick

1. Start **Minutes Dev** and choose **Start Recording**. Use **Live** only if you want transcript-only assistance without recorded screen context.
2. Click **Sidekick** beside the active session.
3. Speak naturally; optionally put role, goals, people, or known facts in `SIDEKICK_BRIEF.md`.
4. Type a question or direction in the Sidekick window. Each turn refreshes the bounded live transcript and exact-session screen evidence before it answers.
5. Try a decision question rather than a recall question. Sidekick is designed to compute consequences, expose contradictions and risks, find thresholds, and suggest reversible moves—not merely summarize.

The current adapter sends only the bounded reasoning window to the selected Codex backend. Audio, the full transcript corpus, and the meeting database remain local. A fully local reasoning adapter is an explicit architectural target of the provider contract.

## Reproducible validation

Fast deterministic checks:

```bash
cargo test -p minutes-core live_sidekick -- --test-threads=1
cargo test -p minutes-app codex_reasoning_backend -- --test-threads=1
node --test \
  scripts/test/codex_app_server.test.mjs \
  scripts/test/sidekick_provider.test.mjs \
  scripts/test/sidekick_session.test.mjs \
  scripts/test/sidekick_rehearsal_golden.test.mjs
```

Live Codex Meridian evaluation (uses Codex app-server and therefore requires a signed-in Codex CLI):

```bash
node scripts/sidekick_session_eval.mjs \
  --repeat 1 \
  --output /tmp/sidekick-session-eval.json
```

The golden fixture and checker live in [`tests/eval/sidekick_rehearsal_golden.mjs`](tests/eval/sidekick_rehearsal_golden.mjs). The live runner is [`scripts/sidekick_session_eval.mjs`](scripts/sidekick_session_eval.mjs).

## Timestamped Build Week commits

All timestamps below are the author timestamps recorded by Git. PDT is UTC−07:00 on July 21–22, 2026.

| Commit | UTC | Pacific | Build Week contribution |
| --- | --- | --- | --- |
| `115739b6` | 2026-07-21 20:25:52 | 13:25:52 PDT | Add Codex Sidekick preview |
| `98e7c8f1` | 2026-07-21 20:41:57 | 13:41:57 PDT | Treat sandbox `EPERM` PID checks as alive |
| `30d114e1` | 2026-07-21 21:03:40 | 14:03:40 PDT | Restore fresh transcript + screen grounding on each turn |
| `d22b08b8` | 2026-07-21 21:16:12 | 14:16:12 PDT | Add Meridian rehearsal gate |
| `21de6ec7` | 2026-07-21 21:28:19 | 14:28:19 PDT | Replace a restored Recall session before Sidekick starts |
| `d7d60efa` | 2026-07-21 21:41:19 | 14:41:19 PDT | Read live context from the Sidekick sandbox |
| `1fd4410e` | 2026-07-21 21:44:35 | 14:44:35 PDT | Persist context WAL for sandbox readers |
| `2bf55e04` | 2026-07-21 21:52:32 | 14:52:32 PDT | Bind Sidekick to the running app CLI |
| `e20a9ef8` | 2026-07-21 21:56:10 | 14:56:10 PDT | Make a dead Sidekick terminal recoverable |
| `8f9f48b2` | 2026-07-21 21:59:51 | 14:59:51 PDT | Pin Sidekick to the bundled Minutes CLI |
| `2576cc90` | 2026-07-21 23:07:54 | 16:07:54 PDT | Add provider-neutral native Sidekick, persistent Codex app-server adapter, and adversarial harness |
| `40acc2ab` | 2026-07-21 23:09:27 | 2026-07-21 16:09:27 PDT | Add the Build Week submission guide and project boundary |
| `b77453af` | 2026-07-22 00:12:42 | 2026-07-21 17:12:42 PDT | Harden native Sidekick behavior for production use |
| `064635d2` | 2026-07-22 00:26:42 | 2026-07-21 17:26:42 PDT | Add the installed-binary headless acceptance path |
| `d1de9f83` | 2026-07-22 00:30:20 | 2026-07-21 17:30:20 PDT | Support isolated synthetic Sidekick fixtures |
| `3c6693cf` | 2026-07-22 00:32:40 | 2026-07-21 17:32:40 PDT | Keep strict Codex isolation configuration valid |
| `c0ae8b30` | 2026-07-22 00:51:26 | 2026-07-21 17:51:26 PDT | Bind the Sidekick listener during desktop startup |
| `4b6fcb25` | 2026-07-22 01:20:05 | 2026-07-21 18:20:05 PDT | Gate desktop installs on frontend readiness |
| `325f673a` | 2026-07-22 02:13:39 | 2026-07-21 19:13:39 PDT | Make Sidekick acceptance hermetic |
| `c3e8dd86` | 2026-07-22 02:19:48 | 2026-07-21 19:19:48 PDT | Tolerate cold dev-app registration |
| `a7276e19` | 2026-07-22 03:09:38 | 2026-07-21 20:09:38 PDT | Harden Sidekick strategy quality and golden scoring |
| `3c04c34b` | 2026-07-22 03:30:18 | 2026-07-21 20:30:18 PDT | Keep desktop startup off meeting storage |
| `0d250143` | 2026-07-22 03:49:36 | 2026-07-21 20:49:36 PDT | Decouple frontend readiness from optional hydration |
| `80b95870` | 2026-07-22 04:54:19 | 2026-07-21 21:54:19 PDT | Preserve quantified contract remedies in held-out evals |
| `feeddae2` | 2026-07-22 05:02:09 | 2026-07-21 22:02:09 PDT | Fail closed on contract semantics |
| `265a93ec` | 2026-07-22 05:10:53 | 2026-07-21 22:10:53 PDT | Define an exact contract-output grammar |
| `4c38edb6` | 2026-07-22 05:29:21 | 2026-07-21 22:29:21 PDT | Attest the canonical Sidekick installation |
| `3df8aa35` | 2026-07-22 05:49:05 | 2026-07-21 22:49:05 PDT | Close reviewed Sidekick acceptance bypasses |
| `70a3d033` | 2026-07-22 05:57:44 | 2026-07-21 22:57:44 PDT | Align Sidekick host and contract identity |
| `1c7f0511` | 2026-07-22 08:27:55 | 2026-07-22 01:27:55 PDT | Harden the native UI/provider gate, exact evidence bytes, adversarial mutations, and truthful scope |

This branch intentionally remains separate from `main` for Build Week review and live rehearsal.
