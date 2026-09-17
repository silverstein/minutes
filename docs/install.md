# Install

### macOS

```bash
# Desktop app (menu bar, recording UI, AI assistant)
brew install --cask silverstein/tap/minutes

# CLI only (terminal recording, search, vault sync)
brew tap silverstein/tap
brew install minutes

# Or from source (requires Rust + cmake)
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo install --path crates/cli
```

#### Which install gets Parakeet by default

- The signed desktop app does: its sherpa plugin is bundled and signed.
- `minutes-macos-arm64-sherpa.tar.gz` does: its Developer ID-signed plugin sits
  beside the binary.
- The bare `minutes-macos-arm64` asset, `cargo install`, and the Homebrew CLI
  formula do not. Those installs use Whisper until the plugin is placed beside
  the binary or in another documented loader path.

On Apple Silicon, the MCP server's auto-install fetches and checksum-verifies
the sherpa archive, validates the extracted CLI, then atomically installs the
binary and plugin together. Older releases without the archive fall back to the
checksum-verified bare binary and stay on Whisper. When `auto` sees the plugin
but the Parakeet model is missing, the MCP server runs plain `minutes setup`,
which installs Parakeet plus the Whisper tiny fallback.

### Windows

Download **`minutes-windows-x64.zip`** from the
[latest release](https://github.com/silverstein/minutes/releases/latest), unzip
it, and run `minutes.exe` from the unzipped folder.

Use the zip rather than the bare `minutes-windows-x64.exe`. Minutes is built
with MSVC and imports the Visual C++ runtime, which is not part of Windows. On
a PC that has never had the Visual C++ Redistributable installed the bare
executable exits immediately and prints nothing at all. The zip carries those
runtime files beside the executable, so it works on a fresh machine with no
install step and no admin rights. Keep the DLLs next to `minutes.exe` when you
move it.

```powershell
# Or build from source:
# Requires: Rust, cmake, MSVC build tools, LLVM (for libclang)

# Install LLVM (needed by whisper-rs bindgen):
winget install LLVM.LLVM
[Environment]::SetEnvironmentVariable("LIBCLANG_PATH", "C:\Program Files\LLVM\bin", "User")
# Restart your terminal after setting LIBCLANG_PATH

# Compressed imports use ffmpeg.exe when installed, otherwise the bundled bounded decode worker. Add its bin directory to PATH, or:
[Environment]::SetEnvironmentVariable("MINUTES_FFMPEG", "C:\path\to\ffmpeg.exe", "User")

# Full build (includes speaker diarization):
cargo install --path crates/cli

# Without speaker diarization:
cargo install --path crates/cli --no-default-features --features whisper
```

> **Note:** If diarization fails to compile on Windows, use
> `--no-default-features --features whisper` so transcription remains enabled.
> This is a [known upstream issue](https://github.com/silverstein/minutes/issues/27)
> with `pyannote-rs`'s ONNX Runtime dependency. Everything except speaker labels works without it.

### Linux

```bash
# Debian/Ubuntu — full dep list:
sudo apt-get install -y \
  build-essential cmake pkg-config \
  clang libclang-dev \
  libasound2-dev libpipewire-0.3-dev libspa-0.2-dev \
  ffmpeg

cargo install minutes-cli
# or, from a checkout:
cargo install --path crates/cli
```

**Why each dep is needed:**
- `build-essential`, `cmake` — whisper.cpp build
- `clang`, `libclang-dev` — bindgen (used by `whisper-rs` and `pipewire-sys`)
- `libasound2-dev` — cpal's ALSA backend
- `libpipewire-0.3-dev`, `libspa-0.2-dev` — cpal's PipeWire backend (compiled unconditionally on Linux)
- `ffmpeg` — preferred bounded decoder for `.m4a`/`.mp3`/`.ogg`; optional, since the bundled bounded decode worker handles the same containers when ffmpeg is absent. WAV is decoded directly

**Other distros** are best-effort; Debian/Ubuntu is the validated path. Ask for
setup help in [Discussions](https://github.com/silverstein/minutes/discussions)
if a package name is wrong for your distro.

- **Fedora/RHEL**: `sudo dnf install -y gcc-c++ cmake pkgconf-pkg-config clang clang-devel alsa-lib-devel pipewire-devel ffmpeg-free`
- **Arch**: `sudo pacman -S --needed base-devel cmake clang alsa-lib pipewire ffmpeg`

### Chromebook (Crostini)

Yes, Minutes runs on a Chromebook via the Linux development environment (Crostini). The CLI is the supported path — there's no native ChromeOS build and the Tauri desktop app isn't exercised there, but the core engine, folder watcher, and MCP server all work.

**One-time ChromeOS setup:**

1. **Turn on Linux.** Settings → About ChromeOS → Developers → Linux development environment → Turn on. Pick a disk size of 10 GB or more (whisper models plus build artifacts).
2. **Grant microphone access to the Linux container.** Settings → Developers → Linux development environment → toggle **Allow Linux to access your microphone**. This is off by default and is the single most common reason `minutes record` produces silence on a Chromebook.
3. **Open the Linux terminal** and follow the [Debian/Ubuntu](#linux) install above (`apt-get install …` + `cargo install minutes-cli`).

**Verify the environment** before your first real recording:

```bash
minutes health          # confirms model, mic, disk, watcher
minutes record          # speak for 5 seconds
minutes stop
```

If `minutes health` flags the mic as missing, the ChromeOS mic toggle is off — not a cpal bug. Flip it on in Settings and re-run.

**What works well on a Chromebook:**

- `minutes watch` is the killer flow. Drop voice memos from your phone into a synced Google Drive / Dropbox folder that also mounts inside Crostini, and Minutes auto-transcribes them. No mic permission dance, no hotkey fight.
- CLI recording and transcription with the `tiny` / `base` / `small` models. Expect CPU-only performance — Crostini doesn't expose GPU acceleration to Linux apps, so skip `--features metal/cuda/vulkan` and pick a smaller model than you would on a Mac.
- The MCP server (`npx minutes-mcp`) for Claude Desktop or other MCP clients running inside the container.

**What to expect less of:**

- **No global hotkeys or tray app.** ChromeOS doesn't surface system-level shortcuts to Crostini. `minutes record` / `minutes stop` from the terminal is the intended flow.
- **No Tauri desktop app support.** It may build, but it isn't tested and the live-coaching / AI Assistant surface assumes a macOS-style window server.
- **Slower transcription.** A Chromebook CPU on the `small` model is usually 2–4x realtime for English. Budget accordingly, or lean on the folder watcher where latency doesn't matter.

If Crostini support breaks for you, please [open an issue](https://github.com/silverstein/minutes/issues) — Chromebook isn't a first-class test target yet, so real bug reports are the fastest way to harden it.

### GPU acceleration

macOS release binaries (DMG + `cargo install minutes-cli` from published CI
artifacts) ship with Metal enabled — `large-v3` runs ~2× faster than the
CPU-only build and offloads nearly all work to the GPU. Other backends remain
opt-in at build time.

| Backend | Platform | Feature flag | Prerequisites | Default in release |
|---------|----------|-------------|---------------|--------------------|
| Metal | macOS | `metal` | Xcode Command Line Tools | **Yes** |
| CoreML | macOS | `coreml` | Xcode Command Line Tools + `.mlmodelc` bundle | No |
| CUDA | Windows/Linux | `cuda` | [CUDA Toolkit](https://developer.nvidia.com/cuda-toolkit) | No |
| ROCm/HIP | Linux | `hipblas` | [ROCm](https://rocm.docs.amd.com/) 6.1+ (`hipcc`, `hipblas`, `rocblas`) | No |
| Vulkan | Windows/Linux | `vulkan` | [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) (+ `vulkan-headers` on Arch) | No |

Metal is the only backend that is exercised daily by the maintainer. CUDA, ROCm/HIP,
and Vulkan should be considered experimental: they wire through to whisper.cpp via
whisper-rs and are expected to work, but have not been validated in CI.

> **NVIDIA on Windows:** prefer `vulkan` over `cuda`. The CUDA path on older
> (Pascal / GTX 10-series) GPUs hits an NVIDIA-toolchain crash that Minutes
> cannot work around; Vulkan gives GPU acceleration with no CUDA Toolkit. See
> [GPU acceleration](development/gpu-acceleration.md).

```bash
# Apple Metal (macOS) — already enabled in the release DMG; use this for source builds
cargo install --path crates/cli --features metal

# Apple CoreML (macOS Neural Engine) — encoder-only; see note below
cargo install --path crates/cli --features metal,coreml

# NVIDIA GPU (Windows/Linux)
cargo install --path crates/cli --features cuda

# AMD GPU via ROCm (Linux — experimental)
cargo install --path crates/cli --features hipblas

# Vulkan (Windows/Linux — experimental)
cargo install --path crates/cli --features vulkan
```

> **CoreML note:** `--features coreml` only accelerates the Whisper encoder on
> the Apple Neural Engine. It requires the companion `ggml-<model>-encoder.mlmodelc`
> bundle next to the `.bin` weights (e.g. for `large-v3`, download
> [`ggml-large-v3-encoder.mlmodelc.zip`](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-encoder.mlmodelc.zip)
> and unzip into `~/.minutes/models/`). Without it, whisper.cpp silently falls
> back to the CPU/Metal encoder. Stack it with `metal` for the best of both
> worlds — a subsequent PR will fetch the bundle automatically from
> `minutes setup --model large-v3 --coreml`.

> **Windows CUDA users:** You may need to set environment variables before building:
> ```powershell
> $env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4"
> $env:CMAKE_CUDA_COMPILER = "$env:CUDA_PATH\bin\nvcc.exe"
> $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
> $env:CMAKE_GENERATOR = "NMake Makefiles"
> ```
> The first CUDA build takes longer than usual (compiling GPU kernels) — this is a one-time cost.

> **ROCm/HIP users:** The build expects ROCm installed at `/opt/rocm`. If your
> installation is elsewhere, set `HIP_PATH` before building:
> ```bash
> export HIP_PATH=/path/to/rocm
> ```
>
> **Vulkan users:** On Windows and macOS, set `VULKAN_SDK` to your SDK install
> root before building. On Linux, `whisper-rs-sys` links against the system
> `libvulkan`.

### Setup (all platforms)

```bash
# Download whisper model (also downloads Silero VAD model for non-English audio)
minutes setup --model small   # Recommended (466MB, good accuracy)
minutes setup --model tiny    # Fastest (75MB, but misses quiet audio)
minutes setup --model base    # Middle ground (141MB)

# Install ffmpeg for non-WAV audio formats such as m4a/mp3/ogg/flac
brew install ffmpeg           # macOS
# apt install ffmpeg          # Linux
# Windows: install ffmpeg.exe and add it to PATH, or set MINUTES_FFMPEG
# to its full path (for example C:\path\to\ffmpeg.exe).
# Without ffmpeg, compressed input is decoded by the bundled bounded worker;
# WAV processing remains available.

# Enable speaker diarization (optional, ~34MB ONNX models)
minutes setup --diarization

# Experimental engine: Sherpa (in-process sherpa-onnx running parakeet-tdt-0.6b-v3).
# Newer multilingual model (EN/FR/ES and more EU languages), no Python. Opt-in and
# NOT yet the recommended daily engine: in real-meeting A/B it trails the Parakeet
# engine on segmentation/casing (see issue #369).
#
# sherpa runs from an isolated plugin, so it now coexists with diarization on
# every platform. Both stacks vendor their own ONNX Runtime and
# kaldi-native-fbank, and one executable can only keep one copy of each, which
# broke voice enrollment or sherpa itself depending on which copy won (#683).
# Loading sherpa from its own dynamic library removes the conflict (#685).
# Build the plugin, then the CLI, then copy the plugin where the CLI looks:
#   (cd crates/sherpa-plugin && cargo build --release)
#   cargo build --release -p minutes-cli --features engine-sherpa,metal
#     cp crates/sherpa-plugin/target/release/libminutes_sherpa.dylib ~/.local/bin/
#   (beside the minutes binary, wherever you installed it)
# Then enable in one command:
minutes setup --sherpa        # downloads the int8 ONNX model (~670MB) + sets engine = "sherpa"
# If you select sherpa without the feature, model, or plugin, transcription auto-falls-back
# to Whisper (the bundled default), so a recording never breaks. See docs/architecture/sherpa-engine.md.
# macOS sherpa builds are self-contained (static). On Linux/Windows, run from the
# repo (cargo run) rather than copying the binary out of target/ — details in the doc.

# Parakeet preferences currently resolve to Whisper on every platform. The
# pathname-only parakeet.cpp helper cannot safely receive Minutes' sealed
# private audio, so setup and selection fail closed until a secure byte
# transport lands. See docs/architecture/parakeet.md for technical status.

# Enroll your voice for automatic speaker identification
minutes enroll              # Records 10s of your voice
minutes voices              # View enrolled profiles
```

### Speaker identification

Minutes maps anonymous speaker labels (`SPEAKER_1`, `SPEAKER_2`) to real names using four levels of confidence-aware attribution:

| Level | How | Confidence | Requires |
|-------|-----|-----------|----------|
| **0** | Calendar attendees + `identity.name` → deterministic mapping for 1-on-1 meetings | Medium | Calendar access, `[identity] name` in config |
| **1** | LLM analyzes transcript context clues and maps speakers to attendees | Medium (capped) | Attendees known + summarization engine or agent CLI |
| **2** | Your enrolled voice is matched against speaker segments | High | `minutes enroll` (one-time 10s recording) |
| **3** | You confirm "SPEAKER_1 is Sarah" after a meeting | High | `minutes confirm --meeting <path>` |

Only **High**-confidence attributions rewrite transcript labels. Medium/Low are stored in frontmatter (`speaker_map`) for Claude to surface when asked — "SPEAKER_1 is likely Sarah."

```bash
# Set your name (required for Levels 0-2)
# In your config file (`$XDG_CONFIG_HOME/minutes/config.toml` when set,
# otherwise `~/.config/minutes/config.toml`):
[identity]
name = "Your Name"

# Enroll your voice (Level 2)
minutes enroll                    # Record 10s sample
minutes enroll --file sample.wav  # Or from existing audio

# Confirm attributions after a meeting (Level 3)
minutes confirm --meeting ~/meetings/2026-03-25-standup.md
minutes confirm --meeting path.md --speaker SPEAKER_1 --name "Sarah" --save-voice

# Manage voice profiles
minutes voices              # List profiles
minutes voices --json       # JSON output
minutes voices --delete     # Remove all profiles
```

**Recover a failed mapping (Level 1).** If a meeting shipped with anonymous `SPEAKER_n` labels (the Level-1 call timed out, errored, or ran before attendees were known), re-run just the speaker mapping without reprocessing the audio:

```bash
minutes redo-speaker-mapping <meeting>                          # dry run: show the proposed map
minutes redo-speaker-mapping <meeting> --apply                  # write speaker_map + a speaker_mapping health block
minutes redo-speaker-mapping <meeting> --engine ollama --apply  # override the engine for this run
minutes redo-speaker-mapping <meeting> --json                   # machine-readable output
```

`<meeting>` is a path or a search term. The command never downgrades an existing High-confidence attribution, and it records a `speaker_mapping:` health block in the frontmatter (status `ok` / `empty` / `skipped`, plus the model, speaker/attendee counts, and timing) so a meeting that shipped anonymous is both greppable and re-runnable.

**Re-run the AI pass after editing a transcript.** If you've corrected transcription errors, deleted worthless sections, or struck sensitive content from a meeting file, the summary and derived frontmatter go stale. `minutes resummarize` re-runs just the summarization stage on the current transcript text — no audio reprocessing, no duplicate file:

```bash
minutes resummarize <meeting>                    # preview: show the regenerated summary + merge decisions
minutes resummarize <meeting> --apply            # splice the regenerated content into the file
minutes resummarize <meeting> --engine apple     # override the engine for this run
minutes resummarize <meeting> --apply --ingest   # also re-ingest into the knowledge base
minutes resummarize <meeting> --json             # machine-readable output
```

`<meeting>` is a path or a search term (memos and `minutes import text` files work too — for imported archives this is their first AI pass). The preview **does invoke the model** (on cloud engines the transcript leaves the machine), it just doesn't write. Only the AI-owned sections (`## Summary`, `## Decisions`, `## Action Items`, `## Open Questions`, `## Commitments`) and derived frontmatter are replaced — `## Notes`, `## Transcript`, `speaker_map`, and capture/consent metadata are never touched. Action items and decisions keep user-curated state (`status: done`, due dates, decision authority) by exact-identity matching; anything ambiguous is surfaced in the preview instead of silently resolved. A failed run (engine `none`, provider error, empty output) never modifies the file, and the previous version is backed up to a hidden `.<name>.pre-resummarize.<unix-secs>.bak` sibling on every apply; the newest 3 backups per artifact are kept. One redaction caveat: editing the transcript never touches the retained WAV — for true audio redaction, delete or re-record the audio (`minutes cleanup` / retention settings).

Because an apply rewrites summary-derived frontmatter, everything computed from it goes stale. A successful `--apply` therefore refreshes the derived views the same way the recording pipeline does: the relationship graph index is rebuilt, the vault copy re-synced (`strategy = "copy"` only), and the QMD collection reindexed if one is configured. Knowledge-base ingestion is **not** automatic — its chronological log is append-only, so re-ingesting the same meeting would add a duplicate entry every run; pass `--ingest` when you want it. All of this is best-effort: a refresh that fails warns and leaves the derived view stale, but never undoes a write that already succeeded.

**Privacy**: Voice enrollment is self-only (no enrolling others). Level 3 confirmed profiles require explicit opt-in per person. Voice embeddings are stored locally in `~/.minutes/voices.db` with 0600 permissions. Nothing leaves your machine.

> **Platform notes:** Calendar integration (auto-detecting meeting attendees) requires macOS. Screen context capture works on macOS and Linux. The voice memo pipeline works on all platforms — any folder sync (iCloud, Dropbox, Google Drive, Syncthing) can feed the watcher. The `minutes service install` auto-start command requires macOS (launchd); on Linux, use systemd or cron. Speaker diarization (`pyannote-rs`) works on all platforms (CLI, Tauri app, and via MCP). All other features — recording, transcription, search, action items, person profiles — work on all platforms.

### Desktop app

macOS source builds require the macOS 26 SDK from Xcode 26 or newer, including
when the resulting app will run on macOS 15. The older Xcode 16 command-line
tools do not include the Apple Speech declarations used by the build. Select
an installed Xcode 26 before building; for example, with Xcode 26.3 installed:

```bash
export DEVELOPER_DIR=/Applications/Xcode_26.3.app/Contents/Developer
xcrun --sdk macosx --show-sdk-version  # must report 26 or newer
```

```bash
# macOS — Homebrew cask (recommended)
brew install --cask silverstein/tap/minutes

# macOS — build from source
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo tauri build --bundles app --features parakeet,metal

# macOS — local desktop development with stable permissions
./scripts/install-dev-app.sh
```

The notarized Homebrew cask/update feed currently tracks the Apple Silicon desktop build. Intel Macs on macOS 15+ can still use the desktop app by building from source with the commands above.

```powershell
# Windows — build desktop installer from source
cargo install tauri-cli --version 2.10.1 --locked
cd tauri/src-tauri
cargo tauri build --ci --bundles nsis --no-sign
```

Tagged GitHub releases can include three Windows desktop assets: an NSIS installer as `minutes-desktop-windows-x64-setup.exe`, a portable archive as `minutes-desktop-windows-x64.zip`, and a raw desktop binary as `minutes-desktop-windows-x64.exe`. The installer is currently unsigned, so treat it as an advanced-user / preview distribution surface until Windows signing is added.

**Use the installer or the zip, not the raw binary.** The desktop app imports the Visual C++ runtime, which Windows does not include. The installer and the zip both place those runtime files beside the executable; the raw `.exe` does not carry them, so on a PC without the Visual C++ Redistributable it exits immediately with no message (#657).

The zip is the no-install option: unpack it anywhere and run `minutes-app.exe` from the extracted folder. Keep the runtime DLLs alongside it, since Windows resolves the application's own directory ahead of the system path and that is the whole reason the archive works.

The desktop app adds a system tray icon, recording controls, audio visualizer, Recall, and a meeting list window. The current Windows desktop build covers recording, transcription, search, settings, and Recall. Calendar suggestions, call detection, tray copy/paste automation, and the native dictation hotkey remain macOS-only for now.

Release workflow details live in:

- [macOS release workflow](release/platform-macos.md)
- [Windows release workflow](release/platform-windows.md)

For macOS development, use a dedicated signed dev app identity:

- Production app: `/Applications/Minutes.app` (`com.useminutes.desktop`)
- Development app: `~/Applications/Minutes Dev.app` (`com.useminutes.desktop.dev`)

If you are testing hotkeys, Screen Recording, Input Monitoring, or repeated macOS permission prompts, launch only `Minutes Dev.app` via `./scripts/install-dev-app.sh`. Avoid the repo symlink `./Minutes.app`, raw `target/` binaries, or ad-hoc local bundles for TCC-sensitive testing.

This repository is open source, so local development does not require the
maintainer's Apple signing credentials:

- `./scripts/install-dev-app.sh` works with ad-hoc signing by default
- for more stable macOS permission behavior across rebuilds, set
  `MINUTES_DEV_SIGNING_IDENTITY` to a consistent local codesigning identity
- release signing and notarization remain maintainer/release workflows

For dictation, the recommended path is the standard shortcut in the desktop app
(`Cmd/Ctrl + Shift + D` by default). The raw-key path for keys like `Caps Lock`
is available as an advanced option but remains more fragile and permission-heavy.

**Privacy:** All Minutes windows are hidden from screen sharing by default — other participants on Zoom/Meet/Teams won't see the app. Toggle via the tray menu: "Hide from Screen Share ✓".

### Troubleshooting

**No speech detected / blank audio:**
The most common cause is microphone permissions. Check System Settings → Privacy & Security → Microphone and ensure your terminal app (or Minutes.app) has access.

On Windows, a microphone can be present and permitted but still deliver only a
noise floor because of a vendor audio-effects layer. If enrollment reports
`no usable signal`, open **Settings → System → Sound → Input**, select the
microphone, and set **Audio enhancements** to **Off**. Also check the hardware
mute switch and the Windows input-volume control. Minutes now rejects this
signal before voice comparison so it will not misdiagnose a silent microphone
as multiple or inconsistent speakers.

**tmux users:** tmux server runs as a separate process that doesn't inherit your terminal's mic permission. Either run `minutes record` from a direct terminal window (not inside tmux), or use the Minutes.app desktop bundle which gets its own mic permission.

**Build fails with C++ errors on macOS 26+:**
whisper.cpp needs the SDK include path. Set `CXXFLAGS` as shown above before building.

**Dictation hotkey still fails after you enabled it in System Settings:**
The native hotkey uses macOS Input Monitoring, which is separate from Screen Recording. The fastest way to test the exact installed desktop identity is:

```bash
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app"
```

Use `./scripts/install-dev-app.sh` first so you are testing the stable development app identity rather than a raw `target/` build. The helper intentionally launches the app through LaunchServices; direct shell execution of `Contents/MacOS/minutes-app --diagnose-hotkey` can misreport TCC status.

### Updating

```bash
# macOS desktop app (Homebrew cask)
brew upgrade --cask silverstein/tap/minutes

# macOS CLI (Homebrew)
brew upgrade silverstein/tap/minutes

# From source (CLI)
git pull && cargo install --path crates/cli --features parakeet,metal

# From source (desktop app)
git pull
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo tauri build --bundles app --features parakeet,metal
# Then replace /Applications/Minutes.app with the new build from
# target/release/bundle/macos/Minutes.app

# GitHub release (desktop app)
# Download the latest .dmg from https://github.com/silverstein/minutes/releases
# and drag Minutes.app to /Applications, replacing the old version
```

For local source builds, keep the CLI and desktop app on the same transcription feature set. The repo build scripts now default to `MINUTES_BUILD_FEATURES=parakeet,metal`; override that env var only if you intentionally want a narrower build flavor.

Check your current version with `minutes --version` (CLI) or the Settings gear in the desktop app.
