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
- An independent, provider-neutral **evidence verifier** before publication. A real transcript receipt can no longer launder an invented claim: a separate one-candidate reasoning session checks the candidate against a newly sealed bounded transcript and exact-session image. Minutes suppresses unsupported facts, visual claims, contradictions, or uncertain verdicts. During continuous speech it refreshes the seal once, publishes only a supported result against that bounded seal, and preserves anything newer for the next decision cycle instead of starving forever.
- Minutes-owned attempt identities around every provider callback. An old verifier result cannot impersonate a newer verification even when two provider sessions legally reuse the same provider-local turn ID.
- A hybrid quality gate that combines deterministic arithmetic/provenance checks with a structured semantic judge. The judge is calibrated against an 11-case hidden human-labeled holdout set containing natural paraphrases, unsafe “ship everything” reversals, narrowed remedies, aggregate-only audit access, supplier vetoes, and contrast-clause sabotage. Native acceptance launches the three-run evaluator itself and binds the exact report bytes, source commit, three independent strategist/judge session sets, and current output bytes; a saved good report cannot bless a changed bad response.
- One attested Codex executable across the signed app, evaluator, evidence verifier, and exact-response judge. The path, bytes, and version are checked before and after, and the native UI gate uses one private read/execute-only copy for the entire run. This is an acceptance property of the Codex backend, not a dependency in Minutes core; local and alternative providers still implement the same neutral contracts.

## How Codex collaborated in building it

Codex was both the implementation collaborator and the first reasoning backend:

1. It compared the weak Coach iteration with the existing terminal Sidekick behavior and distilled the useful loop: prepared context once, then fresh bounded transcript and exact-session screen evidence on every user turn.
2. It diagnosed live-path failures from real Silverbook rehearsals, including the `EPERM` PID false negative, stale Recall session conflict, sandbox database access, WAL persistence, wrong CLI binding, and dead-terminal recovery.
3. It implemented the Rust provider contract and session engine, the native Tauri Sidekick window, the Codex app-server adapter, and the JavaScript reference/evaluation harness.
4. It used adversarial review loops to catch background-to-foreground publication races, late completions after stop, evidence provenance gaps, a golden-scorer false positive, and provider-specific concepts leaking into Minutes core.
5. It ran the Meridian scenario repeatedly through the real Codex backend, then used independent model grading only after deterministic safety checks. The latest provider-bound witnessed gate passes all three strategic runs and the 11/11 semantic calibration set under the checked-in 4-second first-token and 7-second fully verified p95 budgets.

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
  scripts/test/sidekick_evidence_verifier.test.mjs \
  scripts/test/sidekick_provider_attestation.test.mjs \
  scripts/test/sidekick_provider.test.mjs \
  scripts/test/sidekick_session.test.mjs \
  scripts/test/sidekick_exact_semantic_gate.test.mjs \
  scripts/test/sidekick_hybrid_quality_gate.test.mjs \
  scripts/test/sidekick_semantic_judge.test.mjs \
  scripts/test/sidekick_rehearsal_golden.test.mjs
```

Live Codex Meridian evaluation (uses Codex app-server and therefore requires a signed-in Codex CLI):

```bash
node scripts/sidekick_session_eval.mjs \
  --repeat 3 \
  --output /tmp/sidekick-session-eval.json
```

The golden fixture and checker live in [`tests/eval/sidekick_rehearsal_golden.mjs`](tests/eval/sidekick_rehearsal_golden.mjs). The human-labeled semantic holdout is [`tests/eval/sidekick_semantic_calibration.mjs`](tests/eval/sidekick_semantic_calibration.mjs). The live runner is [`scripts/sidekick_session_eval.mjs`](scripts/sidekick_session_eval.mjs).

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
| `ab8e2b32` | 2026-07-22 08:29:03 | 2026-07-22 01:29:03 PDT | Record the hardened Sidekick acceptance path |
| `62935265` | 2026-07-22 15:50:11 | 2026-07-22 08:50:11 PDT | Make the local development signing identity stable by default |
| `fbd24740` | 2026-07-22 16:36:34 | 2026-07-22 09:36:34 PDT | Run native Sidekick acceptance through macOS LaunchServices |
| `e24c5ba3` | 2026-07-22 16:42:57 | 2026-07-22 09:42:57 PDT | Match native acceptance processes by open-file identity |
| `c4cb0945` | 2026-07-22 16:50:44 | 2026-07-22 09:50:44 PDT | Surface bounded native acceptance launch failures |
| `c34d7c6d` | 2026-07-22 17:00:32 | 2026-07-22 10:00:32 PDT | Use a LaunchServices-safe parent lease |
| `a58be3e3` | 2026-07-22 17:14:20 | 2026-07-22 10:14:20 PDT | Make Sidekick screen-marker startup deterministic |
| `3e104ffd` | 2026-07-22 17:16:34 | 2026-07-22 10:16:34 PDT | Preserve signed application modes during development install |
| `0eae795d` | 2026-07-22 17:26:21 | 2026-07-22 10:26:21 PDT | Match active Minutes processes by executable identity |
| `03057ca6` | 2026-07-22 17:31:14 | 2026-07-22 10:31:14 PDT | Request fresh Screen Recording access for acceptance |
| `9fa93067` | 2026-07-22 17:39:17 | 2026-07-22 10:39:17 PDT | Request Screen Recording on the application UI thread |
| `0cff33f3` | 2026-07-22 19:22:51 | 2026-07-22 12:22:51 PDT | Wait for the visible recording Sidekick control |
| `61031a24` | 2026-07-22 19:31:47 | 2026-07-22 12:31:47 PDT | Verify screen markers across every rendered cell |
| `441a0bc7` | 2026-07-22 19:37:45 | 2026-07-22 12:37:45 PDT | Sample screen markers across Retina cells |
| `b6a1e04a` | 2026-07-22 19:53:15 | 2026-07-22 12:53:15 PDT | Reveal Minutes before Sidekick acceptance |
| `8075b180` | 2026-07-22 20:02:53 | 2026-07-22 13:02:53 PDT | Exercise the visible Sidekick recording pane |
| `66df6680` | 2026-07-22 20:05:49 | 2026-07-22 13:05:49 PDT | Report the Sidekick UI hit target |
| `372817c1` | 2026-07-22 20:10:39 | 2026-07-22 13:10:39 PDT | Traverse Coach onboarding during Sidekick acceptance |
| `0171bd7f` | 2026-07-22 20:15:13 | 2026-07-22 13:15:13 PDT | Tolerate cursor occlusion in screen proof |
| `022e4c12` | 2026-07-22 20:21:57 | 2026-07-22 13:21:57 PDT | Treat cursor artifacts as bounded marker errors |
| `5e44f46a` | 2026-07-22 20:27:43 | 2026-07-22 13:27:43 PDT | Wait for marker paint before screen capture |
| `ddac86d6` | 2026-07-22 20:32:55 | 2026-07-22 13:32:55 PDT | Isolate nonce grids from decorative colors |
| `90ac0834` | 2026-07-22 20:38:45 | 2026-07-22 13:38:45 PDT | Avoid cached fullscreen marker spaces |
| `53b69a02` | 2026-07-22 20:45:37 | 2026-07-22 13:45:37 PDT | Add a fiducial to Sidekick screen proof |
| `697f72a9` | 2026-07-22 20:48:52 | 2026-07-22 13:48:52 PDT | Report observed Sidekick marker geometry |
| `3f0094e1` | 2026-07-22 21:01:43 | 2026-07-22 14:01:43 PDT | Locate the Sidekick marker by its connected fiducial |
| `547e2074` | 2026-07-22 21:06:22 | 2026-07-22 14:06:22 PDT | Wait for Sidekick onboarding paint |
| `c9a66909` | 2026-07-22 21:11:59 | 2026-07-22 14:11:59 PDT | Bind Sidekick screen-context reads to the active session |
| `7dd209e8` | 2026-07-23 01:28:58 | 2026-07-22 18:28:58 PDT | Add evidence-verified publication, continuous-speech liveness, stale-event isolation, provider-bound semantic judging, and the witnessed autonomous quality gate |

This branch intentionally remains separate from `main` for Build Week review and live rehearsal.
