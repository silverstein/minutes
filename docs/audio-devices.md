# Audio Device Setup Guide

Minutes captures audio from whatever input device is set as the system default (or configured in `config.toml`). Different scenarios require different setups.

## Quick reference

| Scenario | Input device | Setup needed |
|----------|-------------|--------------|
| In-person meeting | Built-in Microphone | None |
| External USB mic (Blue Yeti, Rode, etc.) | Your mic name | Plug in, select in System Settings > Sound > Input |
| Zoom / Meet / Teams call | BlackHole 2ch | Install BlackHole + Multi-Output Device (see below) |
| iPhone voice memo | N/A (file-based) | Configure folder watcher |

## Capturing system audio (Zoom, Meet, Teams)

macOS doesn't let apps record system audio directly. You need BlackHole, a free virtual audio device.

### 1. Install BlackHole

```bash
brew install blackhole-2ch
```

### 2. Create a Multi-Output Device

This lets you hear the call AND record it simultaneously:

1. Open **Audio MIDI Setup** (Spotlight → type "Audio MIDI Setup")
2. Click the **+** button at bottom-left → **Create Multi-Output Device**
3. Check both:
   - Your speakers/headphones (e.g., "MacBook Pro Speakers" or "AirPods Pro")
   - **BlackHole 2ch**
4. Make sure your speakers/headphones are listed FIRST (drag to reorder — the first device is what you hear)
5. Right-click the Multi-Output Device → **Use This Device For Sound Output**

### 3. Configure Minutes

Either set BlackHole as the system default input in System Settings > Sound > Input, or configure it in Minutes:

```toml
# ~/.config/minutes/config.toml
[capture]
device = "BlackHole 2ch"
```

Or pass it per-recording:
```bash
minutes record --device "BlackHole 2ch" --title "Team standup"
```

### 4. After the call

Switch your output back to normal speakers/headphones. The Multi-Output Device routes audio through both outputs, which can add slight latency for music/video.

## iPhone voice memos (auto-import)

Use the folder watcher to auto-process voice memos synced from iPhone via iCloud:

```toml
# ~/.config/minutes/config.toml
[watch]
input_dir = "~/Library/Mobile Documents/com~apple~Voicememos/Recordings/"
```

Then: `minutes watch`

Files are moved to `~/meetings/memos/processed/` after transcription.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Empty transcript | Wrong input device selected | Check System Settings → Sound → Input |
| Only your voice, not the call | BlackHole not set up | Follow the Multi-Output Device steps above |
| Echo in recording | Recording both mic + system audio | Mute your mic in the call app if you only want call audio |
| BlackHole disappeared | macOS update removed it | `brew reinstall blackhole-2ch` |
| Recording is mono/low quality | Source audio is compressed VoIP | Normal — VoIP audio is always lower quality than in-person |
| Notification sounds in transcript | System sounds captured via BlackHole | Turn on Do Not Disturb during recording |
