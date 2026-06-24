# Sherpa Engine Setup

Minutes supports [sherpa-onnx](https://k2-fsa.github.io/sherpa/onnx/index.html)
as the recommended SOTA / multilingual transcription engine. The Sherpa path
runs parakeet-tdt-0.6b-v3 through the Rust `sherpa-rs` crate in-process, without
Python and without a separate sidecar binary.

Whisper.cpp remains the **bundled default** (it ships in every build and works
on every platform with no extra download). Sherpa is **opt-in**: it is compiled
when the `engine-sherpa` Cargo feature is enabled and selected at runtime with
`transcription.engine = "sherpa"`. If you select Sherpa on a build/platform that
does not have it, or before the model is installed, transcription automatically
falls back to Whisper (with a warning) so a recording never breaks.

## Quick Start (recommended)

```bash
# Build with the sherpa engine, then download + enable the model in one step:
cargo build --release -p minutes-cli --features engine-sherpa
rm -f ~/.local/bin/minutes && cp target/release/minutes ~/.local/bin/minutes

minutes setup --sherpa     # downloads the int8 ONNX model + sets engine = "sherpa"
```

`minutes setup --sherpa` downloads the four model files (with a size-floor
integrity check) and writes `transcription.engine = "sherpa"` to your config, so
no manual edit is needed. If the binary lacks the `engine-sherpa` feature, setup
still configures the engine and prints a note that transcription falls back to
Whisper until you rebuild with the feature.

Run `minutes setup --list` to see all engines and recommended models.

The rest of this document covers manual install, configuration overrides, and
build details.

## Why Sherpa?

Sherpa gives Minutes a native, no-Python path to the multilingual
parakeet-tdt-0.6b-v3 family. Unlike the `parakeet.cpp` integration, there is
no external executable to install or keep on `PATH`; the recognizer is loaded
directly by `minutes-core` through `sherpa-rs`.

The bundled engine target is:

| Engine | Runtime | Model | Install shape |
|--------|---------|-------|---------------|
| Whisper | whisper.cpp via `whisper-rs` | small by default | default build |
| Parakeet | external `parakeet.cpp` binary / sidecar | tdt-ctc-110m or tdt-600m | opt-in feature + external binary |
| **Sherpa** | **in-process `sherpa-rs`** | **parakeet-tdt-0.6b-v3 int8** | **opt-in feature + ONNX files** |

## Scope

Today, `engine = "sherpa"` is wired as an opt-in offline transcription engine
behind the `engine-sherpa` feature. The dispatch path loads audio, converts it
to 16 kHz mono samples, and calls the in-process Sherpa recognizer.

If Sherpa support is not compiled into the current build, or the model files
are not yet installed, selecting `engine = "sherpa"` transparently falls back to
Whisper for that recording and logs a warning (with the resolved model directory
and whether the feature was compiled in). Whisper is the bundled default and the
fallback target, so a selected-but-unavailable Sherpa engine never breaks a
recording.

The Sherpa engine is for transcription. Any "4 speaker" language in related
notes refers to diarization work, not to the Sherpa transcription model.

## Install Models

The recommended way is `minutes setup --sherpa` (see Quick Start above), which
downloads the model and enables the engine in one step. The manual steps below
are for custom install locations or air-gapped setups.

Sherpa expects the int8 ONNX export from the HuggingFace repository
`csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`.

The model directory is resolved in this order:

1. `transcription.sherpa_model_dir`
2. `MINUTES_SHERPA_MODEL_DIR`
3. `<model_path>/sherpa/parakeet-tdt-0.6b-v3-int8`

With the default Minutes model path, the default location is:

```text
~/.minutes/models/sherpa/parakeet-tdt-0.6b-v3-int8
```

The directory must contain these files:

```text
encoder.int8.onnx
decoder.int8.onnx
joiner.int8.onnx
tokens.txt
```

Manual download with `curl`:

```bash
MODEL_DIR="$HOME/.minutes/models/sherpa/parakeet-tdt-0.6b-v3-int8"
mkdir -p "$MODEL_DIR"
cd "$MODEL_DIR"

curl -L -O https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/encoder.int8.onnx
curl -L -O https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/decoder.int8.onnx
curl -L -O https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/joiner.int8.onnx
curl -L -O https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/tokens.txt

ls -lh encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt
```

The encoder is about 652 MB, so the download is larger than the other three
files combined.

If you install the model somewhere else, either set the config key:

```toml
[transcription]
sherpa_model_dir = "/absolute/path/to/parakeet-tdt-0.6b-v3-int8"
```

or export the environment override before launching Minutes:

```bash
export MINUTES_SHERPA_MODEL_DIR="/absolute/path/to/parakeet-tdt-0.6b-v3-int8"
```

For desktop app launches, the config key is usually more reliable than an
environment variable because Finder, Spotlight, and Dock launches may not
inherit your shell environment.

## Configure Minutes

`minutes setup --sherpa` sets `transcription.engine = "sherpa"` for you. To
configure it by hand (or to point at a custom model directory), edit
`~/.config/minutes/config.toml`:

```toml
[transcription]
engine = "sherpa"               # "whisper" (default), "parakeet", or "sherpa"
# sherpa_model_dir = "/absolute/path/to/parakeet-tdt-0.6b-v3-int8"
```

Leave `sherpa_model_dir` unset to use the default resolved directory under
`model_path`, or set it when you want to store the ONNX files somewhere else.

## Language Support

The Sherpa engine uses parakeet-tdt-0.6b-v3, the multilingual Parakeet v3
model family. It is intended for multilingual transcription, including English,
French, Spanish, and other supported European languages.

For languages outside the Parakeet v3 coverage, use Whisper.

## Building Minutes with Sherpa Support

The `engine-sherpa` Cargo feature must be enabled at build time:

```bash
# Core library check
cargo check -p minutes-core --features engine-sherpa

# CLI build
cargo build --release -p minutes-cli --features engine-sherpa
```

The feature is opt-in and not included in the default build. Whisper is still
the default feature, so a normal `--features engine-sherpa` build includes both
Whisper and Sherpa unless you also pass `--no-default-features`.

`sherpa-rs-sys` builds sherpa-onnx through CMake, so a working CMake toolchain
must be available during the build.

## Switching Back to Whisper

Change `engine = "whisper"` in config.toml. No model deletion is required.

## Limitations

- The int8 encoder is about 652 MB, so the Sherpa model install is not small.
- Sherpa preserves spoken disfluencies such as "uh" and "um" more than the
  current Whisper path. That can be useful for faithful transcripts, but it may
  read less polished in meeting notes.
- Decode-hint biasing is not wired into the Sherpa dispatch path yet.
- Sherpa is a transcription engine. Diarization remains separate.

## Troubleshooting

### "engine-sherpa" is unavailable

The binary was built without the `engine-sherpa` feature. Rebuild with:

```bash
cargo build -p minutes-cli --features engine-sherpa
```

### "sherpa model not found"

The resolved model directory does not contain all four required files. Check
the configured path and the default path:

```bash
ls -lh ~/.minutes/models/sherpa/parakeet-tdt-0.6b-v3-int8
```

If you store the files elsewhere, set `transcription.sherpa_model_dir` or
`MINUTES_SHERPA_MODEL_DIR`.

### CMake is missing

`sherpa-rs-sys` needs CMake while compiling sherpa-onnx. Install CMake through
your platform package manager or runner image before building with
`--features engine-sherpa`.

### "vocab_size does not exist in the metadata"

The current engine code sets `model_type = ""` so sherpa auto-detects the NeMo
Parakeet-TDT loader. If a future `sherpa-rs` change or local patch forces the
generic `"transducer"` model type, model loading can fail with a metadata error
like this. Keep `model_type` empty for this model family.
