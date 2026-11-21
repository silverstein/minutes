# Minutes MCP tool map

> Generated from `manifest.json` by `node scripts/generate_llms_txt.mjs`.
> Do not edit by hand.

This manifest contains 34 MCP tools.

| Tool | Description |
|---|---|
| `start_recording` | Start recording audio from the default input device |
| `stop_recording` | Stop the current recording and process it |
| `get_status` | Check if a recording is currently in progress |
| `list_processing_jobs` | List background processing jobs for recent recordings |
| `list_meetings` | List recent normal meetings and voice memos; restricted items are excluded unless an explicit logged override is enabled and requested |
| `search_meetings` | Search normal meeting transcripts and voice memos; restricted items are excluded unless an explicit logged override is enabled and requested |
| `get_meeting` | Get a normal meeting transcript; restricted meetings return a content-free stub unless an explicit logged override is enabled and requested |
| `activity_summary` | Summarize desktop context bound to one exact normal meeting source |
| `search_context` | Search desktop-context events bound to one exact normal meeting source |
| `get_moment` | Show the local desktop-context rewind bound to one exact normal meeting source |
| `get_screen_context` | Retrieve bounded, verified screenshots bound to one exact normal meeting source |
| `process_audio` | On macOS and Linux, process bounded inbox or Downloads WAV audio; compressed/private containers and Windows fail closed without reading audio; retained library recordings are unavailable |
| `add_note` | Add a timestamped note to the current active recording; existing meeting files are not mutable from this assistant tool |
| `consistency_report` | Flag conflicting decisions and stale commitments |
| `get_person_profile` | Build a profile from policy-authorized meetings within supported corpus bounds |
| `research_topic` | Research a topic across policy-authorized meetings within supported corpus bounds |
| `start_dictation` | Start dictation mode — speech to clipboard and daily notes |
| `stop_dictation` | Stop dictation mode |
| `track_commitments` | List open and stale commitments, optionally filtered by person |
| `relationship_map` | Rank relationships from the bounded process-private policy-safe graph projection |
| `list_voices` | List enrolled voice profiles for speaker identification |
| `confirm_speaker` | Compatibility name only: agent-controlled speaker mutation is unavailable; use the Minutes app or human CLI |
| `get_meeting_insights` | Query decisions, commitments, and questions extracted from meetings; each insight records a path to its source meeting, that path is resolved to a meeting in the live corpus and the resolved meeting is re-verified against live sensitivity policy before release, and withheld records are reported as a partial view |
| `start_live_transcript` | Start a live transcript session for real-time meeting transcription |
| `read_live_transcript` | Read utterances from the active live transcript with optional cursor or time window |
| `start_copilot` | Start the independent real-time copilot for a goal and observe its CLI nudge stream |
| `stop_copilot` | Stop the active real-time copilot without changing recording or live transcription |
| `copilot_status` | Read current copilot session and provider health from the CLI status surface |
| `read_copilot_nudges` | Read observed copilot nudges incrementally by cursor or time window |
| `ingest_meeting` | Extract facts from a meeting and update the knowledge base (person profiles, log, index) |
| `resummarize_meeting` | Re-run the AI pass on an edited meeting or memo, previewing by default and preserving user edits |
| `knowledge_status` | Show the current state of the knowledge base — configuration, adapter, people count, log entries |
| `add_agent_annotation` | Append attributed agent commentary as an agent.annotation event, never editing meeting markdown or frontmatter (allowlist-gated by ~/.minutes/agents.allow) |
| `get_agent_annotations` | Compatibility name only: unavailable because an annotation's source pointer and body are both author-supplied, so revalidating the pointer cannot bound what the body discloses |
