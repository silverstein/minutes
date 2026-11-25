# Whole-file Whisper VAD evaluation

Run two Whisper-enabled Minutes CLIs against the same local audio:

```bash
scripts/eval_vad_gating.sh --baseline /path/to/baseline/minutes \
  --candidate /path/to/candidate/minutes --model small --out /tmp/vad-gating
scripts/eval_vad_gating.sh --self-test
```

The evaluator uses Python 3.10 or later and the standard library. It calls `minutes transcribe` with a fresh HOME and config for every invocation. `transcription.engine = "whisper"` selects the whole-file Whisper path. `transcription.model` defaults to `small`, and `transcription.vad_model` is `silero-v6.2.0`. Temporary symlinks let Minutes read the installed Whisper and Silero models in `~/.minutes/models`.

The default corpus uses the dictation and Apple Speech manifests in `tests/eval/fixtures` plus the WAVs in `crates/assets`. The bundled `demo.wav` and the meeting fixture share the same audio and reference. Clipped parity fixtures without reference text still contribute timing and output diagnostics. To use a different corpus, pass `--corpus DIR` with a `corpus.json` array like `[{"id":"clip","audioPath":"clip.wav","referenceText":"Hello world."}]`. Paths can be relative to the manifest. Use mono 16 kHz 16-bit PCM WAVs. An empty reference marks a noise-only file.

The evaluator adds 20 s and 60 s gaps of silence or seeded pink noise at −50 and −40 dBFS RMS, one file with 30 s of silence at each end, and 90 s of pink noise. These files use the first two speech clips, or repeat the sole speech clip, and are written under the output directory.

`report.md` shows per-file and category results. Category WER pools word edits over nonempty references and excludes whisper-guard annotations. Noise-only words, collapse annotations, and repeated runs are counted separately. `results.json` includes each transcript and any lines whose reported start falls inside a known gap. The CLI reports starts in whole seconds, and a line that starts in a gap can include speech from beyond it. Wall times include startup and model loading. A CLI failure stops the comparison with its diagnostic.
