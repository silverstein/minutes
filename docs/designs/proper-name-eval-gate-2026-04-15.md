# Proper-Name Eval Gate — 2026-04-15

## Goal

The decode-hints work for `minutes-h1zm` should not ship on vibes.

We need a repeatable way to compare:

- baseline transcription
- hinted transcription

against a small proper-name corpus that captures both upside and regression risk.

This gate is intentionally local-first. It is meant for maintainers to run on
real recordings that cannot live in the public repo.

## Runner

Use:

```bash
scripts/run_proper_name_eval.sh /abs/path/to/corpus.json
```

The runner executes the ignored Rust test:

```bash
cargo test -p minutes-core --features whisper,parakeet proper_name_eval_corpus -- --ignored --nocapture
```

and prints JSON results to stdout.

## Corpus format

See:

`tests/fixtures/proper-name-eval.example.json`

Each entry can specify:

- `id`
- `audio_path`
- `content_type`
- `engine`
- `reference_text` or `reference_path`
- `title`
- `calendar_event_title`
- `pre_context`
- `attendees`
- `identity_name`
- `identity_aliases`
- `language`
- `max_wer_regression`
- `require_hinted_terms`
- `forbid_hinted_terms`

## What the harness measures

For each case it computes:

- baseline WER
- hinted WER
- hinted minus baseline delta
- required-term hits in baseline and hinted output
- forbidden-term hits in baseline and hinted output

If any case exceeds `max_wer_regression`, misses a required hinted term, or
contains a forbidden hinted term in the hinted output, the test fails.

## Recommended corpus shape

Build a small but adversarial set:

- single-token common first names
- multi-token full names
- nickname ↔ formal-name variants
- connector/product terms that should survive
- noisy or degraded audio
- at least one non-English or multilingual proper-name case

## Promotion criteria

Before changing any default behavior, we should have evidence that:

1. Whisper prompting improves or holds WER on the corpus.
2. Parakeet local boosts do not reintroduce common-name hallucinations.
3. Common-name cases do not regress beyond the explicit per-case threshold.
4. Required names/terms show up in hinted output more reliably than baseline.

Until then, `minutes-v3bj` remains the release gate for decode-hint behavior.

## Current Readout

Initial local evaluation from 2026-04-15 / 2026-04-16 supports a **narrow
self-name correction slice**, not a general proper-name claim.

What held up:

- A Parakeet starter corpus focused on self-name cases passed cleanly once the
  candidate path included the guarded self-name normalization layer.
- Those self-name cases all improved, and none regressed.

What did not hold up:

- The broader corpus still failed on an external proper-noun case.
- Current decode-time hinting does not yet justify a general statement like
  "Minutes now fixes people names."

Practical interpretation:

1. Self-name correction for safe intro/test patterns looks viable.
2. External proper nouns remain an open research problem.
3. Release notes and docs should keep those scopes separate.

## Safe Product Wording

If we describe the currently supported slice, keep it narrow and literal:

> Minutes can improve the recorded user's own name in safe self-introduction
> and short self-reference patterns during batch meeting processing.

Avoid wording like:

- "Minutes fixes people names"
- "Minutes now corrects proper nouns"
- "Minutes understands attendee names generally"

Those broader claims are not supported by the current evaluation results.
