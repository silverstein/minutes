# Diarization model sets

Minutes runs diarization locally through ONNX Runtime and the existing
pyannote-rs-compatible pipeline. The model set is selected in configuration:

```toml
[diarization]
engine = "pyannote-rs"
model = "legacy"
```

`legacy` remains the default and uses `segmentation-3.0.onnx` with CAM++ speaker
embeddings. Existing configurations and recordings therefore keep the same
model files and inference path unless the user opts in.

`community-1` uses the community-1 segmentation export and its WeSpeaker
ResNet34 embedding export. Install and select it explicitly:

```console
minutes setup --diarization --model community-1
```

```toml
[diarization]
engine = "pyannote-rs"
model = "community-1"
```

The setup command downloads the model pair; it does not save this selection.
Update the existing `[diarization]` table in your configuration rather than adding
a duplicate table. This is separate from `[transcription] model`, which selects
speech recognition. A running desktop app needs to reload its startup settings;
restart it when no recording is active before testing the new selection.

The community-1 model is licensed under CC BY 4.0. Distributions and derived
uses must retain attribution to pyannote and the model authors. The ONNX export
used by Minutes is published in the
[`altunenes/speaker-diarization-community-1-onnx`](https://huggingface.co/altunenes/speaker-diarization-community-1-onnx)
repository and points back to the original
[`pyannote/speaker-diarization-community-1`](https://huggingface.co/pyannote/speaker-diarization-community-1)
model card.

The exports keep the 16 kHz, 10-second segmentation window and seven powerset
classes used by the legacy path, but rename the segmentation tensors. The
community embedding also renames its tensors and expects filter-bank input on
the original 16-bit PCM scale. Minutes adapts those contracts without Python.
The upstream pyannote.audio-only `exclusive_speaker_diarization` reconciliation
is not part of these two ONNX exports, so Minutes continues to use its own
segment clustering and transcript reconciliation.

Voice profiles are isolated by embedding model id in `voices.db`. Community-1
embeddings are never silently compared with profiles enrolled using CAM++;
enroll a new profile under the active model set if voice matching is needed.
