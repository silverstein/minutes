# Apple Speech Benchmark — April 22, 2026

This document records the first real local benchmark run for Minutes' Apple
speech evaluation path.

If you are looking for the **current shipped Apple Speech scope**, see
[`docs/APPLE_SPEECH.md`](../APPLE_SPEECH.md). This benchmark memo is historical
evidence and backend-evaluation context, not the primary product-scope doc.

It is intentionally narrower than a final backend decision memo. The goal of
this run was to answer:

- can Minutes probe Apple speech eligibility honestly on a real Mac?
- can Minutes benchmark `SpeechTranscriber` and `DictationTranscriber` against
  current backends on the same audio?
- what does the first local result suggest, and what does it still not prove?

## What was run

Date:
- April 22, 2026

Machine/runtime:
- macOS `26.3`
- `SpeechTranscriber.isAvailable == true`
- Apple speech assets were initially `supported`, then became `installed`
  during the benchmark run

Corpus:
- `tests/eval/fixtures/apple-speech-corpus.json`
- 2 locally generated TTS clips:
  - `dictation-short`
  - `meeting-longer`

Artifact output used for the recommendation below:
- `~/.minutes/research/apple-speech/2026-04-22T22-00-34Z`

Related first-install run:
- `~/.minutes/research/apple-speech/2026-04-22T18-45-27Z`
- this earlier run is still useful because it captured the transition from
  Apple speech assets being merely `supported` to becoming `installed`

Important limitation:
- this corpus is synthetic TTS speech, not real human meeting audio
- the result is useful for relative backend shape, not for a final product default
- the summary table below captures the first benchmark snapshot; later product
  updates added richer reporting such as punctuation-insensitive WER and
  per-content-type slices in generated benchmark artifacts

## Measured result

Aggregate summary from the run:

| Backend | Cases succeeded | Avg elapsed | Avg first result | Avg WER |
|---|---:|---:|---:|---:|
| `SpeechTranscriber` | 2/2 | 309 ms | 258.0 ms | 11.54% |
| `DictationTranscriber` | 2/2 | 352 ms | 343.5 ms | 32.69% |
| `whisper` | 2/2 | 1,684.5 ms | n/a | 9.62% |
| `parakeet` | 2/2 | 1,498.0 ms | n/a | 9.62% |

Case highlights:

- `dictation-short`
  - `SpeechTranscriber`: 300 ms warm, 7.69% WER
  - `DictationTranscriber`: 262 ms warm, 15.38% WER
  - `whisper`: 1,616 ms, 0.00% WER
  - `parakeet`: 1,461 ms, 0.00% WER

- `meeting-longer`
  - `SpeechTranscriber`: 318 ms warm, 15.38% WER
  - `DictationTranscriber`: 442 ms warm, 50.00% WER
  - `whisper`: 1,753 ms, 19.23% WER
  - `parakeet`: 1,535 ms, 19.23% WER

## What this means

### 1. The evaluation path is worth keeping

This is the most important outcome of the work.

Minutes can now:
- probe Apple speech capability in a read-only way
- install the helper outside the repo
- benchmark Apple transcription against current backends
- write repeatable artifacts under `~/.minutes/research/apple-speech/`

That alone resolves the original question of "should we be evaluating this?"
with a clear yes.

### 2. `SpeechTranscriber` is the only Apple path that looks immediately promising

On this first run:
- it was dramatically faster than Whisper on both clips
- after assets were installed, it stayed dramatically faster than Whisper on both clips
- it emitted usable timestamps
- its WER was competitive on the longer meeting-style sample

It did **not** clearly beat Whisper on quality overall in this tiny corpus, but
it was close enough to justify more investigation.

### 3. `DictationTranscriber` looks more like a compatibility/fallback path than a winner

It worked, but on this run it was:
- slower than `SpeechTranscriber` on the longer clip
- materially worse on accuracy
- especially weak on the meeting-style sample

That matches the product intuition that `DictationTranscriber` may be useful
for dictation-style compatibility or broader device coverage, but not as the
primary Apple backend candidate for Minutes.

### 4. Whisper and Parakeet remain the quality baselines in this first corpus

Whisper and Parakeet tied on WER in this synthetic corpus, while both were much
slower than `SpeechTranscriber`. Parakeet was also modestly faster than Whisper
on elapsed time.

That means this run does **not** justify switching Minutes defaults to Apple
speech.

## Recommendation

The recommendation from the April 22, 2026 run is:

1. **Keep the Apple speech benchmark path.**
   - This is already valuable and answers the strategic question with evidence.

2. **Do not switch Minutes' default backend based on this run.**
   - The corpus is too small and too synthetic.
   - `SpeechTranscriber` is much faster, but Whisper/Parakeet still set the
     quality bar on this corpus.

3. **If Minutes productizes any Apple path next, start with `SpeechTranscriber`, not `DictationTranscriber`.**
   - `SpeechTranscriber` is the only Apple candidate from this run that looks
     both fast and credible enough to merit a real human-audio follow-up.

4. **Run the same benchmark on a real human corpus before making a keep/defer product call.**
   - The next decision-quality run should include:
     - real human dictation audio
     - real far-field meeting audio
     - at least one Apple-ineligible or Apple-limited machine, if available

## Explicit non-recommendations

This run does **not** support:
- replacing Whisper today
- replacing Parakeet today
- claiming Apple speech is "better" in general
- claiming Apple speech is ready for all macOS users without runtime gating

## Command recap

These are the commands that produced the current workflow:

```bash
minutes apple-speech capabilities
minutes apple-speech benchmark --corpus tests/eval/fixtures/apple-speech-corpus.json
```

Those commands are the shipped output of this work, even if the backend
decision itself remains intentionally conservative.
