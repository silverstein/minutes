# minutes

[![Crates.io](https://img.shields.io/crates/v/minutes-cli)](https://crates.io/crates/minutes-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/silverstein/minutes/blob/main/LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/silverstein/minutes?style=social)](https://github.com/silverstein/minutes)

**Open-source conversation memory for AI assistants.** Record meetings, capture voice memos, search everything — your AI remembers every conversation you've had.

[Website](https://useminutes.app) | [GitHub](https://github.com/silverstein/minutes) | [MCP Server](https://www.npmjs.com/package/minutes-mcp) | [Desktop App](https://github.com/silverstein/minutes/releases)

## Install

```bash
cargo install minutes-cli
minutes setup --model tiny    # Download whisper model (75MB)
```

Or via Homebrew on macOS:

```bash
brew tap silverstein/tap && brew install minutes
```

## Quick start

```bash
minutes record                # Start recording
minutes stop                  # Stop and transcribe
minutes search "pricing"      # Search across all meetings
minutes actions               # Open action items
```

## What it does

- **Record** meetings and voice memos from your microphone
- **Transcribe** locally with whisper.cpp (Apple Silicon optimized)
- **Diarize** speakers with pyannote-rs (native Rust, no Python)
- **Extract** action items, decisions, and people into structured YAML frontmatter
- **Search** across all conversations by keyword, person, or topic
- **Track** commitments and relationships across meetings
- **Sync** voice memos from your phone via iCloud/Dropbox/any folder sync
- **Integrate** with Claude, Cursor, Windsurf, Obsidian via MCP

## Key commands

| Command | Description |
|---------|-------------|
| `minutes record` | Start recording from microphone |
| `minutes stop` | Stop recording and transcribe |
| `minutes process <file>` | Transcribe an audio file |
| `minutes search <query>` | Full-text search across meetings |
| `minutes actions` | List open action items |
| `minutes people` | Relationship intelligence |
| `minutes commitments` | Track what you promised who |
| `minutes watch` | Auto-process voice memos from a folder |
| `minutes dictate` | Speak-to-text (clipboard + daily note) |
| `minutes health` | System diagnostics |
| `minutes setup` | Download models and configure |

## Output format

Meetings save as markdown with structured YAML frontmatter:

```yaml
---
title: Q2 Pricing Discussion
date: 2026-03-17T14:00:00
duration: 42m
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
decisions:
  - text: Run pricing experiment with monthly billing
---
```

Works with Obsidian, Logseq, grep, or any markdown tool.

## Features

| Feature | Details |
|---------|---------|
| Transcription | whisper.cpp, local, multiple model sizes |
| Speaker diarization | pyannote-rs (native Rust, ~34MB models) |
| Voice activity detection | Silero VAD (prevents hallucination loops) |
| Audio formats | wav directly; m4a, mp3, ogg, webm, mp4, mov, and aac with ffmpeg |
| GPU acceleration | Metal, CoreML (macOS), CUDA (Linux/Windows), ROCm/HIP, Vulkan |
| Phone voice memos | Folder watcher + iCloud/Dropbox/Syncthing |
| MCP server | 15 tools + 7 resources for Claude/Cursor/Windsurf |
| Desktop app | Tauri v2 menu bar app (macOS, Windows) |
| Privacy | Everything local, 0600 permissions on output |

## Claude / MCP integration

No API keys needed — Claude reads your meetings via MCP tools.

```bash
# MCP server (no Rust required)
npx minutes-mcp
```

```
You: "What did Alex say about pricing?"
Claude: [searches meetings] → synthesizes answer from transcripts
```

## GPU acceleration

| Backend | Platform | Feature flag | Prerequisites |
|---------|----------|-------------|---------------|
| Metal | macOS | `metal` | Xcode Command Line Tools |
| CoreML | macOS | `coreml` | Xcode Command Line Tools |
| CUDA | Windows/Linux | `cuda` | [CUDA Toolkit](https://developer.nvidia.com/cuda-toolkit) |
| ROCm/HIP | Linux | `hipblas` | [ROCm](https://rocm.docs.amd.com/) 6.1+ (`hipcc`, `hipblas`, `rocblas`) |
| Vulkan | Windows/Linux | `vulkan` | [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) (+ `vulkan-headers` on Arch) |

Metal is the only backend that is exercised daily by the maintainer. CUDA, ROCm/HIP,
and Vulkan should be considered experimental: they wire through to whisper.cpp via
whisper-rs and are expected to work, but have not been validated in CI.

```bash
cargo install minutes-cli --features metal    # macOS Metal
cargo install minutes-cli --features coreml   # macOS Neural Engine
cargo install minutes-cli --features cuda     # NVIDIA CUDA
cargo install minutes-cli --features hipblas  # AMD ROCm/HIP (experimental)
cargo install minutes-cli --features vulkan   # Vulkan (experimental)
```

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

## Links

- **Website**: [useminutes.app](https://useminutes.app)
- **GitHub**: [github.com/silverstein/minutes](https://github.com/silverstein/minutes)
- **MCP Server**: [npmjs.com/package/minutes-mcp](https://www.npmjs.com/package/minutes-mcp)
- **Desktop App**: [GitHub Releases](https://github.com/silverstein/minutes/releases)
- **Claude Code Plugin**: `claude plugin marketplace add silverstein/minutes`

## License

MIT — Built by [Mat Silverstein](https://github.com/silverstein), founder of [X1 Wealth](https://x1wealth.com)
