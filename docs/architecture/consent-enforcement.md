# Consent Enforcement — Restricted Meetings at the Agent Layer

Wave 2 of the consent layer (bead `minutes-3yub.4`). Wave 1 introduced the
designation — meetings can carry `capture: none` and `sensitivity: restricted`
frontmatter (see [frontmatter-schema.md](frontmatter-schema.md)). Wave 2 makes
the designation an enforcement contract: a restricted meeting is **excluded by
default from every agent surface**, with an explicit, logged override where an
override exists at all. The human-readable markdown on disk is never touched —
the operator's own files stay fully readable.

## The contract

| Surface | Default | Override | Override logging |
|---|---|---|---|
| Core text search (`search`, `search_with_mode`) | excluded | `SearchFilters::include_restricted` | caller's responsibility (CLI does it) |
| Core intent search (`search_intents`) | excluded | `SearchFilters::include_restricted` | caller's responsibility (CLI does it) |
| Core open actions (`find_open_actions`) | excluded | `include_restricted` parameter | caller's responsibility (CLI does it) |
| Core cross-meeting research (`cross_meeting_research`) | excluded | `SearchFilters::include_restricted` | caller's responsibility (CLI does it) |
| Core consistency report (`consistency_report`) | excluded | none this wave | n/a |
| Core person profile (`person_profile`) | excluded | none this wave | n/a |
| Knowledge graph rebuild (`graph.rs`) | excluded | none this wave | n/a — graph.db wipes on rebuild, so exclusion at build time is complete |
| Knowledge ingest (`knowledge.rs`, `minutes ingest`) | skipped in batch; explicit ingest of a restricted meeting is refused with a message | none this wave | n/a |
| CLI `search` / `list` / `actions` / `research` | excluded | `--include-restricted` | `sensitivity.override` event appended before results are returned |
| SDK reader (`crates/sdk/src/reader.ts`: list, search, actions, decisions, person profiles, voice memos) | excluded | `includeRestricted` option | stderr warning naming count + surface |
| SDK reader `getMeeting` by exact path | minimal stub (title, date, `sensitivity: restricted`, note) — never the body | `includeRestricted` option | stderr warning |
| MCP tools (`list_meetings`, `search_meetings`, `get_meeting`, `research_topic`, `get_person_profile`) | excluded / stub | `include_restricted` parameter | via the CLI's `sensitivity.override` event on CLI-backed paths; server log + `sensitivity_override` response note where the server cannot write events |
| MCP graph-backed tools (`track_commitments`, `relationship_map`, `get_person_profile` graph path) | excluded | none this wave (restricted facts never enter the graph) | n/a |

Desktop app search, list, palette actions, and other in-app navigation are the
**operator's own surface**, not an agent surface: restricted meetings stay
visible to the human in their own app. Assistant-facing context builders in
the desktop app (assistant workspace context, proactive context bundle, the
automation weekly summary) follow the agent-surface default and exclude
restricted meetings.

## Override logging

The override is never silent. When `--include-restricted` is passed to a CLI
read command, the CLI appends a `sensitivity.override` event to the
append-only event log (`~/.minutes/events.jsonl`) **before returning
results**:

```json
{"v":1,"seq":42,"timestamp":"...","event_type":"sensitivity.override","surface":"cli.search","query":"pricing"}
```

- `surface` — the read surface that honored the override (`cli.search`,
  `cli.actions`, `cli.research`; `cli.list` routes through `cli.search` with
  an empty query).
- `query` — the query or filter context supplied by the caller, omitted when
  there is none.

The append is best-effort by design: a failed append warns on stderr but
never blocks the caller (the never-block-non-interactive-callers rule from
v1). MCP tools that route through the CLI inherit this event. The MCP server
itself has no event-bus writer; where a tool serves an override without a CLI
round-trip (`get_meeting`), it records the override in the server log and
flags it in the structured response (`sensitivity_override`).

## Restricted stub (get-by-path)

Knowing a restricted meeting's path is not a bypass. `getMeeting` in the SDK
reader and the MCP `get_meeting` tool return a minimal stub without the
override:

- title, date, `sensitivity: restricted`
- a note that content is excluded by default and the `include_restricted`
  parameter is required
- never the transcript body, action items, decisions, or attendees

The SDK stub is marked with `restricted_stub: true` so callers can tell it
apart from a full meeting.

## No override surfaces (this wave)

The knowledge graph, knowledge ingest, consistency reports, and person
profiles have **no override**: restricted facts simply do not enter those
derived stores or reports. `graph.db` wipes on rebuild, so graph exclusion at
build time is complete. An explicitly named restricted meeting passed to
`minutes ingest` is refused with a message naming the designation; batch
ingest (`--all`) skips restricted meetings and reports the skipped count.

## Compatibility notes

- All changes are additive: `sensitivity` absent means normal behavior
  everywhere, and existing corpora are unaffected.
- Agents never write `sensitivity` (RFC #194 discipline) — the designation is
  set by the human-initiated flows from Wave 1.
- The MCP server compiles against the published `minutes-sdk` typings, which
  may lag the in-repo reader. Until an SDK release that includes the
  `includeRestricted` option is published and bundled, the pure-TS fallback
  paths pass the option through shims (older SDKs ignore it) and the
  CLI-backed paths carry the enforcement.
