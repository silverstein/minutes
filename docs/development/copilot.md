# Developing Coach and live-assistance boundaries

Coach is implemented in `crates/core/src/copilot/` and wired to the portable `minutes copilot` CLI in `crates/cli/src/main.rs`. Start with [RFC 0004](../rfcs/0004-copilot-realtime-stream.md), which freezes the transcript/nudge contract and failure-isolation rules. [RFC 0005](../rfcs/0005-copilot-eval.md) defines deterministic replay and scoring.

[RFC 0006](../rfcs/0006-live-sidekick-session-and-eval.md) and the
[staged implementation plan](../plans/live-assistance-orchestration-2026-07-15.md)
define how Coach stays separate from Terminal Sidekick and Native Recall.
RFC 0006 is a target contract: the current live-sidekick branch does not add a
native GUI or change Coach's runtime.

## Surface ownership

| Surface | Owner | Must not do |
| --- | --- | --- |
| Coach HUD | Existing `copilot` runtime, cadence, prompts, providers, feedback, and HUD | Become arbitrary terminal chat, inherit terminal tools, or stop capture as a side effect |
| Terminal Sidekick | Host-managed interactive agent plus the generated `minutes-live-sidekick` contract | Claim proactive monitoring without proven host preemption, or implement another transcript polling loop |
| Native Recall | Future Minutes-owned session manager and capability-gated read-only provider path | Reuse unrestricted PTY authority or silently absorb Coach output into chat history |

Coach nudges may eventually be presented in Recall only as separately labeled,
provenance-bearing evidence. Recall must not pause, resume, or stop Coach as an
ordinary chat side effect.

## Runtime boundaries

| Boundary | Invariant |
| --- | --- |
| Capture producer | Lock-free/nonblocking partial publication; a full ring drops a partial. |
| Fast lane | One model request at a time; newer evidence cancels/suppresses older advice. |
| Depth lane | Retrieval and strategy run on an isolated worker and never block the fast or capture paths. |
| Queues | Command, depth, and event channels are bounded and use `try_send`; saturation sheds Coach work. |
| Failure | Provider timeouts, worker errors, and poisoned copilot mutexes recover or degrade Coach only. |
| Persistence | Partials, battle cards, strategy, nudges, and latency timelines remain process-local. |

Do not route Coach through the transcript JSONL, change final-event production, or add a second capture owner. The authoritative durable seam remains `live.utterance.final`; cross-process partials use the capture relay defined by RFC 0004.

## Security model

`CopilotRequest::trusted_system_prompt` and `StrategyRequest::system_prompt` contain policy only. Goals, transcript strings, battle-card history, and strategy state are encoded into delimited JSON user messages. Keep this role split when adding a provider. Provider request types intentionally have no tools field and `CopilotModel` has no tool-execution method.

`BattleCard::assemble` rebuilds the graph before use and relies on the default restricted-history exclusion for graph, structured intent/decision, and FTS sources. It fails closed rather than reading a stale graph. Never add an `include_restricted` control to Coach.

The HUD consumes only the versioned `Nudge`; it must not receive a request, battle card, or strategy snapshot. Native Coach windows request screen-share content protection. macOS and Windows can honor it; Linux must show the warning returned by `evaluate_copilot_window_contract` because the compositor owns the final guarantee.

## Evaluation and gates

Run the embedded synthetic corpus without wall-clock sleeps:

```bash
minutes copilot eval --accelerated
minutes copilot eval --accelerated --json
```

Fixtures live in `crates/core/tests/fixtures/copilot_eval/v1/`. Add a synthetic regression fixture when behavior changes; never copy private meeting content into the corpus. The suite scores useful-nudge precision, opportunity recall, stale/contradictory/duplicate rates, no-nudge quality, strategy/grounding behavior, and latency percentiles against RFC 0005 thresholds.

For code changes, run:

```bash
cargo fmt --all -- --check
cargo build
RUST_TEST_THREADS=1 cargo test -p minutes-core -p minutes-cli
cargo clippy -p minutes-core -p minutes-cli -- -D warnings
```

Security coverage is colocated with the boundary it protects: prompt-role and injection tests in `types.rs`, `strategy.rs`, and `ollama_provider.rs`; restricted retrieval/prompt/strategy tests in `battle_card.rs`; HUD and capture-contention tests in CLI tests; queue, cancellation, poison recovery, and lane-isolation tests in `runner.rs`.

### Live-sidekick foundation checks

The separate live-sidekick foundation provides reducer tests, versioned
synthetic behavior specifications, privacy/schema gates, actual reducer replay,
and canonical routing replay. Run the checks that exist today:

```bash
cargo test -p minutes-core --lib live_sidekick --no-fail-fast
python3 scripts/check_live_sidekick_fixture_privacy.py
python3 -m unittest \
  tests/eval/test_live_sidekick_fixture_privacy.py \
  tests/eval/test_live_sidekick_fixture_schema.py
python3 tests/eval/live_sidekick_fixture_schema.py
cargo test -p minutes-core --no-default-features \
  --test live_sidekick_eval -- --test-threads=1
npm --prefix tooling/skills run build
node tests/eval/live_sidekick_routing_eval.mjs
```

The independent `live_sidekick_eval` CI job runs these public gates, and the
aggregate CI gate depends on it. The fourteen fixtures are schema-valid and
privacy-clean: four fully execute against the core reducer, four execute named
core projections, one executes three canonical routing cases, and five are
explicitly contract-only. The Rust runner covers eight core-target fixtures
across nine deterministic double replays. Do not present a projection's named
deferrals or the five future-orchestration contracts as passing behavior.

Any future Native Recall UI change also requires the repository's signed-dev
app workflow, keyboard and screen-reader checks, permission/error-state review,
and real-Mac click-testing. A green Rust or TypeScript build does not validate
the rendered interface.
