# Minutes MCP tools

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
> Regenerate: node scripts/generate_llms_txt.mjs
> Last generated: 2026-08-07

Minutes exposes 34 tools, 11 resources, and 6 prompt templates through the MCP server.

## Install

```json
{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}
```

## Tools

### Real-time Coach

<a id="tool-start-copilot"></a>

#### `start_copilot`

Start the independent real-time copilot for a goal and observe its CLI nudge stream

Reference URL: https://useminutes.app/docs/mcp/tools#tool-start-copilot

<a id="tool-stop-copilot"></a>

#### `stop_copilot`

Stop the active real-time copilot without changing recording or live transcription

Reference URL: https://useminutes.app/docs/mcp/tools#tool-stop-copilot

<a id="tool-copilot-status"></a>

#### `copilot_status`

Read current copilot session and provider health from the CLI status surface

Reference URL: https://useminutes.app/docs/mcp/tools#tool-copilot-status

<a id="tool-read-copilot-nudges"></a>

#### `read_copilot_nudges`

Read observed copilot nudges incrementally by cursor or time window

Reference URL: https://useminutes.app/docs/mcp/tools#tool-read-copilot-nudges

### Recording

<a id="tool-start-recording"></a>

#### `start_recording`

Start recording audio from the default input device

Reference URL: https://useminutes.app/docs/mcp/tools#tool-start-recording

<a id="tool-stop-recording"></a>

#### `stop_recording`

Stop the current recording and process it

Reference URL: https://useminutes.app/docs/mcp/tools#tool-stop-recording

<a id="tool-get-status"></a>

#### `get_status`

Check if a recording is currently in progress

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-status

<a id="tool-list-processing-jobs"></a>

#### `list_processing_jobs`

List background processing jobs for recent recordings

Reference URL: https://useminutes.app/docs/mcp/tools#tool-list-processing-jobs

### Search and recall

<a id="tool-list-meetings"></a>

#### `list_meetings`

List recent normal meetings and voice memos; restricted items are excluded unless an explicit logged override is enabled and requested

Reference URL: https://useminutes.app/docs/mcp/tools#tool-list-meetings

<a id="tool-search-meetings"></a>

#### `search_meetings`

Search normal meeting transcripts and voice memos; restricted items are excluded unless an explicit logged override is enabled and requested

Reference URL: https://useminutes.app/docs/mcp/tools#tool-search-meetings

<a id="tool-get-meeting"></a>

#### `get_meeting`

Get a normal meeting transcript; restricted meetings return a content-free stub unless an explicit logged override is enabled and requested

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-meeting

<a id="tool-activity-summary"></a>

#### `activity_summary`

Summarize desktop context bound to one exact normal meeting source

Reference URL: https://useminutes.app/docs/mcp/tools#tool-activity-summary

<a id="tool-search-context"></a>

#### `search_context`

Search desktop-context events bound to one exact normal meeting source

Reference URL: https://useminutes.app/docs/mcp/tools#tool-search-context

<a id="tool-get-moment"></a>

#### `get_moment`

Show the local desktop-context rewind bound to one exact normal meeting source

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-moment

<a id="tool-get-screen-context"></a>

#### `get_screen_context`

Retrieve bounded, verified screenshots bound to one exact normal meeting source

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-screen-context

<a id="tool-research-topic"></a>

#### `research_topic`

Research a topic across policy-authorized meetings within supported corpus bounds

Reference URL: https://useminutes.app/docs/mcp/tools#tool-research-topic

### People and relationships

<a id="tool-consistency-report"></a>

#### `consistency_report`

Flag conflicting decisions and stale commitments

Reference URL: https://useminutes.app/docs/mcp/tools#tool-consistency-report

<a id="tool-get-person-profile"></a>

#### `get_person_profile`

Build a profile from policy-authorized meetings within supported corpus bounds

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-person-profile

<a id="tool-track-commitments"></a>

#### `track_commitments`

List open and stale commitments, optionally filtered by person

Reference URL: https://useminutes.app/docs/mcp/tools#tool-track-commitments

<a id="tool-relationship-map"></a>

#### `relationship_map`

Rank relationships from the bounded process-private policy-safe graph projection

Reference URL: https://useminutes.app/docs/mcp/tools#tool-relationship-map

### Insights

<a id="tool-get-meeting-insights"></a>

#### `get_meeting_insights`

Query decisions, commitments, and questions extracted from meetings; each insight records a path to its source meeting, that path is resolved to a meeting in the live corpus and the resolved meeting is re-verified against live sensitivity policy before release, and withheld records are reported as a partial view

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-meeting-insights

<a id="tool-ingest-meeting"></a>

#### `ingest_meeting`

Extract facts from a meeting and update the knowledge base (person profiles, log, index)

Reference URL: https://useminutes.app/docs/mcp/tools#tool-ingest-meeting

<a id="tool-knowledge-status"></a>

#### `knowledge_status`

Show the current state of the knowledge base — configuration, adapter, people count, log entries

Reference URL: https://useminutes.app/docs/mcp/tools#tool-knowledge-status

### Live and dictation

<a id="tool-start-dictation"></a>

#### `start_dictation`

Start dictation mode — speech to clipboard and daily notes

Reference URL: https://useminutes.app/docs/mcp/tools#tool-start-dictation

<a id="tool-stop-dictation"></a>

#### `stop_dictation`

Stop dictation mode

Reference URL: https://useminutes.app/docs/mcp/tools#tool-stop-dictation

<a id="tool-start-live-transcript"></a>

#### `start_live_transcript`

Start a live transcript session for real-time meeting transcription

Reference URL: https://useminutes.app/docs/mcp/tools#tool-start-live-transcript

<a id="tool-read-live-transcript"></a>

#### `read_live_transcript`

Read utterances from the active live transcript with optional cursor or time window

Reference URL: https://useminutes.app/docs/mcp/tools#tool-read-live-transcript

### Notes and processing

<a id="tool-process-audio"></a>

#### `process_audio`

On macOS and Linux, process bounded inbox or Downloads WAV audio; compressed/private containers and Windows fail closed without reading audio; retained library recordings are unavailable

Reference URL: https://useminutes.app/docs/mcp/tools#tool-process-audio

<a id="tool-add-note"></a>

#### `add_note`

Add a timestamped note to the current active recording; existing meeting files are not mutable from this assistant tool

Reference URL: https://useminutes.app/docs/mcp/tools#tool-add-note

<a id="tool-resummarize-meeting"></a>

#### `resummarize_meeting`

Re-run the AI pass on an edited meeting or memo, previewing by default and preserving user edits

Reference URL: https://useminutes.app/docs/mcp/tools#tool-resummarize-meeting

### Voice and speaker ID

<a id="tool-list-voices"></a>

#### `list_voices`

List enrolled voice profiles for speaker identification

Reference URL: https://useminutes.app/docs/mcp/tools#tool-list-voices

<a id="tool-confirm-speaker"></a>

#### `confirm_speaker`

Compatibility name only: agent-controlled speaker mutation is unavailable; use the Minutes app or human CLI

Reference URL: https://useminutes.app/docs/mcp/tools#tool-confirm-speaker

### Agent Event Bus

<a id="tool-add-agent-annotation"></a>

#### `add_agent_annotation`

Append attributed agent commentary as an agent.annotation event, never editing meeting markdown or frontmatter (allowlist-gated by ~/.minutes/agents.allow)

Reference URL: https://useminutes.app/docs/mcp/tools#tool-add-agent-annotation

<a id="tool-get-agent-annotations"></a>

#### `get_agent_annotations`

Compatibility name only: unavailable because an annotation's source pointer and body are both author-supplied, so revalidating the pointer cannot bound what the body discloses

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-agent-annotations

## Resources

### Dashboard

<a id="resource-minutes-dashboard"></a>

#### `ui://minutes/dashboard`

Interactive meeting dashboard and detail viewer

Reference URL: https://useminutes.app/docs/mcp/tools#resource-minutes-dashboard

### Meetings

<a id="resource-recent-meetings"></a>

#### `minutes://meetings/recent`

List of recent meetings and memos

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recent-meetings

<a id="resource-meeting"></a>

#### `minutes://meetings/{slug}`

Get a specific meeting by its filename slug

Reference URL: https://useminutes.app/docs/mcp/tools#resource-meeting

### Status

<a id="resource-recording-status"></a>

#### `minutes://status`

Current recording status

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recording-status

<a id="resource-recent-events"></a>

#### `minutes://events/recent`

Recent pipeline events with meeting-derived content withheld until source policy provenance is available

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recent-events

<a id="resource-agent-annotations"></a>

#### `minutes://events/agent-annotations`

Agent annotations are withheld until their source policy provenance can be revalidated

Reference URL: https://useminutes.app/docs/mcp/tools#resource-agent-annotations

### Memory

<a id="resource-open-actions"></a>

#### `minutes://actions/open`

All open action items across meetings

Reference URL: https://useminutes.app/docs/mcp/tools#resource-open-actions

<a id="resource-recent-ideas"></a>

#### `minutes://ideas/recent`

Recent voice memos and ideas captured from any device (last 14 days)

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recent-ideas

### Live

<a id="resource-live-events"></a>

#### `minutes://events/live`

Live events are currently withheld because raw cursors can reveal restricted activity; reads return a constant unavailable response.

Reference URL: https://useminutes.app/docs/mcp/tools#resource-live-events

<a id="resource-live-copilot"></a>

#### `minutes://live/copilot`

Current copilot state and latest observed nudge. Subscribe for notifications/resources/updated or poll this URI; MCP only controls and observes the independent minutes copilot engine.

Reference URL: https://useminutes.app/docs/mcp/tools#resource-live-copilot

<a id="resource-live-events-since-seq"></a>

#### `minutes://events/live{?since_seq,limit}`

Live event cursor reads are currently withheld because raw cursors can reveal restricted activity; reads return a constant unavailable response.

Reference URL: https://useminutes.app/docs/mcp/tools#resource-live-events-since-seq

## Prompt templates

### Prep

<a id="prompt-meeting-prep"></a>

#### `meeting_prep`

Prepare for an upcoming meeting

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-meeting-prep

<a id="prompt-person-briefing"></a>

#### `person_briefing`

Get a briefing on a person before a meeting

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-person-briefing

<a id="prompt-topic-research"></a>

#### `topic_research`

Research a topic across policy-authorized meetings within supported corpus bounds

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-topic-research

### Review

<a id="prompt-weekly-review"></a>

#### `weekly_review`

Review this week's meetings

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-weekly-review

<a id="prompt-find-action-items"></a>

#### `find_action_items`

Find action items assigned to someone

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-find-action-items

### Capture

<a id="prompt-start-meeting"></a>

#### `start_meeting`

Start recording a meeting

Reference URL: https://useminutes.app/docs/mcp/tools#prompt-start-meeting
