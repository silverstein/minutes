# Desktop Context Store

Minutes keeps conversation artifacts and desktop-context state in different
places on purpose:

- `~/meetings/*.md` remains the durable source of truth for meetings and memos
- `~/.minutes/context.db` stores local desktop-context sessions, events, and
  artifact links

That split is the contract. If `context.db` disappears, Minutes still has the
meeting and memo corpus. Only the adjacent desktop-context index is lost.

## What lives in `context.db`

`context.db` is a local SQLite sidecar with three first-class tables:

- `context_sessions`
  one row per recording, memo-adjacent capture window, standalone live
  transcript session, or future focus session
- `context_events`
  timestamped app/window/browser/screenshot references keyed directly by
  `session_id`
- `context_links`
  one-to-many links from a session to supporting artifacts such as the queued
  job id, moved capture audio, final markdown artifact, or live transcript
  JSONL/WAV

The schema is intentionally narrow:

- raw desktop events do **not** go into `graph.db`
- markdown does **not** move into SQLite
- event rows link to sessions directly instead of requiring a second join table

## Session lifecycle

Minutes now creates a stable context session id at capture start and threads it
through the local lifecycle:

1. recording or standalone live transcript starts
2. a `context_sessions` row is created with state `active`
3. recording metadata / job state carries the `context_session_id`
4. background processing promotes the session to `processing`
5. the final markdown artifact or live JSONL/WAV is linked back to the session
6. the session ends in `complete`, `failed`, or `discarded`

Current linkage paths:

- recordings and quick thoughts
  `recording-meta.json` -> queued job json -> final markdown/audio links
- standalone live transcript
  live runtime -> `live-transcript-status.json` -> final JSONL/WAV links

## Stable query shapes

The storage slice is designed around the three queries the next beads need:

- by session id
  give me the context events and linked artifacts for this recording/live
  session
- by linked artifact
  find the context session for this meeting markdown path or live JSONL path
- by timestamp window
  list context events that happened around a decision, memo, or session

Those are available through the core store helpers before any MCP or Recall UX
lands.

## Supportability

- `context.db` uses WAL mode and 0600 file permissions like the other local
  sidecar stores
- deleting `context.db` does not damage meeting markdown or `graph.db`
- `graph.db` remains the rebuildable conversation graph
- `context.db` is not treated as a cross-device sync layer in this slice

This keeps Minutes file-native where it matters while still giving the desktop
context collector and retrieval surfaces a stable local contract.
