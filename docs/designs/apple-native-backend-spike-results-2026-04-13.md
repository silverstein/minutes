# Apple-Native Backend Spike Results

Date: 2026-04-13

Related epic: `minutes-tjg3`
Related bead: `minutes-tjg3.7`

## Question

Now that Minutes has a coordinator-shaped transcription runtime, should we immediately push into a true macOS-native backend, or should we keep the current helper-backed Parakeet path and defer that jump?

## Setup

Environment:
- macOS on Apple Silicon
- local `parakeet` binary: `/Users/you/.local/bin/parakeet`
- model: `tdt-600m`
- tokenizer: `tdt-600m.tokenizer.vocab`

Sample:
- 20 second speech-heavy clip extracted from a real recovered meeting
- source clip: `job-20260413141517678-824-0.wav`

Measurement surfaces used:
- `minutes parakeet-benchmark`
- repeated `minutes parakeet-helper` runs
- `/usr/bin/time -l` for process-level memory

## Results

### 1. Helper-backed baseline latency

`minutes parakeet-benchmark` on the same 20 second sample produced:

```json
{
  "backendId": "parakeet",
  "model": "tdt-600m",
  "gpu": true,
  "directElapsedMs": 1791,
  "directSegments": 10,
  "helperElapsedMs": 1310,
  "helperSegments": 10
}
```

Interpretation:
- the current path is already fast enough to feel acceptable on Apple Silicon
- helper vs direct numbers are noisy and should not be overfit
- the helper path is not obviously disqualified on raw latency alone

### 2. Transcript and timestamp stability

Repeated helper runs on the exact same clip produced:
- identical transcript text
- identical segment counts
- identical segment payloads

Observed check:

```text
run1_eq_run2_transcript True
run1_eq_run2_segments True
run1_eq_run3_transcript True
run1_eq_run3_segments True
```

Interpretation:
- current helper-backed output is stable enough to act as the comparison baseline
- timestamp/segment drift is not the immediate reason to replace it

### 3. Memory behavior

Repeated helper runs with `/usr/bin/time -l` produced roughly:
- run 1 real time: `1.79s`
- run 2 real time: `1.37s`
- run 3 real time: `1.43s`
- maximum resident set size: about `5.1 GB` on all three runs

Interpretation:
- the current subprocess path does not appear to gain a large warm-memory advantage across repeated runs
- memory usage remains high and fairly flat run-to-run
- this supports the architectural argument for a future in-process backend more than it supports an immediate rewrite

### 4. Power / energy signal

We captured a privileged `powermetrics` sample during the same helper-backed
Parakeet run on Apple Silicon.

Observed sample:

```text
CPU Power: 5216 mW
GPU Power: 2648 mW
ANE Power: 0 mW
Combined Power (CPU + GPU + ANE): 7864 mW
```

Interpretation:
- the current helper-backed path is using CPU + GPU
- it is **not** using the Apple Neural Engine today
- that strengthens the architectural case for a future native Apple backend,
  because an Apple-native path could plausibly shift work toward different
  runtime characteristics than the current subprocess + Metal setup

Important caution:
- this is one point sample, not a full power study
- it is enough to establish “current path uses CPU/GPU, not ANE”
- it is not enough to claim a future native backend will automatically be more
  energy efficient without measuring that backend too

## Comparison against Muesli-style Apple-native ideas

Muesli still looks ahead of Minutes in one important area:
- it treats native Apple execution as a first-class backend, not as an external subprocess

That matters because a native backend could plausibly improve:
- preload persistence
- warmup semantics
- runtime ownership
- backend-specific instrumentation
- possibly latency and energy efficiency

But the current spike results do **not** show a crisis that forces an immediate jump.

What they show is:
- current helper-backed Parakeet is stable
- current helper-backed Parakeet is already reasonably fast
- current helper-backed Parakeet still lacks a convincing warm-state benefit
- the main justification for a native backend is architectural cleanliness and potentially better long-lived runtime behavior, not a dramatic current-user performance emergency

## Decision

Recommendation: **defer full native-backend implementation for now**

Keep:
- the current helper-backed Parakeet path
- the new coordinator as the runtime owner

Do next:
- keep measuring through the coordinator contracts
- only pursue a true macOS-native backend when we can do it as a contained backend implementation, not as a repo-wide rewrite
- treat the current power sample as the baseline that any future native backend
  has to beat or justify

## Why this is the right call

The current helper-backed backend is no longer the weak point it used to be.

What is still missing is not “a faster number at all costs.” It is:
- real persistent warm-state semantics
- cleaner runtime ownership
- a better long-term backend boundary

The coordinator work already moved Minutes toward that future. The next Apple-native step should happen when we can evaluate a real backend candidate behind that boundary, not when the current path is already working well enough for users.

## Bottom line

If we had seen unstable timestamps, terrible latency, or obviously bad user-facing performance, the answer would be “push ahead now.”

We did not.

The honest answer from this spike is:
- keep the current helper-backed path
- treat the native backend as a measured future backend experiment
- do not confuse architectural cleanliness with an urgent product fire
