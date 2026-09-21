# Sidekick SOTA Adversarial Corpus v1

This corpus evaluates whether Sidekick improves a live decision rather than
merely summarizing a transcript. Every fixture is behavior-first, synthetic,
and structurally validated before it can run.

The initial slice contains seven scenarios across six domains:

- four `executable` scenarios run through the persistent provider-neutral
  Sidekick session, independent evidence verifier, mechanical provenance gate,
  and fixture-specific semantic judge;
- three `executable_projection` scenarios specify the required historical,
  relationship, and repository behavior without pretending the native product
  already exposes those evidence lanes; and
- no projection result counts as a production pass.

The restricted-board scenario begins after Minutes has excluded the synthetic
restricted artifact. It tests transcript prompt-injection resistance,
hallucination restraint, and post-filter non-disclosure. It does not claim to
exercise the upstream retrieval filter; runner coverage reports that boundary
as untested.

Validate fixture structure, privacy, scoring, and runner selection:

```sh
node --test scripts/test/sidekick_sota_fixture.test.mjs \
  scripts/test/sidekick_sota_judge.test.mjs \
  scripts/test/sidekick_sota_eval.test.mjs
python3 scripts/check_live_sidekick_fixture_privacy.py \
  tests/fixtures/sidekick_sota/v1
```

The public structural check catches identifiers, unsafe fields, role-token
violations, and other repository hygiene risks. It cannot prove synthetic
provenance by itself. Before publishing or releasing a changed corpus, the
local-only overlap gate is mandatory and blocks if its private corpus is not
configured:

```sh
MINUTES_PRIVATE_EVAL_CORPUS_DIR=/path/to/private/corpus \
  python3 scripts/check_sidekick_sota_release_privacy.py
```

The overlap gate reports only fixture IDs and counts. It never prints private
corpus paths, matching text, or hashes.

List the runnable and deferred scenarios without starting a model:

```sh
node scripts/sidekick_sota_eval.mjs --list
```

Run the current production-path scenarios autonomously:

```sh
node scripts/sidekick_sota_eval.mjs \
  --allow-partial \
  --output target/sidekick-sota-eval.json
```

Use `--scenario synthetic-runway-hiring-tradeoff` to isolate one scenario.
Scenario runs and corpora with deferred projections exit nonzero unless
`--allow-partial` is explicit. Partial success is behavioral development
evidence, never a release pass.
Projection fixtures cannot be promoted into passing runs with a CLI flag. They
become executable only when the missing product evidence lane exists and the
fixture status is changed in reviewed source.
