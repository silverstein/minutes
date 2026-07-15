# Live-sidekick eval corpus v1

Every JSON fixture in this directory is synthetic and authored from scratch.
The corpus contains no copied, redacted, anonymized, or name-swapped meeting
text. Speaker identity is represented only by the role tokens documented in
each fixture's `privacy.approved_role_tokens` list.

The v1 documents are behavior contracts rather than serialized Rust state.
Events use this stable envelope:

```json
{
  "at_ms": 0,
  "kind": "capture_started",
  "session_id": "SESSION_A",
  "payload": {}
}
```

`session_id` is omitted only for surface-routing requests that are not yet
attached to a capture. Expectations use action-kind strings plus structured
state, provenance, cadence, and parity assertions. A deterministic runner may
adapt this public schema to internal reducer types without making those types
part of the fixture contract.

Run the public structural gate with:

```text
python3 scripts/check_live_sidekick_fixture_privacy.py
```

Before publishing a new fixture batch, a privacy reviewer must also run the
optional local-only overlap gate against an authorized private corpus. The
command reports only pass/fail counts, thresholds, and fixture IDs. It never
prints matching text or corpus hashes.
