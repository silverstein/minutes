# Minutes MCP tools

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
> Regenerate: node scripts/generate_llms_txt.mjs
> Last generated: 2026-04-24

Minutes exposes 29 tools, 7 resources, and 6 prompt templates through the MCP server.

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

List recent meetings and voice memos

Reference URL: https://useminutes.app/docs/mcp/tools#tool-list-meetings

<a id="tool-search-meetings"></a>

#### `search_meetings`

Search meeting transcripts and voice memos

Reference URL: https://useminutes.app/docs/mcp/tools#tool-search-meetings

<a id="tool-get-meeting"></a>

#### `get_meeting`

Get full transcript of a specific meeting

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-meeting

<a id="tool-activity-summary"></a>

#### `activity_summary`

Summarize meeting-adjacent desktop context for a linked artifact, context session, or time window

Reference URL: https://useminutes.app/docs/mcp/tools#tool-activity-summary

<a id="tool-search-context"></a>

#### `search_context`

Search desktop-context events across app focus and captured window titles, including opted-in browser titles

Reference URL: https://useminutes.app/docs/mcp/tools#tool-search-context

<a id="tool-get-moment"></a>

#### `get_moment`

Show the local desktop-context rewind around a linked artifact, session, or timestamp

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-moment

<a id="tool-research-topic"></a>

#### `research_topic`

Research a topic across meetings, decisions, and follow-ups

Reference URL: https://useminutes.app/docs/mcp/tools#tool-research-topic

### People and relationships

<a id="tool-consistency-report"></a>

#### `consistency_report`

Flag conflicting decisions and stale commitments

Reference URL: https://useminutes.app/docs/mcp/tools#tool-consistency-report

<a id="tool-get-person-profile"></a>

#### `get_person_profile`

Build a profile for a person across all meetings

Reference URL: https://useminutes.app/docs/mcp/tools#tool-get-person-profile

<a id="tool-track-commitments"></a>

#### `track_commitments`

List open and stale commitments, optionally filtered by person

Reference URL: https://useminutes.app/docs/mcp/tools#tool-track-commitments

<a id="tool-relationship-map"></a>

#### `relationship_map`

All contacts with relationship scores and losing-touch alerts

Reference URL: https://useminutes.app/docs/mcp/tools#tool-relationship-map

### Insights

<a id="tool-get-meeting-insights"></a>

#### `get_meeting_insights`

Query structured meeting insights (decisions, commitments, questions) with confidence filtering

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

Process an audio file through the transcription pipeline

Reference URL: https://useminutes.app/docs/mcp/tools#tool-process-audio

<a id="tool-add-note"></a>

#### `add_note`

Add a timestamped note to the current recording or an existing meeting

Reference URL: https://useminutes.app/docs/mcp/tools#tool-add-note

<a id="tool-open-dashboard"></a>

#### `open_dashboard`

Open the Meeting Intelligence Dashboard in the browser — visual overview of conversation memory

Reference URL: https://useminutes.app/docs/mcp/tools#tool-open-dashboard

### Voice and speaker ID

<a id="tool-list-voices"></a>

#### `list_voices`

List enrolled voice profiles for speaker identification

Reference URL: https://useminutes.app/docs/mcp/tools#tool-list-voices

<a id="tool-confirm-speaker"></a>

#### `confirm_speaker`

Confirm or correct speaker attribution in a meeting transcript

Reference URL: https://useminutes.app/docs/mcp/tools#tool-confirm-speaker

### Integration

<a id="tool-qmd-collection-status"></a>

#### `qmd_collection_status`

Check if the Minutes output directory is registered as a QMD collection

Reference URL: https://useminutes.app/docs/mcp/tools#tool-qmd-collection-status

<a id="tool-register-qmd-collection"></a>

#### `register_qmd_collection`

Register the Minutes output directory as a QMD collection

Reference URL: https://useminutes.app/docs/mcp/tools#tool-register-qmd-collection

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

Recent pipeline events (recordings, processing, notes)

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recent-events

### Memory

<a id="resource-open-actions"></a>

#### `minutes://actions/open`

All open action items across meetings

Reference URL: https://useminutes.app/docs/mcp/tools#resource-open-actions

<a id="resource-recent-ideas"></a>

#### `minutes://ideas/recent`

Recent voice memos and ideas captured from any device (last 14 days)

Reference URL: https://useminutes.app/docs/mcp/tools#resource-recent-ideas

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

Research a topic across all meetings

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
