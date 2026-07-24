# Sidekick Engine Replay Checkpoint — 2026-07-24

This checkpoint adds the first one-command, no-human Sidekick orchestration
gate. It is a milestone inside `minutes-k1qp.2`, not completion of that bead
and not a general SOTA claim.

## Run it

From the repository root:

```bash
bash scripts/sidekick_engine_eval.sh
```

The command runs without a microphone, screen, network, click, or typed
response. It writes a bounded JSON artifact to:

```text
target/sidekick-eval/live-sidekick-engine-eval.json
```

The artifact contains no meeting text, user text, image bytes, local paths, or
provider payloads. It records only scenario IDs, pass/fail assertions, bounded
counts, simulated provider timing, an explicit coverage boundary, and a
deterministic SHA-256 digest.

## What passed

The checkpoint executes each scenario twice through the public production
`LiveSidekickEngine` and requires byte-equivalent normalized results:

| Scenario | Production behavior exercised |
| --- | --- |
| Exact screen publication | Exact selected PNG bytes, evidence receipt, independent verifier, publication gate |
| Correction during verification | Moving transcript window, refreshed verification, contradiction rejection, fresh regeneration |
| Provider failure and recovery | Retryable network-class failure, capture isolation, provider epoch replacement, successful retry |
| Screen unavailable | Missing image bytes rejected, fabricated visual provenance blocked, transcript-only recovery |
| Foreground preemption | Typed user turn interrupts background work; late background completion is ignored |
| Provider steering | Active persistent turn is steered into foreground work without a second generation |
| Evidence bounds | Only the newest configured transcript items enter a request and the serialized envelope stays under budget |
| Teardown | Active work and provider sessions close; a late completion has no visible effect |

Result at this checkpoint:

```text
8/8 scenarios
29/29 assertions
reproducible=true
digest=149b3933272bb8d21be6e93559c45f54703df478efac95541c6c44c9416fb63d
```

The deterministic backend lives only under the example/integration-test
boundary. It implements the same persistent, streaming, steerable,
provider-neutral contract as Codex app-server or a future Claude/local
backend; it is not compiled into the production Minutes engine.

## Honest coverage boundary

This checkpoint uses the real Minutes reducer, evidence-window assembler,
provider contract, verification gate, suppression/publication decision,
recovery, and teardown paths.

It does **not** yet exercise:

- native microphone or system-audio capture;
- speech recognition from prerecorded audio;
- two-speaker diarization;
- the native macOS Screen Recording permission adapter;
- historical meeting or repository retrieval adapters;
- real Codex, Claude-compatible, or local-model network behavior; or
- signed-app UI event order.

Those remain required by `minutes-k1qp.2`. The existing signed-Mac checkpoint
and real Codex SOTA suite complement this deterministic lane, but the three
artifacts must not be collapsed into one inflated release claim.

## Adversarial review outcome

The initial implementation placed scripted eval machinery in the production
core module. Review rejected that structure even though the scenarios passed.
The final shape keeps all deterministic provider state under
`crates/core/tests/support/`, shares it with a tiny example entry point, and
leaves the shipped provider-neutral engine untouched.

The artifact also labels every provider duration as simulated and sets
`release_ready_from_this_report_alone=false`. A future change cannot turn this
gate green into a claim about native ASR, diarization, permissions, cloud
latency, or signed-app UX.
