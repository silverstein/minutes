# Live-sidekick eval corpus v1

Every JSON fixture in this directory is synthetic and authored from scratch.
The corpus contains no copied, redacted, anonymized, or name-swapped meeting
text. Speaker identity is represented only by the role tokens documented in
each fixture's `privacy.approved_role_tokens` list.

The v1 documents are versioned product behavior contracts rather than
serialized Rust state. Events use this stable envelope:

```json
{
  "at_ms": 0,
  "kind": "capture_started",
  "session_id": "SESSION_A",
  "payload": {
    "capture_session_id": "CAPTURE_A",
    "capture_mode": "live"
  }
}
```

`session_id` is omitted only for surface-routing requests that are not yet
attached to a capture. Schema v1 requires explicit stable synthetic IDs in
every executable payload: adapters do not infer capture IDs, source event IDs,
or background run IDs from surrounding fields. Expectations use action-kind
strings plus structured state, provenance, cadence, and parity assertions.

## Execution truth

Every fixture has an `execution` classification validated by CI:

- `executable`: the declared contract is replayed against an implementation.
- `executable_projection`: the named reducer-owned subset is replayed, and
  `deferred_assertions` records exactly what is not executed yet.
- `contract_only`: future orchestration behavior is schema- and
  privacy-validated but is not presented as implementation proof.

The current v1 corpus contains:

- 4 executable core-reducer fixtures;
- 4 executable core-reducer projections;
- 1 executable canonical skill-routing fixture;
- 5 contract-only future-orchestration fixtures.

The Rust integration test adapts only explicitly selected event indexes to the
public reducer API, replays each case twice, compares the two results, then
compares the normalized reduction trace and final state to the fixture. Where
an action carries reducer-issued invocation identity, the expected trace pins
its sequence and policy/user generations. The routing runner likewise calls
the compiled canonical router twice. Contract-only events never enter either
runner.

## Public gates

Run the same public gates used by CI:

```text
python3 scripts/check_live_sidekick_fixture_privacy.py
python3 -m unittest tests/eval/test_live_sidekick_fixture_privacy.py tests/eval/test_live_sidekick_fixture_schema.py
python3 tests/eval/live_sidekick_fixture_schema.py
cargo test -p minutes-core --no-default-features --test live_sidekick_eval -- --test-threads=1
(cd tooling/skills && npm install --no-save --no-audit --no-fund typescript@5.9.3 @types/node@22.19.11 && ./node_modules/.bin/tsc -p tsconfig.json --typeRoots node_modules/@types)
node tests/eval/live_sidekick_routing_eval.mjs
```

The privacy test and schema test include synthetic negative controls so CI
also proves the gates fail closed. The schema command reports executable,
projection, and contract-only counts instead of implying that all fixtures ran.

## Local-only private-corpus overlap review

Before publishing a new fixture batch, a privacy reviewer must also run the
optional local-only overlap gate against an authorized private corpus. The
command reports only pass/fail counts, thresholds, and fixture IDs. It never
prints matching text or corpus hashes.
