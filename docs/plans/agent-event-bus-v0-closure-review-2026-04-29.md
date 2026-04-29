# Agent Event Bus v0 Closure Review - 2026-04-29

Scope: GitHub #194 / `minutes-l5sa.6`.

Verdict: close #194 after landing the `.6` follow-filter fix. No remaining P1/P2
contract gaps were found in the v0 contract as frozen by RFC 0003.

## Findings

### Fixed In This Review

`minutes events --follow --event-type X` could miss matching historical events
during its initial backfill because it read the last `N` total events and only
then filtered. Non-follow reads already read all events before filtering. The
follow path now uses the same behavior when `--event-type` is present, then
applies the requested limit after filtering.

Severity before fix: P2 for subscriber ergonomics, because a filtered follower
could start without seeing the latest matching annotation/semantic event even
though it existed in the log.

### Clean Checks

- Envelope/taxonomy: RFC 0003 freezes the v0 flat JSONL contract:
  `v`, `seq`, `timestamp`, `event_type`, plus flattened payload fields.
- Backward compatibility: legacy missing `v`/`seq` reads are repaired; dotted
  aliases read for `recording.completed`, `meeting.insight.detected`, and
  `live.utterance.final`.
- Recording lifecycle: `recording.started` emits after the real capture/live/
  dictation/watch start points; `recording.completed` serializes as the dotted
  v0 name.
- Transcript/semantic events: `live.utterance.final` is the v0 live transcript
  event; `transcript.delta` is deliberately out of v0 with rationale; semantic
  insights serialize as `meeting.insight.detected`.
- Agent write-back: `agent.annotation` is append-only, attributed, validated,
  and default-deny gated by `~/.minutes/agents.allow`; writes do not touch
  meeting markdown/frontmatter.
- CLI/MCP tailing: `minutes events --since-seq`, `--follow`, and
  `--event-type` cover durable local reads; MCP `minutes://events/live` supports
  subscribe/update/read/reconnect.
- Host compatibility: host-config smoke passes for local Claude Desktop and
  Codex CLI configurations; OpenCode/Cursor/Cline gaps are documented rather
  than hidden.

## Residual Non-Blocking Gaps

- Original issue prose included a hypothetical top-level `ts/session_id/kind`
  envelope. RFC 0003 intentionally rejects that for v0 to preserve shipped
  compatibility; this is a documented decision, not an open implementation gap.
- `transcript.delta` remains valuable for future mid-utterance coaching, but it
  needs revision IDs, replacement semantics, volume limits, and explicit gating.
  It belongs in v1.
- Host proof is a host-config protocol smoke, not GUI click-through in Claude
  Desktop. It still exercises the exact stdio command those hosts are configured
  to launch and validates the MCP primitives required by #194.

## Evidence

- RFC: `docs/rfcs/0003-agent-event-bus-v0.md`
- Host proof: `docs/plans/mcp-live-events-host-compat-2026-04-29.md`
- Host smoke harness: `crates/mcp/test/live_events_host_compat.mjs`
- Core event contract tests: `cargo test -p minutes-core events::tests -- --nocapture`
- MCP subscription tests: `pnpm --filter minutes-mcp test:unit`
- Host-config smoke: `pnpm --filter minutes-mcp run test:host-compat`
