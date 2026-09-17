# Sherpa Engine Setup

Minutes supports [sherpa-onnx](https://k2-fsa.github.io/sherpa/onnx/index.html)
as the recommended SOTA / multilingual transcription engine. The Sherpa path
runs parakeet-tdt-0.6b-v3 through the Rust `sherpa-rs` crate in-process, without
Python and without a separate sidecar binary.

`transcription.engine = "auto"` is the compiled default. On Apple Silicon it
selects sherpa when the build includes `engine-sherpa`, the sherpa plugin is
present, and the Parakeet v3 model is installed; otherwise it selects Whisper.
Explicit `"whisper"` and `"sherpa"` values keep their meaning, and unavailable
sherpa requests still fall back to Whisper so a recording never breaks.

## Which install gets Parakeet by default

- The signed desktop app does: the plugin is bundled and signed inside the app.
- `minutes-macos-arm64-sherpa.tar.gz` does: keep its Developer ID-signed plugin
  beside the binary.
- The bare `minutes-macos-arm64` asset, `cargo install`, and the Homebrew CLI
  formula do not. They use Whisper until a compatible plugin is placed in a
  loader search path.

The MCP server's Apple Silicon auto-install downloads and checksum-verifies the
sherpa archive, verifies the staged CLI with `--version`, and then atomically
replaces the binary and plugin. It falls back to the bare binary for older
releases that do not publish the archive.

## Quick Start (recommended)

```bash
# Build the isolated sherpa plugin, then the CLI, then install the model.
# The plugin is a separate dynamic library on purpose (see below); the CLI
# dlopens it at runtime.
(cd crates/sherpa-plugin && cargo build --release)
cargo build --release -p minutes-cli --features engine-sherpa
rm -f ~/.local/bin/minutes && cp target/release/minutes ~/.local/bin/minutes
```

Install the plugin **beside the `minutes` binary**. That is where release
archives and the macOS app carry it, and it is the path the loader checks
before any other on-disk location. Substitute your own install directory if it
is not `~/.local/bin` (a `cargo install` layout puts the binary in
`~/.cargo/bin`).

Installing differs by platform, because only macOS links sherpa-onnx
statically into the plugin:

```bash
# macOS: the plugin is self-contained.
cp crates/sherpa-plugin/target/release/libminutes_sherpa.dylib ~/.local/bin/

# Linux: the sherpa libraries must travel with it, since the plugin resolves
# them through its own $ORIGIN. Copying the plugin alone installs one that
# cannot load.
cp crates/sherpa-plugin/target/release/*.so ~/.local/bin/
```

```bash
minutes setup --sherpa     # downloads the int8 ONNX model + sets engine = "sherpa"
```

> **Why sherpa is a plugin (issues #683, #685).** sherpa-onnx bundles its own
> ONNX Runtime (1.17.1) and its own kaldi-native-fbank; pyannote, used for
> diarization, brings ONNX Runtime 1.22.0 and a different kaldi-native-fbank
> under the same archive names. One executable holds one definition per symbol,
> so statically linking both breaks whichever consumer loses: voice enrollment
> fails with an ORT API 17/22 mismatch, or aborts inside feature extraction, or
> sherpa itself aborts. There is no link order that works.
>
> Loading sherpa from its own dynamic library removes the conflict instead of
> arbitrating it. A Rust cdylib exports only its own `#[no_mangle]` surface, so
> the embedded runtime stays internal and macOS two-level namespaces bind each
> image's calls to its own copy. Verified on arm64: one binary enrolls a voice
> through ORT 1.22 and transcribes through the plugin's ORT 1.17, in one
> process.
>
> The plugin is found via `MINUTES_SHERPA_PLUGIN`, then beside the running
> executable, then in `<transcription.model_path>/sherpa/lib/`, which with the
> default model path is `~/.minutes/models/sherpa/lib/`. If it is missing or
> reports a different ABI version, transcription falls back to Whisper with a
> warning naming every path tried, exactly like a missing model.
>
> Note that `~/.minutes/lib/` is **not** one of these paths. It exists, but it
> belongs to the Apple Foundation Models helper. Earlier revisions of this page
> and of `docs/install.md` sent the plugin there, where nothing ever looks for
> it, and `minutes health` then reports it as not installed. Reported by
> @ReticentEclectic in #998.

> **Platform note.** On macOS, the plugin links sherpa-onnx statically, so it is
> self-contained and the copy above is all you need.
>
> On Linux, sherpa-rs links dynamic libraries (`libsherpa-onnx-c-api.so`,
> `libonnxruntime.so`) that are built into the plugin crate's `target/release`.
> The plugin carries an `$ORIGIN` runpath, so it finds them in whatever
> directory it is installed to, but they have to travel with it. Copy the whole
> set, not just the plugin:
>
> ```bash
> cp crates/sherpa-plugin/target/release/*.so ~/.local/bin/
> ```
>
> The binary itself no longer links sherpa at all, so the plugin must remain
> beside it. The released Linux and macOS sherpa archives ship exactly this
> layout, which is one of the paths the loader searches.
>
> Windows resolves a DLL's dependencies from the loading process's search path
> rather than the DLL's own directory, so the same trick does not apply there
> and it needs its own packaging. That is tracked in #645.

On Apple Silicon builds with `engine-sherpa` and a plugin present, plain
`minutes setup` downloads the four model files plus Whisper tiny for live and
dictation fallback. Without the plugin it installs Whisper small and explains
which packaged install includes Parakeet; setup does not download the plugin.
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
| Whisper | whisper.cpp via `whisper-rs` | small by default; tiny alongside Apple Silicon auto | every build |
| Parakeet | external `parakeet.cpp` binary / sidecar | tdt-ctc-110m or tdt-600m | opt-in feature + external binary |
| **Sherpa** | **in-process `sherpa-rs`** | **parakeet-tdt-0.6b-v3 int8** | **opt-in feature + ONNX files** |

## Scope

Today, `engine = "sherpa"` is wired as an opt-in offline transcription engine
behind the `engine-sherpa` feature. The dispatch path loads audio, converts it
to 16 kHz mono samples, and calls the in-process Sherpa recognizer.

If Sherpa support is not compiled into the current build, or the model files
are not yet installed, selecting `engine = "sherpa"` transparently falls back to
Whisper for that recording and logs a warning (with the resolved model directory
and whether the feature was compiled in). Whisper is the universal fallback and the
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
engine = "sherpa"               # default is "auto"; explicit "whisper" and "sherpa" are supported
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
cargo check -p minutes-core --no-default-features --features engine-sherpa

# CLI build, keeping the default features. The plugin has to be built too:
# `engine-sherpa` compiles only the loader.
cargo build --release -p minutes-cli --features engine-sherpa,metal
(cd crates/sherpa-plugin && cargo build --release)
```

The feature is opt-in for source builds and enabled in macOS arm64 release
builds.

`engine-sherpa` and `diarize` coexist on every platform since #685, so there is
no longer any reason to drop the default features. Before that, both stacks
were statically linked into one macOS image and their two ONNX Runtime copies
collided, so this document told you to build with `--no-default-features` and
give up diarization. Now sherpa lives in its own dlopened image with its own
runtime, and CI compiles `diarize,engine-sherpa` together on purpose to keep it
that way.

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
# macOS: the isolated plugin keeps sherpa's ONNX Runtime separate from
# diarization, so the ordinary defaults can remain enabled:
cargo build -p minutes-cli --features engine-sherpa,metal
(cd crates/sherpa-plugin && cargo build --release)
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
