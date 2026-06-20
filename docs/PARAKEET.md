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

## Scope

Today, `engine = "parakeet"` is wired for these paths:

- post-recording batch transcription (`minutes process`, desktop processing, and the shared cleanup pipeline)
- folder watcher memo processing after a file lands on disk
- recording-sidecar live transcription during `minutes record`
- standalone live transcription (`minutes live` and desktop Live Mode) — see RFC 0002

Both live paths route each VAD-gated utterance through the Parakeet path. If
the sidecar is effective (auto-on when `example-server` resolves, or forced with `parakeet_sidecar_enabled = true`), they reuse the warm `example-server` socket;
otherwise they fall back to the Parakeet subprocess path for each utterance.
The standalone live path additionally warms the sidecar at session start so the
first utterance does not pay the subprocess-spawn + model-load cost.

Parakeet also participates in the experimental Apple Speech standalone-live
path as the **first runtime fallback**. If `engine = "apple-speech"` is set for
`minutes live` and Apple Speech cannot run or fails mid-session, Minutes tries
a ready Parakeet backend before falling back to Whisper. Apple Speech itself is
still configured separately and remains standalone-live-only; this note is just
about the fallback order behind that path. See [`docs/APPLE_SPEECH.md`](APPLE_SPEECH.md)
for the current Apple Speech scope and desktop-settings limitation.

Strongly recommended for live use: install `example-server` (the sidecar then auto-enables; `parakeet_sidecar_enabled = true` forces it) and
ensure `example-server` is discoverable (either on `PATH` or via
`MINUTES_PARAKEET_SERVER_BINARY`). Without the warm sidecar, every live
utterance incurs full subprocess startup, which makes live mode visibly slow.

Dictation remains Whisper by default because its overlay depends on fast
mid-utterance partials. You can opt into Parakeet for final utterance
transcription with:

```toml
[dictation]
backend = "parakeet"
```

In that mode, Whisper still powers progressive partial text while Parakeet is
used at VAD-finalization when the installed/compiled backend is ready. If
Parakeet is unavailable or fails for an utterance, dictation falls back to
Whisper for that utterance.

If Parakeet support is not compiled into the current build, Minutes logs a
warning and falls back to Whisper for live and dictation paths.

Note: both live paths still require the `whisper` Cargo feature to be compiled
in. Whisper is the runtime fallback when Parakeet fails mid-session (warmup
error, sidecar unreachable, transcription failure), so builds with
`--features parakeet` and `--no-default-features` (no whisper) cannot run
`minutes live` — the session errors out immediately. Whisper is a default
feature, so this only matters for unusual build configurations.

## Fastest Path on Apple Silicon

If you want the shortest path from "I have a Mac" to "Minutes is using
Parakeet locally on Metal," do this. Steps 1–3 are shell commands; step 4
is a manual download + conversion (the CLI prints these instructions but
does not run them automatically); step 5 writes a config file — do not paste
the TOML block into the shell.

Prerequisites first (see [Prerequisites](#prerequisites) for details):
- Full Xcode installed (Command Line Tools alone is not enough — Metal needs
  the full Xcode shader compiler)
- CMake 3.31.x. CMake 4.x trips an atomics check in parakeet.cpp's bundled
  Google Highway dependency on Apple Silicon. Homebrew no longer ships
  `cmake@3`, so grab the official Kitware universal tarball:

  ```bash
  mkdir -p ~/.local/opt && cd ~/.local/opt
  curl -L -O https://github.com/Kitware/CMake/releases/download/v3.31.12/cmake-3.31.12-macos-universal.tar.gz
  tar xf cmake-3.31.12-macos-universal.tar.gz
  export PATH="$HOME/.local/opt/cmake-3.31.12-macos-universal/CMake.app/Contents/bin:$PATH"
  cmake --version    # expect 3.31.12
  ```

  Only export this `PATH` in the build shell — your system CMake stays
  untouched everywhere else. The separate `install(EXPORT) ... AxiomTargets`
  error is handled by build flags rather than CMake version; see step 1
  below and [Troubleshooting](#troubleshooting).

```bash
# 1. Build and install parakeet.cpp — clone OUTSIDE the Minutes repo
mkdir -p ~/src && cd ~/src
git clone --recursive https://github.com/Frikallo/parakeet.cpp
cd parakeet.cpp

# Configure with the maintainer's two opt-out flags (AXIOM_INSTALL=OFF,
# PARAKEET_INSTALL=OFF). They short-circuit the install(EXPORT) export-set
# strictness error and work on any CMake version. PARAKEET_BUILD_SERVER_EXAMPLE
# turns on the warm-sidecar binary that live mode reuses across utterances.
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF \
  -DPARAKEET_INSTALL=OFF \
  -DPARAKEET_BUILD_SERVER_EXAMPLE=ON \
  -DAXIOM_BUILD_TESTS=OFF \
  -DAXIOM_BUILD_EXAMPLES=OFF
cmake --build build -j

# Install both binaries. Locations differ by generator: Ninja puts them in
# build/bin/, Unix Makefiles (the default when ninja isn't installed) puts
# parakeet in build/ and example-server in build/examples/server/. The find
# expression handles either layout.
mkdir -p ~/.local/bin
find build -type f -perm -u+x \( -name parakeet -o -name example-server \) \
  -exec cp {} ~/.local/bin/ \;
ls -lh ~/.local/bin/parakeet ~/.local/bin/example-server

# Sanity check: both binaries should link Metal, MPS, MPSGraph, and Accelerate.
otool -L ~/.local/bin/parakeet | grep -E 'Metal|Accelerate'

# 2. Build the Minutes CLI WITH the parakeet feature, then install it
cd <path/to/your/minutes/checkout>     # e.g. ~/Sites/minutes
cargo build --release -p minutes-cli --features parakeet
mkdir -p ~/.local/bin
rm -f ~/.local/bin/minutes && cp target/release/minutes ~/.local/bin/minutes
# Make sure ~/.local/bin is on PATH (add to ~/.zshrc if it isn't):
#   export PATH="$HOME/.local/bin:$PATH"

# 3. Install Silero VAD weights + sanity-check the parakeet binary on PATH.
#    `minutes setup --parakeet` only installs the bundled VAD weights and
#    resolves your parakeet binary location. It does NOT download or convert
#    the .nemo model — that's step 4. The brew formula CLI (whisper-only)
#    can run this command; it prints a feature-flag warning but the setup
#    itself completes correctly.
minutes setup --parakeet
```

4. Download and convert the multilingual `tdt-600m` model. This is the
   manual step the CLI prints instructions for. The `.nemo` file is publicly
   curl-able from HuggingFace — no `huggingface_hub` dependency needed — and
   conversion needs a small Python venv with `torch`, `safetensors`,
   `packaging`, and `numpy`. (`packaging` is a transitive dep `safetensors`
   doesn't pull on its own; without it `convert_nemo.py` crashes at import
   time.)

```bash
# 4a. Python venv for the conversion script (uv is fastest; python -m venv
#     works too)
uv venv ~/.local/venvs/parakeet-convert --python 3.13
uv pip install --python ~/.local/venvs/parakeet-convert/bin/python \
  torch safetensors packaging numpy
source ~/.local/venvs/parakeet-convert/bin/activate

# 4b. Download the .nemo (2.4 GB). HF transparently redirects to the xet-hub
#     CDN; no auth required.
mkdir -p /tmp/parakeet-dl && cd /tmp/parakeet-dl
curl -L -O https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo

# 4c. Convert to safetensors (2.3 GB output, 627M params)
mkdir -p ~/.minutes/models/parakeet/tdt-600m
python ~/src/parakeet.cpp/scripts/convert_nemo.py \
  parakeet-tdt-0.6b-v3.nemo \
  -o ~/.minutes/models/parakeet/tdt-600m/tdt-600m.safetensors \
  --model 600m-tdt

# 4d. Extract the SentencePiece tokenizer.vocab. macOS BSD tar has no
#     --wildcards flag; list the archive, then extract by the literal
#     UUID-prefixed filename. (GNU tar users can keep their --wildcards form.)
tar tf parakeet-tdt-0.6b-v3.nemo | grep tokenizer.vocab
#   -> something like 8f3c5b6e-..._tokenizer.vocab
tar xf parakeet-tdt-0.6b-v3.nemo <paste-the-uuid-filename-from-above>
cp *_tokenizer.vocab ~/.minutes/models/parakeet/tdt-600m/tdt-600m.tokenizer.vocab

# 4e. Free the 2.4 GB download (the safetensors file is what runtime needs)
rm -rf /tmp/parakeet-dl
```

5. Edit `~/.config/minutes/config.toml` so it contains the following.
   The block goes **inside the file**, not into the shell:

```toml
[transcription]
engine = "parakeet"
parakeet_model = "tdt-600m"
parakeet_binary = "/Users/<you>/.local/bin/parakeet"
parakeet_vocab = "tdt-600m.tokenizer.vocab"
# The warm example-server sidecar auto-enables when step 1's example-server
# copy resolves. No key needed; set parakeet_sidecar_enabled = true/false
# only to force it on or off.
parakeet_fp16 = true
```

That gives you the validated multilingual path:
- `tdt-600m`
- local `parakeet.cpp`
- local Metal GPU acceleration on Apple Silicon

If you want the smaller English-only model instead, set
`parakeet_model = "tdt-ctc-110m"` and
`parakeet_vocab = "tdt-ctc-110m.tokenizer.vocab"` in the same file.

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

You'll also need CMake 3.31.x for the parakeet.cpp build (Homebrew has dropped
the `cmake@3` formula). Use Kitware's official universal tarball:

```bash
mkdir -p ~/.local/opt && cd ~/.local/opt
curl -L -O https://github.com/Kitware/CMake/releases/download/v3.31.12/cmake-3.31.12-macos-universal.tar.gz
tar xf cmake-3.31.12-macos-universal.tar.gz
# Use only when building parakeet.cpp; don't add to your shell profile.
export PATH="$HOME/.local/opt/cmake-3.31.12-macos-universal/CMake.app/Contents/bin:$PATH"
```

### Linux / Windows (parakeet.cpp, CPU only)

parakeet.cpp does not yet have CUDA support (WIP in the axiom tensor library).
CPU-only builds work but lose the speed advantage. Monitor the
[parakeet.cpp repo](https://github.com/Frikallo/parakeet.cpp) for CUDA updates.

### Linux with an NVIDIA GPU (NeMo wrapper, CUDA)

If you have an NVIDIA GPU on Linux, NVIDIA's [NeMo toolkit](https://github.com/NVIDIA/NeMo)
supports Parakeet natively with full CUDA acceleration. The `parakeet_binary`
config key accepts any executable that follows the parakeet.cpp CLI contract,
so you can point it at a small Python wrapper around NeMo and get GPU-backed
transcription without waiting on parakeet.cpp CUDA support.

This approach was contributed by [@ed0c](https://github.com/silverstein/minutes/issues/122).
Tested on an RTX 3090 with CUDA 13.2: a 68-minute French meeting transcribes
in about 3.5 minutes total, with quality that beats Whisper large-v3 on
mixed-language audio.

**1. Create a Python venv with NeMo**

```bash
python3 -m venv ~/parakeet-env
source ~/parakeet-env/bin/activate
pip install nemo_toolkit[asr]
```

**2. Create the wrapper script**

Save this as `~/bin/parakeet-nemo` (or any path you control) and `chmod +x` it:

```bash
#!/bin/bash
source ~/parakeet-env/bin/activate

python3 - "$@" << 'EOF'
import sys
import os
import contextlib

os.environ['PYTORCH_CUDA_ALLOC_CONF'] = 'expandable_segments:True'

audio_files = [a for a in sys.argv[1:] if a.endswith('.wav')]
if not audio_files:
    sys.exit(0)

with contextlib.redirect_stdout(sys.stderr):
    import nemo.collections.asr as nemo_asr
    model = nemo_asr.models.ASRModel.from_pretrained('nvidia/parakeet-tdt-0.6b-v3')
    model = model.cuda()

output = model.transcribe(audio_files, timestamps=True)
for result in output:
    segments = result.timestamp.get('segment', [])
    if segments:
        for seg in segments:
            text = seg['segment'].strip()
            if text:
                sys.stdout.write(f"[{seg['start']:.2f} - {seg['end']:.2f}] {text}\n")
                sys.stdout.flush()
    elif result.text.strip():
        sys.stdout.write(f"[0.00 - 1.00] {result.text.strip()}\n")
        sys.stdout.flush()
EOF
```

**3. Point Minutes at it**

In `~/.config/minutes/config.toml`:

```toml
[transcription]
engine = "parakeet"
parakeet_binary = "/home/you/bin/parakeet-nemo"
parakeet_model = "tdt-600m"
parakeet_vocab = "tdt-600m.tokenizer.vocab"
```

**Known limitation: per-chunk model reload**

Minutes invokes the parakeet binary once per audio chunk, so the NeMo
wrapper reloads the model from disk cache on every call (about 4 to 5
seconds of overhead per chunk). For long recordings this adds up. A
persistent daemon that keeps the model resident in VRAM eliminates the
reload cost; see [#122](https://github.com/silverstein/minutes/issues/122)
if you want to help land one.

## Build parakeet.cpp

No upstream binary releases exist as of 2026-05-15, so the build is mandatory.
There are no Homebrew, MacPorts, or precompiled GitHub Release artifacts to
fall back on.

```bash
# Clone with submodules
git clone --recursive https://github.com/Frikallo/parakeet.cpp
cd parakeet.cpp

# Configure. The four AXIOM/PARAKEET flags below are the load-bearing ones:
#   AXIOM_INSTALL=OFF + PARAKEET_INSTALL=OFF
#     Disable the install(EXPORT "AxiomTargets") rule that breaks on any
#     modern CMake. The parakeet.cpp maintainer added these as the supported
#     escape hatch (see CMakeLists.txt:96-100).
#   PARAKEET_BUILD_SERVER_EXAMPLE=ON
#     Builds example-server, the warm sidecar binary live mode talks to.
#     Off by default; without it live transcription respawns per utterance.
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF \
  -DPARAKEET_INSTALL=OFF \
  -DPARAKEET_BUILD_SERVER_EXAMPLE=ON \
  -DAXIOM_BUILD_TESTS=OFF \
  -DAXIOM_BUILD_EXAMPLES=OFF
cmake --build build -j

# Binary layout depends on the generator CMake picked. Ninja:
#   build/bin/parakeet
#   build/bin/example-server
# Unix Makefiles (default when Ninja isn't installed):
#   build/parakeet
#   build/examples/server/example-server
# This find handles either:
mkdir -p ~/.local/bin
find build -type f -perm -u+x \( -name parakeet -o -name example-server \) \
  -exec cp {} ~/.local/bin/ \;

# Verify Metal + Accelerate linkage on Apple Silicon
otool -L ~/.local/bin/parakeet | grep -E 'Metal|Accelerate'
```

If `cmake --build` fails with `Neither lock free instructions nor -latomic
found`, you're on CMake 4.x — install CMake 3.31.x and reconfigure. See
[Troubleshooting](#cmake-4x-atomics-error-build).

## Install Models

Parakeet models are distributed as `.nemo` files on HuggingFace and must be
converted to safetensors format. There is currently one model-install path:
manual download + conversion. `minutes setup --parakeet` is a helper that
installs the native Silero VAD weights and prints the manual recipe — it
does not download or convert the `.nemo` itself.

### Step 1 — Silero VAD weights + binary resolution (one command)

```bash
minutes setup --parakeet                                      # Multilingual v3 (tdt-600m)
# or:
minutes setup --parakeet --parakeet-model tdt-ctc-110m         # English-only compact
```

This installs `~/.minutes/models/parakeet/silero_vad_v5.safetensors` (1.2 MB)
and prints a `parakeet` binary resolution line. It runs to completion on a
whisper-only CLI (e.g. the Homebrew Formula build) — you'll see a feature-flag
warning, but the VAD weights and binary check still complete.

### Step 2 — Python environment for the conversion script

`convert_nemo.py` needs PyTorch and `safetensors`. `safetensors` has a
transitive `packaging` dependency it does not auto-pull, and torch emits a
warning on first tensor op without `numpy` — install both explicitly.
`torchaudio` and `huggingface_hub` are **not** required and add hundreds of
MB.

```bash
# uv is fastest; `python -m venv` + `pip install` works equivalently
uv venv ~/.local/venvs/parakeet-convert --python 3.13
uv pip install --python ~/.local/venvs/parakeet-convert/bin/python \
  torch safetensors packaging numpy
source ~/.local/venvs/parakeet-convert/bin/activate
```

### Step 3 — Download the .nemo

HuggingFace transparently redirects this public URL to its xet-hub CDN; no
auth and no `huggingface_hub` Python package required.

```bash
mkdir -p /tmp/parakeet-dl && cd /tmp/parakeet-dl
curl -L -O https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo
# For the English-only compact model, swap the repo:
#   https://huggingface.co/nvidia/parakeet-tdt_ctc-110m/resolve/main/parakeet-tdt_ctc-110m.nemo
```

### Step 4 — Convert to safetensors

```bash
mkdir -p ~/.minutes/models/parakeet/tdt-600m
python ~/src/parakeet.cpp/scripts/convert_nemo.py \
  parakeet-tdt-0.6b-v3.nemo \
  -o ~/.minutes/models/parakeet/tdt-600m/tdt-600m.safetensors \
  --model 600m-tdt
# Output is ~2.3 GB, 627M params, 723 tensors.
```

### Step 5 — Extract the SentencePiece tokenizer.vocab

macOS BSD `tar` does **not** support GNU's `--wildcards` / `--no-anchored`
flags. The vocab inside the `.nemo` is named `<UUID>_tokenizer.vocab`, so
list the archive, grab the literal filename, and extract that.

```bash
tar tf parakeet-tdt-0.6b-v3.nemo | grep tokenizer.vocab
# e.g.   ./8f3c5b6e-1d2a-4f7c-9a3b-de1a2b3c4d5e_tokenizer.vocab
tar xf parakeet-tdt-0.6b-v3.nemo ./8f3c5b6e-..._tokenizer.vocab   # use your UUID
cp *_tokenizer.vocab ~/.minutes/models/parakeet/tdt-600m/tdt-600m.tokenizer.vocab
```

GNU-tar users (Linux, or `gtar` from Homebrew on macOS) can still use the
original form: `tar xf *.nemo --wildcards --no-anchored '*tokenizer.vocab'`.

### Step 6 — Reclaim disk

```bash
rm -rf /tmp/parakeet-dl    # the .nemo is no longer needed
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
# parakeet_sidecar_enabled auto-enables when example-server (copied above) resolves; set true/false to force
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

For local macOS builds in this repo, prefer the helper scripts because they keep the CLI and desktop app aligned on the same feature set:

```bash
MINUTES_BUILD_FEATURES=parakeet,metal ./scripts/build.sh
MINUTES_BUILD_FEATURES=parakeet,metal ./scripts/install-dev-app.sh
```

Note: The `parakeet` feature is opt-in and not included in the default build.
Whisper is always compiled in (it's the default feature). Both engines can coexist
in the same binary — the config file selects the offline/batch path plus both
live transcription paths (`minutes record` sidecar and standalone `minutes live`).
Dictation still uses Whisper. See [Scope](#scope).

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

Google Highway's `FindAtomics.cmake` is incompatible with CMake 4.x on Apple
Silicon. The atomics check fails because it forces `CMAKE_CXX_STANDARD 11`
which conflicts with the project's C++20:

```
Neither lock free instructions nor -latomic found.
```

Build with CMake 3.31.x. Homebrew dropped the `cmake@3` formula, so install
the Kitware universal tarball into a local opt dir and prepend it on `PATH`
only for the parakeet.cpp build shell:

```bash
mkdir -p ~/.local/opt && cd ~/.local/opt
curl -L -O https://github.com/Kitware/CMake/releases/download/v3.31.12/cmake-3.31.12-macos-universal.tar.gz
tar xf cmake-3.31.12-macos-universal.tar.gz
export PATH="$HOME/.local/opt/cmake-3.31.12-macos-universal/CMake.app/Contents/bin:$PATH"
cmake --version    # confirm 3.31.x is now first

cd ~/src/parakeet.cpp
rm -rf build
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF -DPARAKEET_INSTALL=OFF \
  -DPARAKEET_BUILD_SERVER_EXAMPLE=ON \
  -DAXIOM_BUILD_TESTS=OFF -DAXIOM_BUILD_EXAMPLES=OFF
cmake --build build -j
```

Only the parakeet.cpp build needs CMake 3.x; CMake 4.x can stay on the
system `PATH` for everything else.

### axiom install(EXPORT) export-set error (build)

```
CMake Error in third_party/axiom/CMakeLists.txt:
  install(EXPORT "AxiomTargets" ...) includes target "axiom" which requires
  target "hwy" that is not in any export set.
```

This fires on any modern CMake (3.x and 4.x), not just CMake 4. The
parakeet.cpp `axiom` submodule exports `axiom` but does not export its
`hwy` transitive, and CMake's `install(EXPORT)` rule rejects that.

The parakeet.cpp maintainer added two opt-out flags at `CMakeLists.txt:96-100`
specifically for this case. Pass them at configure time:

```bash
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF \
  -DPARAKEET_INSTALL=OFF \
  ...
```

You only lose the ability to `make install` parakeet.cpp into a system
prefix, which the Minutes workflow doesn't use anyway (we copy the
binaries out of `build/` into `~/.local/bin/` manually).

### Metal shader compiler not found (build)
Requires full Xcode (not just Command Line Tools):
```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
xcodebuild -downloadComponent MetalToolchain
```
