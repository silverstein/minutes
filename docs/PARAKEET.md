# Parakeet Engine Setup

Minutes supports [parakeet.cpp](https://github.com/Frikallo/parakeet.cpp) as an alternative
transcription engine. Parakeet uses NVIDIA's FastConformer architecture and achieves lower
word error rates than Whisper at equivalent model sizes, with dramatically faster inference
on Apple Silicon via Metal GPU acceleration.

## Why Parakeet?

| Engine | Model | Params | LibriSpeech Clean WER | Speed (10s audio, M-series GPU) |
|--------|-------|--------|----------------------|--------------------------------|
| Whisper | small (default) | 244M | 3.4% | ~200ms |
| Whisper | medium | 769M | 2.9% | ~600ms |
| Whisper | large-v3 | 1.55B | 2.4% | ~1.5s |
| **Parakeet** | **tdt-ctc-110m** | **110M** | **2.4%** | **~27ms** |
| **Parakeet** | **tdt-600m** | **600M** | **1.7%** | **~520ms** |

Parakeet's 110M model matches Whisper large-v3 accuracy at 14x fewer parameters.
The 600M model beats everything in its class.

## Fastest Path on Apple Silicon

If you want the shortest path from "I have a Mac" to "Minutes is using
Parakeet locally on Metal," do this:

```bash
# 1. Build and install parakeet.cpp
git clone --recursive https://github.com/Frikallo/parakeet.cpp
cd parakeet.cpp
make build
cp build/bin/parakeet ~/.local/bin/

# 2. Install the multilingual model through Minutes
cd /Users/silverbook/Sites/minutes
minutes setup --parakeet

# 3. Edit ~/.config/minutes/config.toml
[transcription]
engine = "parakeet"
parakeet_model = "tdt-600m"
parakeet_binary = "/Users/you/.local/bin/parakeet"
parakeet_vocab = "tdt-600m.tokenizer.vocab"
```

That gives you the validated multilingual path:
- `tdt-600m`
- local `parakeet.cpp`
- local Metal GPU acceleration on Apple Silicon

If you want the smaller English-only model instead:

```toml
[transcription]
engine = "parakeet"
parakeet_model = "tdt-ctc-110m"
parakeet_binary = "/Users/you/.local/bin/parakeet"
parakeet_vocab = "tdt-ctc-110m.tokenizer.vocab"
```

Minutes will continue to run locally either way.

## Important macOS note for desktop users

If you launch Minutes from Finder, Spotlight, or the Dock, the app may not see
the same `PATH` as your shell.

That means this can work in Terminal:

```bash
which parakeet
```

but the desktop app can still fail with "parakeet binary not found."

For the desktop app, prefer an **absolute path** in `config.toml`:

```toml
[transcription]
parakeet_binary = "/Users/you/.local/bin/parakeet"
```

Common macOS install locations:
- `/opt/homebrew/bin/parakeet`
- `/usr/local/bin/parakeet`
- `/Users/you/.local/bin/parakeet`

## Prerequisites

### macOS (Apple Silicon)

Full Xcode is required for Metal GPU acceleration (the shader compiler is not
included in Command Line Tools).

```bash
# 1. Install Xcode from the App Store (if not already installed)
#    Or: mas install 497799835

# 2. Accept the license
sudo xcodebuild -license accept

# 3. Switch developer directory to Xcode
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer

# 4. Download the Metal Toolchain
xcodebuild -downloadComponent MetalToolchain
```

### Linux / Windows

parakeet.cpp does not yet have CUDA support (WIP in the axiom tensor library).
CPU-only builds work but lose the speed advantage. Monitor the
[parakeet.cpp repo](https://github.com/Frikallo/parakeet.cpp) for CUDA updates.

## Build parakeet.cpp

```bash
# Clone with submodules
git clone --recursive https://github.com/Frikallo/parakeet.cpp
cd parakeet.cpp

# Build (macOS with Metal)
make build

# If CMake 4.x fails with "Neither lock free instructions nor -latomic found",
# patch third_party/axiom/third_party/highway/cmake/FindAtomics.cmake:
# Replace the check_cxx_source_compiles block with an Apple arm64 short-circuit.
# See: https://github.com/google/highway/issues/XXXX

# Install the binary
cp build/bin/parakeet ~/.local/bin/
```

## Install Models

Parakeet models are distributed as `.nemo` files on HuggingFace and must be
converted to safetensors format.

```bash
# Install Python dependencies
pip install safetensors torch torchaudio huggingface_hub

# Option A: Use Minutes setup (recommended)
minutes setup --parakeet                           # Multilingual v3 (tdt-600m, ~1.2 GB)
minutes setup --parakeet --parakeet-model tdt-ctc-110m # English-only compact model (~220 MB)
# Installs native Silero VAD weights automatically

# Option B: Manual download and conversion
hf download nvidia/parakeet-tdt-0.6b-v3 parakeet-tdt-0.6b-v3.nemo --local-dir .
cd parakeet.cpp
mkdir -p ~/.minutes/models/parakeet/tdt-600m
python scripts/convert_nemo.py parakeet-tdt-0.6b-v3.nemo -o ~/.minutes/models/parakeet/tdt-600m/tdt-600m.safetensors --model 600m-tdt

# Also convert Silero VAD weights manually only if you are not using `minutes setup`
python scripts/convert_silero_vad.py -o ~/.minutes/models/parakeet/silero_vad_v5.safetensors

# Extract the SentencePiece tokenizer vocab and store it with a model-specific name
tar xf parakeet-tdt-0.6b-v3.nemo --wildcards --no-anchored '*tokenizer.vocab'
cp *_tokenizer.vocab ~/.minutes/models/parakeet/tdt-600m/tdt-600m.tokenizer.vocab
```

`parakeet.cpp` expects the SentencePiece `tokenizer.vocab` file, not the
plain extracted `vocab.txt`. If you install more than one Parakeet model,
store each model in its own directory and use model-specific filenames such
as `tdt-ctc-110m/tdt-ctc-110m.tokenizer.vocab` and
`tdt-600m/tdt-600m.tokenizer.vocab` so model switches stay deterministic.

## Configure Minutes

### Config file

Edit `~/.config/minutes/config.toml`:

```toml
[transcription]
engine = "parakeet"              # "whisper" (default) or "parakeet"
parakeet_model = "tdt-600m"      # "tdt-ctc-110m" (English) or "tdt-600m" (multilingual v3)
parakeet_binary = "/Users/you/.local/bin/parakeet"  # Prefer an absolute path for desktop app launches
parakeet_boost_limit = 25        # Experimental: top graph-derived boost phrases (0 disables)
parakeet_boost_score = 2.0       # Experimental tuning for parakeet.cpp --boost-score
parakeet_fp16 = true             # Default on macOS Apple Silicon: ~35% faster transcription with lower GPU memory (see docs/designs/parakeet-perf-2026-04-14.md)
parakeet_vocab = "tdt-600m.tokenizer.vocab"  # Safer when multiple Parakeet models are installed
```

### Tauri Desktop App

Settings > Transcription > Engine dropdown. Select "Parakeet", then choose the
model. On macOS, Finder-launched apps may not inherit your shell `PATH`, so
desktop users should usually configure `parakeet_binary` as an absolute path.

## Language Support

| Model | Languages |
|-------|-----------|
| tdt-ctc-110m | English only |
| tdt-600m (v3) | 25 European languages: Bulgarian, Croatian, Czech, Danish, Dutch, English, Estonian, Finnish, French, German, Greek, Hungarian, Italian, Latvian, Lithuanian, Maltese, Polish, Portuguese, Romanian, Russian, Slovak, Slovenian, Spanish, Swedish, Ukrainian |

For languages outside this list, use Whisper (99 languages supported).

## Building Minutes with Parakeet Support

The `parakeet` Cargo feature must be enabled at build time:

```bash
# CLI only
cargo build --release -p minutes-cli --features parakeet

# Tauri desktop app
TAURI_FEATURES="parakeet" cargo tauri build --bundles app

# Or use the build script (add parakeet feature)
cargo build --release -p minutes-cli --features parakeet
```

Note: The `parakeet` feature is opt-in and not included in the default build.
Whisper is always compiled in (it's the default feature). Both engines can coexist
in the same binary — the config file controls which one is used at runtime.

## Switching Back to Whisper

Change `engine = "whisper"` in config.toml, or use the Tauri settings UI.
No rebuild needed — both engines are compiled in when the `parakeet` feature is enabled.

## Troubleshooting

### "parakeet binary not found"
The `parakeet` executable is not in your PATH. Either:
- Add its location to PATH: `export PATH="$PATH:/path/to/parakeet.cpp/build/bin"`
- Or set the full path in config: `parakeet_binary = "/path/to/parakeet"`

On macOS desktop builds, the second option is more reliable because Finder /
Spotlight / Dock launches may not inherit the same shell `PATH` that Terminal
sees.

### "unknown parakeet model"
Only `tdt-ctc-110m` and `tdt-600m` are supported. Check your config.

### "Expected parakeet model in ~/.minutes/models/parakeet/"
Run `minutes setup --parakeet` to install the recommended Parakeet model plus
native VAD weights, or follow the manual download steps above.

### CMake 4.x atomics error (build)
Google Highway's `FindAtomics.cmake` is incompatible with CMake 4.x on Apple Silicon.
The atomics check fails because it forces `CMAKE_CXX_STANDARD 11` which conflicts with
the project's C++20. Workaround: patch the check to short-circuit on `APPLE AND arm64`.

### Metal shader compiler not found (build)
Requires full Xcode (not just Command Line Tools):
```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
xcodebuild -downloadComponent MetalToolchain
```
