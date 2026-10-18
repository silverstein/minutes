# Evidence benchmark v0.2

This is a runnable benchmark **foundation**, not evidence that Minutes or a model is state of the art. It adds actual MCP calls, exact citations, reproducible corpus-size measurements and offline speech scoring. It does not run an LLM agent.

## Run deterministic conformance checks

Build the SDK and MCP package with the repository's normal dependency install and build procedure, then run from the repository root:

```bash
python3 -m unittest discover -s tests/eval/v02 -p 'test_*.py' -v
python3 tests/eval/v02/mcp_eval.py --sizes 100 --repeats 3 --output tests/eval/results/mcp-v02.json
```

For a source-built CLI, add `--cli target/debug/minutes` (or the release binary). The runner asserts it is the first repository candidate in MCP's existing fixed binary discovery, records its SHA256 and version, and checks it again after the run. A release binary shadowing a requested debug binary fails clearly. This does not introduce an environment override to the production server. Without `--cli`, the report explicitly leaves Rust binary provenance unestablished. MCP server JavaScript SHA256 is always recorded.

The second command launches **the built Minutes MCP server** over JSON-RPC stdio, performs the handshake, lists tools and invokes them. It does not substitute an imported SDK reader or give an answer agent ambient Markdown access. The evaluator uses source bytes only as an oracle for returned evidence. The temporary corpus and server profile are isolated from real meetings, with auto-setup and restricted overrides disabled; temporary files are removed at exit.

There are 52 base documents: 24 initial/revised decision pairs across software, museum operations and manufacturing, three identity fixtures and one restricted source. The 123 questions comprise 96 literal/token-order/case/prefix retrieval queries, 24 currentness checks and three no-answer queries. Mutation probes revoke and delete a previously retrieved source in the **same server session**. Identity probes distinguish Ann Carter, Ann Hughes and Joanna Flores. All data is fictional. Questions are generated from the same templates as answers: this is public synthetic smoke, **not** independent authorship, natural-query or model reasoning measurement.

Rows contain Recall@k, nDCG@k, relative result paths, citation counts and coverage. Citation validation requires an existing in-corpus source, matching raw-byte SHA256 revision and exact quoted UTF-16 offset range. Missing evidence cannot pass. Empty answers earn abstention credit only on explicit no-answer cases. Currentness requires the declared current ID, predecessor status and explicit resolved link. Missing tools/history, errors and timeouts fail instead of being skipped. Exit code 1 means a failed check.

Reports compare candidate recall/nDCG by query category and domain against an **offline simulation of the old exact-substring title/body matcher** on public fixtures. Candidate results always come from actual MCP. This baseline does not execute the previous release and provides no historical latency, policy or citation comparison. It is omitted for external YAML corpora. Failed candidate queries score zero in comparison summaries instead of disappearing from means.

## Scale and independent corpora

```bash
python3 tests/eval/v02/mcp_eval.py --sizes 100,1000,10000 --repeats 5 --output tests/eval/results/scale-v02.json
python3 tests/eval/v02/mcp_eval.py --export-suite /tmp/public-suite.json
python3 tests/eval/v02/mcp_eval.py --suite /secure/heldout-suite.json --sizes 1000 --repeats 5 --output /secure/heldout-result.json
```

Scale adds unrelated documents to the requested total. The report records actual counts, suite hash, startup time, separate history-tool latency and search p50/p95/max using nearest-rank percentiles. First calls are included; no warmup is discarded. Repeats use one server per size. These are measurements of the current host, **not a published baseline**. Runs beyond product corpus bounds can fail and must be reported as such. Sizes below the base corpus retain all base documents.

External JSON suites have `schema_version: 1`, descriptive `provenance`, `documents: [{path, content}]` and `cases`. Export the public suite for examples. Paths must be relative Markdown paths. Retrieval/abstention cases have `id`, `kind`, `domain`, `query`, `relevance: {relativePath: positiveIntegerGrade}`. Currentness cases have `id`, `kind: "currentness"`, `domain`, `topic`, `current_id`, `superseded_id`, `current_path`. Independent people must author and seal held-out tasks before tuning; a provenance flag does not establish blinding.

Revocation probes add a short synthetic source to any corpus. Exact-name probes run only for the public corpus. External fixtures/reports may contain sensitive data; choose their destinations appropriately.

## Real speech scoring

`speech_eval.py` accepts actual user-provided PCM WAV files with SHA256 and consent/license references, reference words and optional speaker intervals. It never downloads recordings or models. `speech-manifest.example.json` contains intentional placeholders and cannot produce invented results.

Hypotheses use this format:

```json
{"candidate":{"model":"exact model revision","configuration":"decoding settings and hardware"},"recordings":[{"id":"meeting-001","text":"candidate transcript","segments":[{"start":0,"end":1.2,"speaker":"speaker_0"}],"processing_seconds":2.5}]}
```

```bash
python3 tests/eval/v02/speech_eval.py --manifest /secure/manifest.json --hypotheses /secure/hypotheses.json --gates /secure/release-gates.json --output /secure/speech-report.json
```

Or run a local candidate adapter that writes a single hypothesis JSON:

```bash
python3 tests/eval/v02/speech_eval.py --manifest /secure/manifest.json --candidate 'model revision; decoding settings; hardware' --command '["python3","/absolute/path/adapter.py","--audio","{audio}","--output","{output}"]' --gates /secure/release-gates.json --output /secure/speech-report.json
```

The argv array runs without a shell. It receives audio/output paths, not reference words or speaker labels. Adapter processing time is measured by the harness; precomputed hypotheses retain `candidate-reported` timing provenance. IDs must cover the manifest exactly, without duplicates. Evaluators provide adapters because transcription invocation differs by backend.

| Metric | Definition and limitation |
| --- | --- |
| WER | Word-level substitutions + deletions + insertions / reference words; Unicode casefold, punctuation removed, apostrophes and decimal numbers retained. No semantic normalization. Can exceed 100%. |
| Critical errors | Missing/extra normalized phrase occurrences; annotate names and domain terms. Numeric tokens are automatic. English negations are automatic for `language: "en"`. Measures occurrences, not contextual attribution. |
| DER | Missed + false alarm + confused speaker time / reference speaker time; zero collar, overlap included, optimal global label mapping, maximum 12 distinct speakers per recording. Does not measure named identity accuracy. |
| Real-time factor | Processing seconds / audio seconds; aggregate requires timings for every recording. |

Development/holdout scores stay separate. WER and DER use micro averages (sum errors/durations before division). Gate JSON accepts `max_wer`, `max_der`, `max_critical_errors`, `max_real_time_factor`, `min_audio_seconds` and `min_recordings`, with finite nonnegative thresholds. Gates apply to holdout only. Missing requested metrics fail; a DER gate requires speaker scores for **every** holdout recording. With no gate file, the report is `not-configured` and exits 2. Establish thresholds before inspecting the holdout; suitable bounds depend on the target devices/workflows, so this harness prescribes no universal threshold.

## Release decision

### Choose a model from measured reports

`select_models.py` can recommend a candidate from supplied speech reports without changing any application defaults. It requires the same manifest hash and holdout IDs, matching measured platform/hardware, acceptable local/cloud privacy and estimated cost, sufficient holdout coverage, valid WER/DER/timing, and a passing predeclared report gate. It rejects synthetic reports and requires manifest provenance `kind: "consented-recordings"` or `"licensed-recordings"`, `independent_holdout: true`, `synthetic: false`. These are evaluator declarations, not independent certification. Only harness-measured timings are eligible.

Supply `--candidate-config /secure/candidate.json` when invoking `speech_eval.py` with `--command`. Candidate metadata must include `model`, `revision`, `hardware`, `platform`, `privacy: "local" | "cloud"`, and `cost_per_audio_hour` (estimated). The hardware and platform strings must exactly match the selection request.

```bash
python3 tests/eval/v02/select_models.py /secure/model-a.json /secure/model-b.json --manifest-sha256 ACTUAL_64_CHARACTER_HASH --hardware 'M3 Pro 18GB' --platform macos --privacy local --max-cost-per-audio-hour 0 --min-recordings 20 --min-audio-seconds 3600 --max-wer 0.15 --max-der 0.10 --max-critical-errors 0 --output /secure/model-recommendation.json
```

After eligibility checks, it sorts ascending by critical errors, WER, DER, then real-time factor. Metrics are recomputed from per-recording counts. Reports with different holdout coverage cannot be compared. With no eligible candidate it emits `insufficient-evidence` and exits 2. The thresholds above are an illustration, not validated product requirements. No supplied real reports means no winner.

### Qualification evidence

| Evidence | Establishes | Qualification requirement |
| --- | --- | --- |
| Public synthetic MCP | Contract, exact citations, currentness, policy regressions | Required CI pass; insufficient alone |
| Independent retrieval tasks | Natural-query relevance/abstention across domains | Protected holdout, independent authors, repeated runs, comparable baselines, published error cases |
| Consented/licensed speech | Performance on specified audio | Devices, noise, accents, languages and overlap; predeclared gates; same inputs for all candidate backends |
| Capture/native workflows | Preservation, permissions, interruption recovery and insertion | Real supported OSs/devices and approved native app identity; not simulated by this harness |

Do not select default models from a synthetic pass. Record build SHA, dependencies, hardware, model revision, decoding configuration and corpus hash with real reports. Native UX, capture reliability, named-speaker errors and answer-level reasoning or citation entailment require separate evidence. No unrun experiment is a success.
