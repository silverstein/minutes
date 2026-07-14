# RFC 0005: Copilot Deterministic Evaluation Contract

Status: accepted as the copilot eval suite v1 contract.

This RFC versions the replay and scoring contract for the real-time meeting
copilot. A behavior change is not considered an improvement merely because it
looks better in one live meeting: it must preserve or improve this suite, and
new failure modes must become fixtures.

## Scope and Safety

The default corpus is embedded from
`crates/core/tests/fixtures/copilot_eval/v1/`. Every v1 transcript is synthetic;
the directory contains no private meeting text, customer data, or copied call
content. A future public or redacted fixture must declare that provenance in
its `content_origin` field.

Replay is an additive copilot consumer. It does not read or write the event
log, transcript JSONL, promoted meeting artifacts, or audio. It does not alter
the durable transcript contract in RFC 0003 or the partial/final contract in
RFC 0004.

## Version and Determinism

Suite version `1` uses fixed seed `0x4d494e5554455301`. Fixture parsing rejects
a different `schema_version`. Any incompatible change to fixture meaning,
opportunity matching, metric denominators, percentile selection, or baseline
interpretation requires a new suite version.

Final utterances are replayed at their meeting-relative end offset. Explicit
partial revisions are replayed at their declared offsets. When
`synthesize_partials` is enabled and no explicit revisions exist, v1 emits
stable 45%, 72%, and 90% word prefixes near 30%, 60%, and 82% of the utterance.
A small fixed-seed jitter prevents the corpus from accidentally depending on
perfectly uniform chunking while remaining byte-for-byte reproducible.

The replay replaces an older partial for the same utterance; it never appends
partial revisions as separate speech. Requests carry the same
`CopilotRequest`, `CopilotUtterance`, freshness lineage, `NudgePolicy`, and
single-lane `CopilotRunner` behavior as production.

`CopilotRunner` accepts an injected `CopilotClock`. Production defaults to the
system clock. Eval uses a fixed UTC epoch and logical monotonic microseconds.
Accelerated mode advances the logical clock immediately. Real-time mode sleeps
until each logical milestone and then advances to the exact same value. Sleep
and worker scheduling therefore cannot change scores or recorded latencies.

The scored provider is `mock/scripted-v1`. It makes no network requests. Rules
match a triggering update kind, utterance sequence, and/or text cue, then emit
a canned draft at declared first-token and completion offsets. An absent draft
is deliberate silence. Slow rules can be superseded while blocked, exercising
runner cancellation and delivery-time freshness. Ollama and Apple provider
types remain available to the production runner, but nondeterministic provider
output is never a CI baseline.

## Scoring Contract

Matching is case-insensitive and punctuation-insensitive. A nudge matches an
opportunity when its evidence publication time is inside the labeled range,
its kind matches when a kind is specified, and its text or source chip contains
at least one `match_any` term. Opportunities are consumed one-to-one in fixture
order.

- **Useful-nudge precision** is uniquely matched delivered nudges divided by
  all delivered nudges. A second nudge for an already consumed opportunity is
  not useful.
- **Opportunity recall** is uniquely matched opportunities divided by labeled
  opportunities. This supplemental guard prevents a silent implementation
  from receiving perfect precision.
- **Stale-nudge rate** is delivered nudges whose global evidence revision is no
  longer latest, or whose grounded partial lineage has already been replaced
  or retracted, divided by delivered nudges. Runner-filtered responses are not
  deliveries and do not enter the denominator.
- **Contradiction-after-revision rate** is delivered partial-grounded nudges
  whose labeled source phrase is semantically reversed by a later revision,
  divided by delivered partial-grounded nudges. Reversals are explicit fixture
  labels, such as `Approve` to `Reject`; the scorer does not guess semantics.
- **Duplicate/nagging rate** is delivered nudges that rematch an already
  consumed opportunity or repeat the same normalized kind and text within 30
  seconds, divided by delivered nudges.
- **No-nudge quality** is labeled no-opportunity ranges containing no nudge
  grounded in evidence from that range, divided by all labeled no-opportunity
  ranges.

An empty precision denominator scores 1.0; empty bad-rate denominators score
0.0; an empty no-opportunity or opportunity denominator scores 1.0. The suite
also reports the raw numerator and denominator so these identities cannot hide
corpus coverage.

## Latency Contract

Replay feeds deterministic `PartialLatencySeed` values into the existing
in-memory latency tracker. It does not persist timing records. The report
derives these non-overlapping stages, plus two useful rollups:

| Stage | Sample |
| --- | --- |
| `audio_to_partial` | `partial_published_us - audio_received_us` |
| `partial_to_trigger` | `trigger_us - partial_published_us` |
| `trigger_to_context` | `context_ready_us - trigger_us` |
| `context_to_model` | `model_request_us - context_ready_us` |
| `model_to_first_token` | `first_token_us - model_request_us` |
| `first_token_to_nudge` | `nudge_us - first_token_us` |
| `model_to_nudge` | `nudge_us - model_request_us` |
| `audio_to_nudge` | `nudge_us - audio_received_us` |

Missing stages are omitted from that stage's samples. p50 and p95 use the
nearest-rank value over sorted microsecond samples and are rendered in
milliseconds. Fixture reports aggregate one runner's records; the suite report
aggregates all fixture records before taking percentiles.

## V1 Corpus

- `synthetic-long-monologue`: fixed-seed partial synthesis must find an early
  owner/deadline opportunity without waiting for the final.
- `synthetic-approve-to-reject`: a slow `Approve` response must be cancelled
  after the partial becomes `Reject`; only final rejection guidance may render.
- `synthetic-overlapping-opportunities`: owner and success-metric labels overlap
  but require distinct, non-repeating nudges.
- `synthetic-no-opportunity-stretch`: routine status narration must stay silent.

Metric positive-control unit tests separately inject stale, contradictory, and
duplicate observations so a zero-valued baseline cannot mask a broken scorer.

## Entry Points and Baseline

```text
minutes copilot eval [--fixtures DIR] [--accelerated] [--json]
```

Without `--accelerated`, replay runs at transcript/model wall pace while using
the deterministic logical timestamps. The default output is a table followed
by one compact `summary_json=` record. `--json` prints the full report.
`--fixtures` loads sorted `*.json` files from a custom directory. A failed
baseline exits non-zero.

`cargo test -p minutes-core` runs the built-in suite in accelerated mode twice,
asserts identical reports, and enforces these v1 thresholds:

| Metric | Threshold |
| --- | ---: |
| useful-nudge precision | at least 0.90 |
| opportunity recall | at least 0.90 |
| stale-nudge rate | at most 0.00 |
| contradiction-after-revision rate | at most 0.00 |
| duplicate/nagging rate | at most 0.00 |
| no-nudge quality | at least 1.00 |
| model-to-first-token p95 | at most 100 ms |
| audio-to-nudge p95 | at most 5000 ms |

Threshold changes are scoring-contract changes and must be reviewed alongside
the report delta and an explanation of why the quality bar moved.
