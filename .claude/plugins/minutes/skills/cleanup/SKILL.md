---
name: minutes-cleanup
description: Manage old recordings — find large files, archive old meetings, delete processed originals. Use when the user says "clean up recordings", "how much space are meetings using", "delete old recordings", "archive meetings", "manage meeting storage", or asks about disk space from minutes.
user_invocable: true
---

# /minutes cleanup

Help the user manage disk space and organize old recordings.

## Check current usage

```bash
# Total disk usage for meetings
du -sh ~/meetings/

# Breakdown: audio vs transcripts
echo "Audio files:"; find ~/meetings -name "*.wav" -exec du -ch {} + 2>/dev/null | tail -1
echo "Transcripts:"; find ~/meetings -name "*.md" -exec du -ch {} + 2>/dev/null | tail -1

# Count by type
echo "Meetings: $(find ~/meetings -maxdepth 1 -name '*.md' | wc -l | tr -d ' ')"
echo "Memos: $(find ~/meetings/memos -maxdepth 1 -name '*.md' 2>/dev/null | wc -l | tr -d ' ')"

# Largest files
find ~/meetings -type f -exec du -sh {} \; | sort -rh | head -10
```

Present this to the user before taking any action.

## Common cleanup tasks

### Delete WAV originals (biggest space saver)

After transcription, the original `.wav` files are no longer needed for search or recap. They only matter if you want to re-transcribe with a better model.

```bash
# List WAV files and their sizes
find ~/meetings -name "*.wav" -exec du -sh {} \;

# Delete them (ask user for confirmation first!)
find ~/meetings -name "*.wav" -delete
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
- **Archived meetings are invisible to search** — `minutes search` only walks `~/meetings/` and `~/meetings/memos/`. If you need archived meetings searchable, configure QMD to index `~/meetings/archive/` too.
- **WAV deletion is irreversible** — If the user might want to re-transcribe with a better model later, suggest keeping WAVs for recent recordings and only deleting old ones.
- **Audio is ~10 MB/minute, transcripts are ~1 KB/minute** — Deleting audio saves 99%+ of space while keeping all searchable content.
- **iCloud sync caveat** — If `~/meetings/` is in an iCloud-synced folder, deleted files go to "Recently Deleted" and still count against storage for 30 days.
