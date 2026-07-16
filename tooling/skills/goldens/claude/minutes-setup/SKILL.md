---
name: minutes-setup
description: Guided first-time setup for Minutes — download whisper model, create directories, configure audio input. Use when the user says "set up minutes", "install minutes", "first time setup", "configure minutes", "get started with minutes", "how do I start using minutes", or when verify shows missing components.
user_invocable: true
---

# /minutes-setup

Walk the user through first-time Minutes setup, step by step.

## Setup steps

### 1. Check current state first

Run the verify skill's script to see what's already done:
```bash
bash "${CLAUDE_PLUGIN_ROOT}/skills/minutes-verify/scripts/verify-setup.sh"
```

Skip any steps that already pass.

### 2. Build the binary (if needed)

```bash
cd ~/Sites/minutes
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build --release
```

The binary lands at `target/release/minutes`. The user should add it to their PATH or create a symlink.

### 3. Download a whisper model

Ask the user which quality level they want using AskUserQuestion:

| Model | Size | Speed | Quality | Best for |
|-------|------|-------|---------|----------|
| `tiny` | 75 MB | ~10x real-time | Low | Quick tests, short memos |
| `small` | 466 MB | ~4x real-time | Good | Daily meetings (recommended) |
| `medium` | 1.5 GB | ~2x real-time | Great | Important meetings, accents |
| `large-v3` | 3.1 GB | ~1x real-time | Best | Legal, medical, foreign language |

Then run:
```bash
minutes setup --model <chosen-model>
```

### 4. Create directories

```bash
mkdir -p ~/meetings/memos
```

### 5. Audio input (if recording calls)

For in-person conversations, the built-in mic works fine. For Zoom/Meet/Teams:

1. Install BlackHole: `brew install blackhole-2ch`
2. Open Audio MIDI Setup (Spotlight → "Audio MIDI Setup")
3. Create a Multi-Output Device combining speakers + BlackHole
4. Set the Multi-Output Device as system output
5. Set BlackHole as Minutes' input (or system default input)

See `minutes-record/references/audio-devices.md` for the full guide.

### 6. Verify

Run verify again to confirm everything passes:
```bash
bash "${CLAUDE_PLUGIN_ROOT}/skills/minutes-verify/scripts/verify-setup.sh"
```

### 7. Test recording

```bash
minutes record --title "Test recording"
# Speak for 10-15 seconds
minutes stop
```

Check the output file exists in `~/meetings/` and has a transcript.

## Gotchas

- **macOS 26 (Tahoe) requires CXXFLAGS** — The whisper.cpp build needs the C++ include path set explicitly. This is a known Apple SDK issue.
- **First model download can be slow** — The `small` model is 466 MB. On slow connections, `tiny` is a good starting point (75 MB).
- **BlackHole setup is the hardest part** — Most users struggle with the Audio MIDI Setup step. Offer to walk through it if they get stuck.

