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

All timestamps below are the author timestamps recorded by Git. PDT is UTC−07:00 on July 21, 2026.

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

This branch intentionally remains separate from `main` for Build Week review and live rehearsal.
