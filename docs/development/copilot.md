# Developing Coach

Coach is implemented in `crates/core/src/copilot/` and wired to the portable `minutes copilot` CLI in `crates/cli/src/main.rs`. Start with [RFC 0004](../rfcs/0004-copilot-realtime-stream.md), which freezes the transcript/nudge contract and failure-isolation rules. [RFC 0005](../rfcs/0005-copilot-eval.md) defines deterministic replay and scoring.

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
