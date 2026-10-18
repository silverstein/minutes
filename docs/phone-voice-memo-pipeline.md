# Phone → desktop voice memo pipeline

No phone app needed. Record a thought on your phone, and it becomes searchable memory on your desktop. Claude even surfaces recent memos proactively — "you had a voice memo about pricing yesterday."

The watcher is folder-agnostic — it processes any audio file that lands in a watched folder. Pick the sync method that matches your setup:

| Phone | Desktop | Sync method |
|-------|---------|-------------|
| **iPhone** | **Mac** | iCloud Drive (built-in, ~5-30s) |
| **iPhone** | **Windows/Linux** | iCloud for Windows, or Dropbox/Google Drive |
| **Android** | **Any** | Dropbox, Google Drive, Syncthing, or any folder sync |
| **Any** | **Any** | AirDrop, USB, email — drop the file in the watched folder |

### Setup (one-time)

**Step 1: Create a sync folder** — pick one that syncs between your phone and desktop:

```bash
# macOS + iPhone (iCloud Drive)
mkdir -p ~/Library/Mobile\ Documents/com~apple~CloudDocs/minutes-inbox

# Any platform (Dropbox)
mkdir -p ~/Dropbox/minutes-inbox

# Any platform (Google Drive)
mkdir -p ~/Google\ Drive/minutes-inbox

# Or just use the default inbox (manually drop files into it)
# ~/.minutes/inbox/  ← already exists
```

**Step 2: Add the sync folder to your watch config** in your config file (`$XDG_CONFIG_HOME/minutes/config.toml` when `XDG_CONFIG_HOME` is set, otherwise `~/.config/minutes/config.toml`):

```toml
[watch]
paths = [
  "~/.minutes/inbox",
  # Add your sync folder here — uncomment one:
  # "~/Library/Mobile Documents/com~apple~CloudDocs/minutes-inbox",  # iCloud
  # "~/Dropbox/minutes-inbox",                                       # Dropbox
  # "~/Google Drive/minutes-inbox",                                  # Google Drive
]
```

**Step 3: Set up your phone**

<details>
<summary><strong>iPhone (Apple Shortcuts)</strong></summary>

1. Open the **Shortcuts** app on your iPhone
2. Tap **+** → Add Action → search **"Save File"**
3. Set destination to `iCloud Drive/minutes-inbox/` (or your Dropbox/Google Drive folder)
4. Turn OFF "Ask Where to Save"
5. Tap the **(i)** info button → enable **Share Sheet** → set to accept **Audio**
6. Name it **"Save to Minutes"**

Now: Voice Memos → Share → **Save to Minutes** → done.
</details>

<details>
<summary><strong>Android</strong></summary>

Use any voice recorder app + your cloud sync of choice:

- **Dropbox**: Record with any app → Share → Save to Dropbox → `minutes-inbox/`
- **Google Drive**: Record → Share → Save to Drive → `minutes-inbox/`
- **Syncthing** (no cloud): Set up a Syncthing share between phone and desktop pointing at your watched folder. Fully local, no cloud.
- **Tasker/Automate** (power users): Auto-move new recordings from your recorder app to the sync folder.
</details>

<details>
<summary><strong>Manual (any phone)</strong></summary>

No sync setup needed — just get the audio file to your desktop's watched folder:
- **AirDrop** (Apple): Share → AirDrop to Mac → move to `~/.minutes/inbox/`
- **Email**: Email the recording to yourself → save attachment to watched folder
- **USB**: Transfer directly
</details>

**Step 4: Start the watcher** (or install as a background service):

```bash
minutes watch                  # Run in foreground
minutes service install        # Install all background services (macOS launchd / Linux systemd)
minutes service status         # Check what's running
minutes service restart        # Restart all services (e.g. after upgrading the binary)
```

`minutes service install` sets up three agents:

| Agent | Schedule | What it does |
|-------|----------|--------------|
| **watcher** | Always on | Processes voice memos from `~/.minutes/inbox/` |
| **weekly-summary** | Sundays 7pm | Generates a weekly digest to `~/.minutes/automations/` |
| **proactive-context** | Daily 8am | Builds a context bundle from recent meetings, memos, and live stale commitments |

> **Upgrading?** `minutes service install` is idempotent. Re-running it after a binary
> upgrade rewrites all plists/units and reloads with the new binary path.

### Sync-folder reliability

The watcher subscribes before its initial scan and periodically reconciles each configured folder, so a missed native filesystem event or a file that is still syncing gets another chance. Small folders are checked every 30 seconds while the watcher is available. Larger backlogs are scanned in bounded pages, with processing continuing between pages. Native events still trigger immediate admission checks.

Repeated events and scan overlap are deduplicated before processing. The same stability, extension, container and source-ownership checks apply to both routes. The watcher skips `.icloud` placeholders and symlinks, and does not recurse into `processed/` or `failed/`. Successfully processed sources are archived through the existing move path. New arrivals wait while a previous processing batch is running.

If manual `minutes process` works but an arrival remains untouched, verify the watcher service is using the expected version and config path. Keep the original and collect the watcher log. A running service alone does not prove it has admitted that file. Real iCloud/FileProvider verification is separate from the cross-platform reconciliation tests.

### How it works

```
Phone (any)                   Desktop (any)
───────────                   ─────────────
Record voice memo        →    Cloud sync / manual transfer
Share to sync folder               │
                                   ▼
                            minutes watch detects file
                                   │
                            probe duration (<2 min?)
                              ├── yes → memo pipeline (fast, no diarization)
                              └── no  → meeting pipeline (full)
                                   │
                            transcribe → save markdown
                                   │
                            ├── event: VoiceMemoProcessed
                            ├── daily note backlink
                            └── surfaces in next Claude session
```

Short voice memos (<2 minutes) automatically route through the fast memo pipeline — no diarization, no heavy summarization. Long recordings get the full meeting treatment. The threshold is configurable: `dictation_threshold_secs = 120` in `[watch]`.

### Optional: sidecar metadata

If your phone workflow also saves a `.json` file alongside the audio (same name, `.json` extension), Minutes reads it for enriched metadata:

```json
{"device": "iPhone", "source": "voice-memos", "captured_at": "2026-03-24T08:41:00-07:00"}
```

This adds `device` and `captured_at` to the meeting's frontmatter. Works with any automation tool (Apple Shortcuts, Tasker, etc.).

Supports `.m4a`, `.mp3`, `.wav`, `.ogg`, `.webm`. WAV is decoded in process. Compressed formats are decoded inside a bounded child process, using [ffmpeg](https://ffmpeg.org/) when it is installed and a bundled decoder otherwise. The bundled decoder covers AAC, MP3, Vorbis and FLAC; ffmpeg is required for Opus, which is what `.webm` browser recordings and `.ogg` voice notes normally contain, and for ALAC in `.m4a`.

If a desktop call capture leaves a raw file under `~/.minutes/native-captures/`, process that audio file directly with `minutes process <path> --type meeting`. For compatibility, `minutes import <audio-file>` also routes to the same meeting-processing path; `minutes import granola` remains the Granola history importer.

### Vault sync (Obsidian / Logseq)

```bash
minutes vault setup              # Auto-detect vaults, configure sync
minutes vault status             # Check health
minutes vault sync               # Copy existing meetings to vault
```

Three strategies: **symlink** (zero-copy), **copy** (works with iCloud/Obsidian Sync), **direct** (write to vault). `minutes vault setup` detects your vault and recommends the right strategy automatically.
