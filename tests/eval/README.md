# Minutes Eval v0.1

**New:** [v0.2 evaluation infrastructure](v02/README.md) adds actual MCP stdio
retrieval, exact source citation checks, explicit-decision currentness, identity
and revocation probes, corpus-size latency measurements, and scoring of supplied
consented/licensed speech. Its built-in 123 questions are public synthetic
conformance fixtures, not blind-authored holdouts or production quality evidence.
The v0.1 agent-with-files experiment below remains a separate evaluation surface.

## Diarization model-set comparison

The diarization eval uses repository-owned one-speaker audio with a small
hand-authored reference. It reports zero-collar, overlap-scored 10 ms DER,
speaker-count accuracy, and segment-boundary mean absolute error. It is a
boundary/count regression corpus, not a multi-speaker quality benchmark.

```bash
./scripts/run_diarization_eval.sh --model legacy
./scripts/run_diarization_eval.sh --model community-1
```

The script downloads the selected ONNX model set when it is not installed;
model binaries are not committed to the repository.

Can an AI agent actually answer real questions about your meetings?

Cloud meeting tools cap cross-meeting chat at somewhere between one meeting (Fireflies' Fred) and twenty-five (Granola). Nobody publishes a head-to-head on whether agents can synthesize across real meeting corpora. v0.1 is a starter shot at that gap.

**What v0.1 is:** scaffolding. 10 fictional markdown files (9 meetings + 1 voice memo), 20 questions authored by the repo maintainer, a runner that points Claude/Codex/Gemini at the fixture corpus via their CLIs. Enough to prove the harness works and to start measuring whether agents given Minutes output can resolve contradictions, track commitments across meetings, infer relationship ownership, and reason about what's current versus stale.

**What v0.1 is NOT:** a rigorous benchmark. The questions are authored by the same person who built the system. The corpus is single-domain SaaS founder theater. There are no blind-authored holdout questions, no adversarial hallucination traps, no noisy-transcript variants, no baseline comparisons against raw-transcript-only or cloud competitors, and no repeated runs. Don't cite v0.1 results as category-level evidence.

**v0.2 (bead minutes-n5vj)** does the rigorous version: 3+ corpora, 100+ questions, blind-authored holdouts, citation-bound answers, hallucination traps, noisy transcripts, and head-to-head baselines. Until that ships, this is a working harness with 20 honest questions and a public invitation to point out where the methodology is thin.

## Why this exists

If an agent given Minutes output can't answer basic questions about a realistic corpus, the frontmatter schema is wrong or the transcription pipeline lost the signal. This is a forcing function for Minutes' own output quality as much as it is a category-level probe.

## What's in this directory

```
tests/eval/
├── README.md                 (this file)
├── fixtures/
│   └── meetings/             (10 seeded markdown files: 9 meetings + 1 voice memo)
├── questions.yml             (20 ground-truth questions)
├── run_eval.py               (runner — subprocesses agent CLIs)
├── grade.py                  (optional LLM-as-judge grader)
└── results/                  (per-run JSON logs, gitignored)
```

Results tables live at `/docs/eval/results-v0.1.md` (in the repo root's docs/, not here).

## How to run

### Prerequisites

Install at least one agent CLI. The runner calls whichever are on your PATH.

- Claude Code: `npm i -g @anthropic-ai/claude-code` (or `brew install anthropic-ai/claude/claude-code`)
- Codex: `npm i -g @openai/codex-cli` (or equivalent)
- Gemini CLI: `npm i -g @google/gemini-cli`

Each CLI must be authenticated. Minutes itself does not need to be installed. The runner invokes each agent CLI with `cwd` set to the fixture directory, so the agent has ambient access to the markdown files in its working context. No MCP config is required for the eval; MCP is a separate optional integration path at runtime.

### Manual run

```bash
cd tests/eval
python3 run_eval.py --agent claude
python3 run_eval.py --agent codex
python3 run_eval.py --agent gemini
python3 grade.py --all          # optional LLM-as-judge pre-pass
```

`run_eval.py` writes `results/<agent>-<timestamp>.json`. `grade.py` reads those, pre-grades with an LLM, and emits a markdown table. Human sign-off on each pass/fail is still required for v0.1.

### Publishing results

Copy the graded table into `/docs/eval/results-v0.1.md`, add a short prose summary of what each agent got right and wrong, and commit. Announce on the release notes of the version that ships this eval and in a follow-up tweet.

## What's tested

Twenty questions across six categories:

| Category | Count | Example |
|---|---|---|
| Relationship ownership | 3 | "Who owns the Northwind relationship?" |
| Active priorities | 3 | "What's the current pricing decision?" |
| What changed recently | 3 | "What got killed in the last product prioritization meeting?" |
| Decision history + contradictions | 4 | "What's been decided about pricing, in order, and which is current?" |
| Commitment / staleness | 4 | "What commitments are overdue?" |
| Cross-meeting inference | 3 | "Why is the SSO launch at risk?" |

Ground-truth answers live in `questions.yml`. Each has an `expected` field (the answer a competent human would give after reading the corpus) and a `rubric` field (what the graded answer must contain).

## What's explicitly out of scope for v0.1

- Running against a user's real meeting corpus (fixture-only).
- Automated CI integration. Manual runs only.
- Scoring beyond human-graded pass/fail. No numeric rubric, no partial credit, no confidence intervals.
- More than three agent targets.
- Adversarial questions designed to elicit hallucination. v0.1 tests competent answers, not robustness.

## What v0.2 will add

- Expansion to 50-100 questions.
- Per-category accuracy breakdown.
- A second fixture corpus in a different domain (the current one is a SaaS startup).
- Optional: automated CI run on every release.
- Optional: run against cloud competitors (Granola, Fireflies) by feeding the fixture transcripts through their APIs. Head-to-head comparison table.

## Contributing

The fixture corpus is fictional and self-contained — no real meetings, no real people. To add a question: append to `questions.yml` with a new unique `id`, an `expected` string, and a `rubric` list of required elements. To add a fixture meeting: create a markdown file in `fixtures/meetings/` matching the Minutes frontmatter schema documented at [useminutes.app/for-agents](https://useminutes.app/for-agents).
