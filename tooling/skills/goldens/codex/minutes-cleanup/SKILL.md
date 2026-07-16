---
name: minutes-cleanup
description: Manage old recordings — find large files, archive old meetings, delete processed originals. Use when the user says "clean up recordings", "how much space are meetings using", "delete old recordings", "archive meetings", "manage meeting storage", or asks about disk space from minutes.
---

# /minutes-cleanup

Help the user manage disk space and organize old recordings. Minutes is
transcript-first: markdown notes and structured memory are durable, while raw
audio is a temporary recovery/reprocessing layer unless pinned.

## Check current usage

```bash
minutes storage
minutes storage --json
```

Present this to the user before taking any action.

## Common cleanup tasks

### Preview raw-audio cleanup

After transcription, the original audio files are no longer needed for search or
recap. They only matter if you want to re-transcribe with a better model or
debug recovery. Cleanup is preview-only unless `--apply` is passed.

```bash
# Use the configured retention policy
minutes cleanup

# Try a shorter successful-audio window
minutes cleanup --older-than 14d

# Machine-readable preview for agents
minutes cleanup --json
```

### Delete raw audio candidates

Only apply cleanup after showing the preview and getting explicit confirmation.

```bash
minutes cleanup --apply
minutes cleanup --older-than 14d --apply
```

### Archive old meetings

Move meetings older than N days to an archive folder:

```bash
mkdir -p ~/meetings/archive

# Find meetings older than 90 days
find ~/meetings -maxdepth 1 -name "*.md" -mtime +90

# Move them (confirm with user first)
find ~/meetings -maxdepth 1 -name "*.md" -mtime +90 -exec mv {} ~/meetings/archive/ \;
```

Archived meetings won't appear in `minutes list` or `minutes search` (which only scans `~/meetings/`), but they're still on disk if needed.

### Clean up processed voice memos

The watcher moves originals to `~/meetings/memos/processed/` after transcription:

```bash
du -sh ~/meetings/memos/processed/ 2>/dev/null
```

### Clean up stale state

```bash
# Remove stale PID file
rm -f ~/.minutes/recording.pid

# Clean old logs (keep last 7 days)
find ~/.minutes/logs -name "*.log" -mtime +7 -delete 2>/dev/null

# Remove last-result.json (transient)
rm -f ~/.minutes/last-result.json
```

## Gotchas

- **Never delete `.md` files without asking** — These are the transcripts. They're small and contain the actual value. WAV files are the space hogs.
- **Prefer `minutes cleanup` over raw `find -delete`** — The CLI understands pinned audio and sidecar stems.
- **Archived meetings are invisible to search** — `minutes search` only walks `~/meetings/` and `~/meetings/memos/`. If you need archived meetings searchable, configure QMD to index `~/meetings/archive/` too.
- **Audio deletion is irreversible** — If the user might want to re-transcribe with a better model later, suggest pinning important recordings and only deleting old unpinned candidates.
- **Pin exceptions in frontmatter** — Add `audio_retention: pinned` to a meeting to keep its raw audio out of cleanup candidates.
- **Audio is ~10 MB/minute, transcripts are ~1 KB/minute** — Deleting audio saves 99%+ of space while keeping all searchable content.
- **iCloud sync caveat** — If `~/meetings/` is in an iCloud-synced folder, deleted files go to "Recently Deleted" and still count against storage for 30 days.

