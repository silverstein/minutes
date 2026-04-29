# RFC 0003: Agent Event Bus v0 Contract

Status: accepted as the v0 contract baseline for GitHub #194.

This document freezes the current Agent Event Bus contract before more event
emitters are added. The goal is to keep the shipped CLI/MCP bus compatible
while making the remaining #194 work explicit.

## Current Shipped Baseline

Minutes already ships the durable bus plumbing:

- append-only JSONL at `~/.minutes/events.jsonl`
- monotonic `seq` assigned under the writer lock
- `events.seq` sidecar cursor for O(1) normal appends
- `minutes events --follow --since-seq N`
- MCP `minutes://events/live` and `minutes://events/live{?since_seq,limit}`
- MCP `resources/subscribe` with `notifications/resources/updated`
- `live.utterance.final` emission from the live transcript writer

## Wire Envelope

The v0 wire format is a flat JSON object, not a nested event object:

```jsonc
{
  "v": 1,
  "seq": 4826,
  "timestamp": "2026-04-29T08:30:00-07:00",
  "event_type": "live.utterance.final",
  "... event-specific fields ...": "..."
}
```

This shape is the canonical v0 persistence and streaming contract for CLI and
MCP consumers.

Compatibility rules:

- `v` defaults to `1` when absent so older log lines still read.
- `seq` defaults to `0` when absent and is repaired while reading legacy logs.
- `timestamp` remains the v0 field name. Do not introduce a top-level `ts`
  synonym in v0.
- `event_type` is the event discriminator.
- Event-specific payload fields remain flattened at top level.
- Do not add a top-level `event`, `kind`, `source`, `confidence`, or
  `provenance` envelope field in v0. Those concepts belong inside typed event
  payloads when needed.

Why not migrate to the original #194 top-level envelope immediately?

The current flat shape is already shipped through the CLI and MCP live resource.
Changing the persisted JSONL shape now would force a compatibility layer before
the remaining emitters are even implemented. v0 therefore freezes the shipped
shape and reserves a future v2 envelope for a cleaner nested/projection model if
real subscribers prove that need.

## Event Taxonomy

The v0 taxonomy has two buckets: shipped legacy names that must keep reading,
and dotted agent-facing names used by new or normalized events.

| Event | v0 status | Notes |
| --- | --- | --- |
| `live.utterance.final` | canonical, shipped | Emitted by live transcript writer. Legacy `LiveUtteranceFinal` is accepted as an alias. |
| `recording.completed` | canonical, shipped | Emitted by recording completion paths. Legacy `RecordingCompleted` is accepted as an alias. |
| `meeting.insight.detected` | canonical, shipped | Emitted by semantic insight extraction. Legacy `MeetingInsightExtracted` is accepted as an alias. |
| `recording.started` | canonical, shipped | Emitted after capture/live/dictation/watch processing actually starts. |
| `transcript.delta` | punted from v0 | Partial streaming revisions are high-volume and model-specific. Keep v0 on final utterances; revisit deltas in a gated v1 design. |
| `agent.annotation` | canonical, shipped | Append-only attributed agent commentary. Gated by `~/.minutes/agents.allow`; never mutates human-authored meeting markdown/frontmatter. |

Existing internal or legacy events such as `AudioProcessed`, `WatchProcessed`,
`NoteAdded`, `VaultSynced`, `VoiceMemoProcessed`, `DeviceChanged`,
`KnowledgeUpdated`, `MicMuted`, and `MicUnmuted` remain valid log entries.
They are not part of the #194 v0 agent contract unless a later bead promotes
them into the dotted taxonomy.

## `transcript.delta` Decision

`transcript.delta` is intentionally out of v0.

The two live transcript hot paths can produce partial streaming revisions while
someone is still speaking. Those revisions are useful for future mid-utterance
coaching, but they are also noisy:

- a single utterance can produce many revised partials before the final text
- Whisper/Parakeet/Apple Speech do not expose identical partial semantics
- high-volume partial events would make `events.jsonl` noisier for local tails
- consumers need revision IDs or replacement semantics to avoid treating draft
  tokens as stable speech

For v0, `live.utterance.final` is the stable live transcript event. A future v1
can add `transcript.delta` behind an explicit config flag with volume limits,
revision IDs, and host compatibility tests.

## Agent Annotation Discipline

`agent.annotation` is the only v0 write-back event for agents. It is commentary
about a meeting or transcript span, not a rewrite of the human-authored meeting
record.

Rules:

- Agents never mutate meeting markdown or frontmatter.
- Writes append a new `agent.annotation` event to `~/.minutes/events.jsonl`.
- Each annotation carries `agent`, `subkind`, `target`, `body`, `citations`,
  `confidence`, and `provenance`.
- The write path is default-deny unless the agent ID is present in
  `~/.minutes/agents.allow`.
- Rejected writes return a structured error with `error`, `agent_id`,
  `event_type`, and `allowlist_path`.

Allowlist format:

```text
# one agent_id per line
codex
review-agent: agent.annotation
workflow-agent: agent.annotation, meeting.insight.detected
```

A bare `agent_id` allows all current agent event writes for that ID. Scoped
lines allow only the comma-separated event types after `:`.

## Closure Rules For #194

GitHub #194 should not close merely because the bus plumbing exists. It can
close only after:

1. This envelope/taxonomy contract is tested and referenced from the issue.
2. Recording lifecycle events are implemented or explicitly scoped.
3. Live transcript delta and semantic insight behavior is settled.
4. Agent annotation write-back is implemented with an allowlist or split out
   with a clear rationale.
5. MCP subscription behavior is tested against real hosts where available.
6. An adversarial review finds no remaining P1/P2 contract gaps.
