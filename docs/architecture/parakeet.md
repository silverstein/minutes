# Parakeet Integration Reference

Minutes retains an experimental [parakeet.cpp](https://github.com/Frikallo/parakeet.cpp)
integration, but it is not currently selectable on any platform. Parakeet uses
NVIDIA's FastConformer architecture; the performance figures below describe the
historical benchmark environment, not an available Minutes runtime option.

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

The code retains those integration paths, but the current pathname-only
Parakeet CLI is not selectable on any platform. macOS securely normalizes
audio into an authenticated encrypted spool that the CLI cannot consume.
Linux can inherit an anonymous descriptor, but ordinary `execve` resets child
dumpability and leaves that descriptor race-openable through `/proc/<pid>/fd`
to a hostile same-UID process. Windows stores normalized audio as authenticated
ciphertext, but the Parakeet subprocess has no supported byte transport into
that sealed capability.
Batch, live, and dictation requests therefore fall back to in-process Whisper
instead of creating a visible plaintext file or exposing a raw descriptor.

The warm server's pathname-only protocol likewise cannot receive Minutes'
anonymous/sealed private-audio capability without reopening mutable ambient
authority. Auto and explicit-on configuration report the sidecar as
unavailable instead of warming a server that private transcription would
bypass. Exact byte/stdin transfer or an acknowledged post-exec descriptor
isolation protocol remains the requirement for removing the fallback.

Parakeet is eligible for standalone-live fallback only when the current build,
platform, transport, and runtime storage probe all report it ready. No current
platform satisfies the Parakeet transport requirement. A retained Parakeet
preference therefore resolves to Whisper. Apple Speech also currently resolves
to Whisper because its pathname-only helper has the same private-audio
transport gap. See [`docs/architecture/apple-speech.md`](apple-speech.md).

All live utterances that request Parakeet use Whisper until parakeet.cpp accepts
a byte stream or a post-exec helper proves descriptor isolation. Installing
`example-server` or forcing `parakeet_sidecar_enabled = true` does not bypass
the private-audio safety gate.

Dictation remains Whisper by default because its overlay depends on fast
mid-utterance partials. A retained `backend = "parakeet"` preference is accepted
only as configuration history and resolves to Whisper; the settings UI does
not allow a new Parakeet selection until secure transport exists.

```toml
[dictation]
backend = "parakeet"
```

With that retained value, Whisper powers both progressive partial text and
VAD-finalization. A future transport-capable Parakeet integration may take over
finalization only after the shared capability gate reports it selectable.

Minutes logs the unavailable secure-transport reason and resolves Parakeet
preferences to Whisper for batch, live, and dictation paths.

The `parakeet` Cargo feature implies `whisper` in minutes-core and each host
crate. This keeps the runtime fallback executable even in
`--no-default-features --features parakeet` builds; a supported Parakeet build
cannot silently degrade to placeholder text or fail live startup merely because
Whisper was omitted from the feature list.

## macOS source-build reference (runtime remains Whisper)

The steps below document how the historical Metal benchmark environment was
built. They do **not** make Parakeet selectable in the current Minutes app:
macOS normalizes audio into an authenticated encrypted spool, while
parakeet.cpp accepts only a pathname. Minutes keeps using Whisper until an
exact byte or inherited-descriptor transport is implemented. Installing these
assets is useful only for development of that future transport and should not
be presented to packaged-app users as setup they need to complete.

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
# can build the optional server for future exact-descriptor transport work;
# current Unix builds use the supervised direct subprocess for private audio.
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF \
  -DPARAKEET_INSTALL=OFF \
  -DPARAKEET_BUILD_SERVER_EXAMPLE=OFF \
  -DAXIOM_BUILD_TESTS=OFF \
  -DAXIOM_BUILD_EXAMPLES=OFF
cmake --build build -j

# Install the Parakeet binary. Locations differ by generator: Ninja puts it in
# build/bin/, while Unix Makefiles put it in build/.
mkdir -p ~/.local/bin
find build -type f -perm -u+x -name parakeet \
  -exec cp {} ~/.local/bin/ \;
ls -lh ~/.local/bin/parakeet

# Sanity check: the binary should link Metal, MPS, MPSGraph, and Accelerate.
otool -L ~/.local/bin/parakeet | grep -E 'Metal|Accelerate'

# 2. Build the Minutes CLI WITH the parakeet feature, then install it
cd <path/to/your/minutes/checkout>     # e.g. ~/Sites/minutes
cargo build --release -p minutes-cli --features parakeet
mkdir -p ~/.local/bin
rm -f ~/.local/bin/minutes && cp target/release/minutes ~/.local/bin/minutes
# Make sure ~/.local/bin is on PATH (add to ~/.zshrc if it isn't):
#   export PATH="$HOME/.local/bin:$PATH"

# 3. Stop here for the current macOS runtime. Do not run
#    `minutes setup --parakeet`: the command now refuses before installing
#    assets because macOS cannot securely transport sealed audio to the
#    pathname-only Parakeet process. The remaining conversion steps are only
#    a developer reference for future transport work, not product setup.
```

4. Optional developer reference: download and convert the multilingual
   `tdt-600m` model manually. This recipe is now developer reference only; the
   current CLI rejects setup on every platform. The `.nemo` file is publicly
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
# `parakeet_sidecar_enabled` may record future warm-server intent, but the
# current pathname-only server is safety-gated off for private audio.
parakeet_fp16 = true
```

That reproduces the Parakeet model and binary used by the historical benchmark:
- `tdt-600m`
- local `parakeet.cpp`
- local Metal GPU acceleration on Apple Silicon

If you want the smaller English-only model instead, set
`parakeet_model = "tdt-ctc-110m"` and
`parakeet_vocab = "tdt-ctc-110m.tokenizer.vocab"` in the same file.

Minutes continues to transcribe with Whisper on macOS either way.

## Current macOS desktop behavior

The desktop app disables Parakeet on macOS because its pathname-only process
cannot receive Minutes' sealed audio. A retained `engine = "parakeet"`
preference resolves visibly to Whisper. Changing `PATH` or setting an absolute
`parakeet_binary` does not make the engine selectable; those settings matter
only to historical benchmark/source-build work outside the current app path.

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

### Linux source-build reference (runtime remains Whisper)

parakeet.cpp does not yet have CUDA support (WIP in the axiom tensor library).
Linux CPU-only builds work but lose the speed advantage. Minutes currently
falls back to Whisper on Linux, macOS, and Windows until a secure process
transport is implemented. Monitor the
[parakeet.cpp repo](https://github.com/Frikallo/parakeet.cpp) for CUDA updates.

### Linux with an NVIDIA GPU (NeMo wrapper, CUDA)

If you have an NVIDIA GPU on Linux, NVIDIA's [NeMo toolkit](https://github.com/NVIDIA/NeMo)
supports Parakeet natively with full CUDA acceleration. The historical Minutes
benchmark path accepted executables following the parakeet.cpp CLI contract,
including the wrapper below. Current Minutes releases reject that pathname-only
contract at the shared private-audio gate, so this is developer reference only
and does not enable GPU-backed transcription in the app.

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

**3. Retained configuration reference (does not activate Parakeet)**

In `~/.config/minutes/config.toml`:

```toml
[transcription]
engine = "parakeet"
parakeet_binary = "/home/you/bin/parakeet-nemo"
parakeet_model = "tdt-600m"
parakeet_vocab = "tdt-600m.tokenizer.vocab"
```

Current Minutes still resolves this preference to Whisper. Compiling the
feature, installing model assets, or changing the binary path does not bypass
the secure-transport requirement.

**Historical limitation: per-chunk model reload**

The historical benchmark invoked the parakeet binary once per audio chunk, so
the NeMo wrapper reloaded the model from disk cache on every call (about 4 to 5
seconds of overhead per chunk). For long recordings this added up. A
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
#   PARAKEET_BUILD_SERVER_EXAMPLE=OFF
#     The current pathname-only server is safety-gated off for private audio.
#     Live transcription uses the supervised direct subprocess per utterance.
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DAXIOM_INSTALL=OFF \
  -DPARAKEET_INSTALL=OFF \
  -DPARAKEET_BUILD_SERVER_EXAMPLE=OFF \
  -DAXIOM_BUILD_TESTS=OFF \
  -DAXIOM_BUILD_EXAMPLES=OFF
cmake --build build -j

# Binary layout depends on the generator CMake picked: Ninja uses
# build/bin/parakeet; Unix Makefiles use build/parakeet. This finds either:
mkdir -p ~/.local/bin
find build -type f -perm -u+x -name parakeet \
  -exec cp {} ~/.local/bin/ \;

# Verify Metal + Accelerate linkage on Apple Silicon
otool -L ~/.local/bin/parakeet | grep -E 'Metal|Accelerate'
```

If `cmake --build` fails with `Neither lock free instructions nor -latomic
found`, you're on CMake 4.x — install CMake 3.31.x and reconfigure. See
[Troubleshooting](#cmake-4x-atomics-error-build).

## Install Models (reference only; runtime remains Whisper)

Parakeet models are distributed as `.nemo` files on HuggingFace and must be
converted to safetensors format. There is currently one model-install path:
manual download + conversion. Current Minutes builds stop before creating
directories or installing assets because no platform can prove a secure
Parakeet process transport. The commands below remain developer reference for
future transport work; use Whisper today.

### Step 1 — Silero VAD weights + binary resolution (one command)

```bash
minutes setup --parakeet                                      # Multilingual v3 (tdt-600m)
# or:
minutes setup --parakeet --parakeet-model tdt-ctc-110m         # English-only compact
```

Current builds reject this setup before installing partial assets. A future
transport-enabled build may install
`~/.minutes/models/parakeet/silero_vad_v5.safetensors` and print a binary
resolution line.

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

## Configure Minutes (future transport reference)

The configuration below is retained for future transport development and
benchmarks. Current Linux, macOS, and Windows builds keep Whisper selected; do
not configure Parakeet or troubleshoot its binary/model paths as an end-user
setup path.

### Config file

Edit `~/.config/minutes/config.toml`:

```toml
[transcription]
engine = "parakeet"              # "whisper" (default) or "parakeet"
parakeet_model = "tdt-600m"      # "tdt-ctc-110m" (English) or "tdt-600m" (multilingual v3)
parakeet_binary = "/home/you/.local/bin/parakeet"  # Prefer an absolute path for desktop app launches
# parakeet_sidecar_enabled records intent but cannot bypass the transport gate
parakeet_boost_limit = 25        # Experimental: top graph-derived boost phrases (0 disables)
parakeet_boost_score = 2.0       # Experimental tuning for parakeet.cpp --boost-score
parakeet_fp16 = false            # Reserved/inert today
parakeet_vocab = "tdt-600m.tokenizer.vocab"  # Safer when multiple Parakeet models are installed
```

### Tauri Desktop App on Linux

Settings > Transcription > Engine shows Parakeet as unavailable and keeps
Whisper active. Compiling the feature or installing assets does not bypass the
secure-transport gate.

## Language Support

| Model | Languages |
|-------|-----------|
| tdt-ctc-110m | English only |
| tdt-600m (v3) | 25 European languages: Bulgarian, Croatian, Czech, Danish, Dutch, English, Estonian, Finnish, French, German, Greek, Hungarian, Italian, Latvian, Lithuanian, Maltese, Polish, Portuguese, Romanian, Russian, Slovak, Slovenian, Spanish, Swedish, Ukrainian |

For languages outside this list, use Whisper (99 languages supported).

## Building Minutes with Parakeet Support

The `parakeet` Cargo feature can still be enabled for development and transport
work. These commands build that dormant integration path:

```bash
# CLI only
cargo build --release -p minutes-cli --features parakeet

# Tauri desktop app
TAURI_FEATURES="parakeet" cargo tauri build --bundles app

# Or use the build script (add parakeet feature)
cargo build --release -p minutes-cli --features parakeet
```

Note: The `parakeet` feature is opt-in and not included in the default build.
Enabling it also enables Whisper because Whisper is the required runtime
fallback. Current capability resolution keeps Whisper active for batch, live,
and dictation. See [Scope](#scope).

## Switching Back to Whisper

Use `engine = "whisper"` in config.toml or the Tauri settings UI. Retained
Parakeet preferences already resolve to Whisper without a rebuild.

## Troubleshooting

### "parakeet binary not found" (Linux)
The `parakeet` executable is not in your PATH. Either:
- Add its location to PATH: `export PATH="$PATH:/path/to/parakeet.cpp/build/bin"`
- Or set the full path in config: `parakeet_binary = "/path/to/parakeet"`

This developer advice does not make Parakeet selectable: all current platforms
use Whisper because secure process transport is unavailable, not because a
binary path needs repair.

### "unknown parakeet model"
Only `tdt-ctc-110m` and `tdt-600m` are supported. Check your config.

### "Expected parakeet model in ~/.minutes/models/parakeet/" (Linux)
This message belongs to the dormant developer integration. Current builds
reject `minutes setup --parakeet` before creating directories or installing
assets because model setup cannot repair the missing secure process transport.
Use Whisper for production transcription.

### Historical source-build note: fp16 MPSGraph crash on Apple Silicon

This section documents the historical benchmark/source-build environment. It
is not a fix for current Minutes on macOS, which uses Whisper because secure
Parakeet transport is unavailable.

On some Apple Silicon + model combinations the fp16 GPU path crashes inside
Apple's MPSGraph with an operand type-mismatch, e.g.:

```
'mps.add' op requires the same element type for all operands and results
... original module failed verification
```

This is an upstream `parakeet.cpp` / Apple MPSGraph limitation (an fp16 kernel
mixing f16 and f32 tensors), **not** a Minutes bug and **not** a broken machine.
Minutes detects the crash signature, persists an fp16 blacklist fingerprint at
`~/.minutes/parakeet-fp16-blacklist.json`, and **auto-retries in fp32** — so
transcription keeps working with no action required (see
`crates/core/src/parakeet_sidecar.rs`, `FP16_CRASH_SIGNATURES`).

To skip the crash-and-retry on every run, set `parakeet_fp16 = false` in
config.toml. fp32 is slightly slower and uses more GPU memory but is otherwise
identical in output. To re-test fp16 after an OS/model update, set
`parakeet_fp16_blacklist_reset = true` once (it clears the persisted fingerprint).

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
