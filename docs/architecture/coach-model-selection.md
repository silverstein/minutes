# Coach model selection

Coach uses a reviewed model manifest plus a measured setup probe. RAM chooses the first
candidate; the probe confirms that the candidate is fast enough on the actual computer.
Minutes never changes a working model in the background.

## Hardware tiers

The source of truth is `COPILOT_MODEL_TIERS` in `crates/core/src/config.rs`. The table is
ordered strongest-first. Apple Silicon uses the Ollama MLX/NVFP4 artifact; other platforms
use the corresponding portable registry tag.

| Installed RAM | Tier | Apple Silicon | Portable fallback |
|---:|---|---|---|
| 64 GB or more | beast | `qwen3.5:35b-a3b-nvfp4` | `qwen3.5:35b-a3b` |
| 32–63 GB | strong | `gemma4:26b-mlx` | `gemma4:26b` |
| 16–31 GB | mainstream | `qwen3.5:9b-mlx` | `qwen3.5:9b` |
| Less than 16 GB | modest | `qwen3.5:4b-mlx` | `qwen3.5:4b` |

`minutes coach setup` detects installed RAM and Apple Silicon, downloads the matching
candidate, warms it, and sends a production-shaped coaching request. Both production and
the probe explicitly send `think: false`. A candidate must meet the configured total
latency target and a time-to-first-token target of at most 1.5 seconds. If it misses either
budget, setup tries the next smaller tier and saves the first passing tag as
`copilot.fast_model`.

An explicitly configured non-manifest model is left alone. Use `minutes coach setup --model
<tag>` to choose one intentionally, or `minutes coach setup --retune` to discard an old
choice and repeat hardware selection.

## Freshness evaluation

`tooling/coach-models/candidates.toml` mirrors every current manifest tag and contains
human-added challengers. `scripts/coach_model_eval.py` selects the entries for the current
machine, pulls and warms each tag, then runs the real fast-lane coaching prompt over the
versioned copilot fixture corpus:

```sh
cargo build -p minutes-cli --no-default-features --release
python3 scripts/coach_model_eval.py \
  --minutes-bin target/release/minutes \
  --output /tmp/coach-model-report.md
```

The report records production-parser success, production-policy-visible nudges, fixture quality
metrics, TTFT and total-generation percentiles, and every parsed nudge. It compares evidence
only; it deliberately does not rank candidates or declare a winner.

The `Coach model freshness` GitHub Actions workflow runs monthly and on manual dispatch. A
hosted macOS runner evaluates only entries marked `small_ci = true`, uploads the full report
and Ollama log, and opens or updates one issue named `Coach model freshness report YYYY-MM`.
Larger tiers must be evaluated on suitable maintainer hardware by running the command above
without `--small-only`.

## Adding or promoting a model

1. Confirm that the exact Ollama tag exists and add it to
   `tooling/coach-models/candidates.toml` with `role = "challenger"`, its intended tier,
   platform, and a short source note. Mark `small_ci = true` only when a hosted macOS runner
   can safely load it.
2. Run the local evaluation on representative hardware. Preserve the generated report for
   human review; coaching quality is not reducible to the automated fixture metrics.
3. If a maintainer selects the challenger, update the centralized Rust manifest and the
   matching `role = "current"` entries in the candidates file in a reviewed PR.
4. Run `minutes coach setup --retune` on representative hardware to verify pull, probe, and
   persisted selection behavior.

The workflow never edits the manifest, opens a pull request, merges code, or changes a
user's configured model.
