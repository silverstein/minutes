# Paired offline diarization evaluation

This measures anonymous speaker activity on specified files. It does not run
ASR, enroll people, use the microphone, play audio, upload data, or qualify
native capture or the desktop UI. `UNKNOWN` is abstention and cannot become a
reference identity through optimal label mapping.

The checked-in manifest pins two complete human AMI meetings and their RTTMs.
Attribution: **AMI Meeting Corpus, AMI Project**. Audio and annotations are
[CC-BY-4.0](https://groups.inf.ed.ac.uk/ami/). RTTM conversion:
[pyannote/AMI-diarization-setup](https://github.com/pyannote/AMI-diarization-setup),
commit `67c2d539286e89f68952d5dcf83912bd9f01dfae`, `only_words` test split.
Neither recording was used to tune this implementation. AMI is a common model
benchmark; this is not a claim of model-training independence or blinding.

Downloads are explicit. Put these four files in an owned directory:

- [EN2002a.Array1-01.wav](https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/EN2002a/audio/EN2002a.Array1-01.wav)
- [EN2002a.rttm](https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/67c2d539286e89f68952d5dcf83912bd9f01dfae/only_words/rttms/test/EN2002a.rttm)
- [EN2002b.Array1-01.wav](https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/EN2002b/audio/EN2002b.Array1-01.wav)
- [EN2002b.rttm](https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/67c2d539286e89f68952d5dcf83912bd9f01dfae/only_words/rttms/test/EN2002b.rttm)

The runner verifies every audio and reference SHA256 before either executable
runs. Models must already be installed in the supplied public-model directory.
Build the same `crates/core/examples/diarize_file.rs` harness at baseline and
candidate revisions:

```bash
cargo build -p minutes-core --no-default-features --features diarize --example diarize_file
```

For a historical baseline that predates the example, copy only that harness
into its own checkout before building. Preserve the baseline source revision,
the exact harness, and the resulting executable. Do not replace either binary
during evaluation. The report hashes both executables and both model files.

```bash
python3 tests/eval/diarization/run.py \
  --manifest tests/eval/diarization/ami-holdout.json \
  --audio-directory /owned/ami \
  --models /owned/models \
  --baseline /owned/baseline-diarize-file \
  --candidate /owned/candidate-diarize-file \
  --baseline-revision BASELINE_SHA \
  --candidate-revision CANDIDATE_SHA \
  --output /owned/results
```

The manifest declares the gate before candidate evaluation: weighted DER must
improve at least five percentage points over the clock-fixed baseline, no file
may regress more than three percentage points, and candidate RTF must remain
below 0.25. A failing gate returns exit code 2. DER uses one optimal mapping over
each complete recording, zero collar, and includes overlap. Runtime is measured
by the harness, not read from a candidate's self-report. Reports include misses,
false alarms, confusion, UNKNOWN duration, and speaker counts. Local stderr
stays in separate files; inspect it locally before sharing.

Use `python3 -m unittest discover -s tests/eval/diarization -p 'test_*.py'`
for scoring and abstention regressions. See
[the research and admission decision](../../../docs/investigations/diarization-quality-2026-10-18.md)
for scope, development fixtures, model compatibility, and what these results
cannot establish. Do not tune on this holdout and continue calling it untouched.

The earlier IS1009a/TS3003a admission is retained in `ami-first-admission.json`.
Its first candidate missed the unchanged five-point improvement gate (4.38 points).
Those recordings are now development data, not an untouched holdout. The current
global clustering candidate was frozen before evaluating EN2002a/EN2002b. Both
new recordings share speakers; this does not establish broad population quality.

## Full native pipeline comparison

`full-native-ami.json` freezes a second experiment against released 0.30.4:
the unmodified `speakrs` CPU example at `af8013f`, with its explicit local
safetensors/PLDA bundle at `67f963b`. It uses three distinct AMI development
groups (IS1008a, TS3004a, IB4001), including an unstructured meeting. These are
fresh to this comparison, not a claim of never-before-project scoring or model
training independence. The old PR71 benchmark omitted source filenames, so its
possible overlap cannot be ruled out. No thresholds or speaker counts are tuned
from this experiment's predictions.

Build both offline probes with Rust 1.95.0 and optimized release profiles.
Build the pinned upstream example with `--no-default-features --features cpu`
so its automatic download feature is absent. Keep it outside the Minutes
dependency graph. Models are downloaded explicitly; their converted card points
to upstream terms but has no independent license declaration. This is evaluation
only, not authorization to redistribute models or reuse voice profiles.

```bash
python3 tests/eval/diarization/fetch_full_pipeline.py \
  --manifest tests/eval/diarization/full-native-ami.json --directory /owned/full-native
python3 tests/eval/diarization/full_pipeline.py \
  --manifest tests/eval/diarization/full-native-ami.json \
  --audio-directory /owned/full-native/audio \
  --baseline /owned/minutes/target/release/examples/diarize_file \
  --candidate /owned/speakrs/target/release/examples/diarize_wav \
  --baseline-models /owned/full-native/baseline-models \
  --candidate-models /owned/full-native/candidate-models \
  --output /owned/new-results
```

This runner currently supports POSIX hosts. It bounds each admitted file to one
hour and each worker to 15 minutes, gives it a separate process group, removes
provider environment variables, applies a kernel output-file size limit, and
kills the group on cancellation or sampled RSS excess. The 4GiB RSS watchdog is
sampled, not a kernel memory reservation; `wait4` separately reports the actual
child peak. Thread environment settings are hints, not an equal-work guarantee.
Raw predictions/errors remain local; engine/model/input hashes bind the report.

The gate retains a five-point weighted DER gain, at most three points of
per-recording regression and RTF at most0.25. It also requires speaker count
within one of reference on each file, no count-error regression, and peak RSS
at most4GiB. A pass only qualifies this comparison. Speaker-attributed word
error, multilingual/short-turn coverage, named profiles, Intel/Windows native
behavior, concurrent live latency and installed capture remain separate gates.

See the [historical rationale and live-path audit](../../../docs/investigations/diarization-history-and-live-latency-2026-10-18.md)
before treating every merge as a bug or moving refinement into the live loop.

### Oracle-word attribution diagnostic

This is a separate descriptive follow-up, frozen at `047195d2` after model DER
was available and before word scoring. It does not change the original gates.
It uses official manual words/timings for both engines, isolating attribution
from recognition. This is **not end-to-end transcription WER**. Exact known
word intervals use MeetEval's `full_segment` timing on both sides and a 0.5s
hypothesis collar for tcpWER. cpWER permits global speaker permutation; tcpWER
also restricts temporal matches. See [MeetEval's definition](https://github.com/fgnt/meeteval/blob/v0.4.3/doc/tcpwer.md).

Exactly one unique predicted identity intersecting the word interval gets that
word. Missing activity, any `UNKNOWN`, or multiple identities abstain. Abstained
words remain in reference and incur attribution deletions; the app's text is
not deleted. No minimum coverage fraction is required. The check deliberately
does not resolve overlapping speakers by consulting reference identities.
Invalid or zero-duration XML word entries are excluded and counted. Fillers
remain. The policy pins every annotation hash, timing and normalization rule.

```bash
python3 tests/eval/diarization/fetch_words.py \
  --policy tests/eval/diarization/oracle-word-policy.json --directory /owned/full-native
python3 -m venv /owned/word-metrics-venv
/owned/word-metrics-venv/bin/pip install 'meeteval==0.4.3'
/owned/word-metrics-venv/bin/python -m unittest discover \
  -s tests/eval/diarization -p 'test_*.py'
/owned/word-metrics-venv/bin/python tests/eval/diarization/word_attribution.py \
  --manifest tests/eval/diarization/full-native-ami.json \
  --word-policy tests/eval/diarization/oracle-word-policy.json \
  --annotations /owned/full-native/word-annotations \
  --predictions /owned/new-results --output /owned/new-word-report.json
```

MeetEval builds native extensions; use an adequately provisioned isolated
environment. CI without this optional dependency skips its two conformance
tests; the recorded Mac run executed all 17 tests with MeetEval 0.4.3 installed.
The [native comparison report](../../../docs/investigations/full-native-diarization-2026-10-18.md)
explains the measured gains, limitations and remaining integration gates.

### Actual host-to-worker regression

The separate `minutes-speaker-worker` helper and `full_diarize_file` example
exercise the opt-in Minutes dispatcher, PCM transport, resource supervision and
anonymous response validation. Follow the [build and setup instructions](../../../crates/speaker-worker/README.md)
for both executables; an older installed host cannot use a newly built helper.

```bash
python3 tests/eval/diarization/check_full_worker.py \
  --probe /owned/minutes/target/release/examples/full_diarize_file \
  --worker /owned/minutes/crates/speaker-worker/target/release/minutes-speaker-worker \
  --fixtures /owned/full-native --output /owned/worker-results
```

On Windows, supply both executable paths with `.exe`. The check uses the three
already-seen AMI recordings and frozen native predictions above. It requires
each file's DER to remain within one percentage point of that result, exactly
four anonymous speakers, RTF at most 0.25 and unchanged input WAV hashes. On
POSIX it also kills its own synthetic host after worker launch and checks that
the actual helper retires. That check does not instrument inference phase or
qualify Windows parent loss. Reports bind the executable, source, input and
checker hashes. The `Full speaker worker` CI workflow runs the actual bridge
on Linux, Windows, Apple Silicon and Intel. This is integration regression,
not a fresh model admission, live capture test or installed-app acceptance.
