# RFC 0007: Voice Live, a spoken assistant over Minutes' memory

Status: accepted for implementation, phase 1 in progress

Date: 2026-09-15

## Summary

Voice Live is a push-to-talk spoken assistant inside Minutes. The user holds a shortcut, talks, and hears an answer. The model is a realtime speech-to-speech model (first provider: Gemini 3.8 Live over the Live API WebSocket). Every fact it speaks comes from a tool call into minutes-core: meeting search and reads, person profiles, commitments, insights, notes, and the configured knowledge base. It is an optional, cloud-backed, failure-isolated consumer. It never owns capture and never runs during a recording without the user starting it.

The 2026-09-15 standalone prototype (a Node bridge from the Live API to the MCP server) established that the loop is useful: prep and debrief flows work by voice, misheard names are the dominant failure and are fixable with lexical biasing plus a phonetic resolver, and grounded judgment ("what stood out this month") works when the prompt allows it. This RFC moves that loop into the product.

## Why in the app, not a sidecar

- Permissions. The app already holds the microphone grant and, when the user has given it, the Screen Recording grant. A separate process has neither.
- Capture stack. `AudioStream::start` already yields 16 kHz mono f32 chunks with device selection, reconnect, and permission preflight. The Live API wants exactly that format.
- Tools. Every MCP tool the prototype used maps to a minutes-core function, so dispatch is a direct call, not a subprocess per turn.
- Trigger and feedback. The shortcut manager, its hold/lock state machine, and the dictation overlay window are the right surface for hold-to-talk.

## Non-goals

- Voice during a live meeting. Coach HUD and the terminal sidekick remain the live-meeting surfaces (docs/plans/live-assistance-orchestration-2026-07-15.md).
- Continuous screen streaming. At most one frame, on explicit request.
- Fully local voice. Tracked as a follow-up provider, not in scope here.
- Auto-start, wake words, or always-on listening.

## Architecture

Module: `crates/core/src/voice_live/` behind Cargo feature `voice-live` (optional dep `tungstenite` with rustls and webpki roots; no tokio, matching core's blocking-threads model). Submodules:

- `protocol.rs`: Live API message types and a blocking WebSocket client. Reader thread decodes server messages into a channel; a `Mutex<WebSocket>` writer sends client messages. Handles `setupComplete`, `serverContent` (audio parts, input and output transcription, `interrupted`, `turnComplete`), `toolCall`, `toolCallCancellation`, `goAway`, `sessionResumptionUpdate`.
- `audio_out.rs`: cpal output stream (cpal is already a dependency) fed by a ring buffer. Model audio is 24 kHz PCM16; it is linearly upsampled to the device rate. `flush()` implements barge-in.
- `voice_io.rs` (macOS): one VoiceProcessingIO unit for both microphone and speaker, so the speaker signal is cancelled out of the mic. Without it, open mic hears the assistant and the provider's speech detection interrupts it mid-sentence, which the first dogfood session hit immediately. Browser clients get this from `getUserMedia({ echoCancellation: true })`; this is the native equivalent. `decimate.rs` brings the 24 kHz mic side down to 16 kHz. Other platforms use `audio_out.rs` plus `AudioStream` and need headphones on open mic until a platform canceller lands.
- `tools.rs`: function declarations and dispatch. Tool results are JSON text, truncated to a per-tool budget so a 128K voice context is not consumed by one transcript.
- `mcp.rs`: a blocking MCP client, so a session can reach tools Minutes does not implement. Servers launch as child processes, `tools/list` becomes function declarations under a `server__tool` name after the JSON Schema is reduced to what the Live API accepts, and a call routes straight back. Allowlisted and capped per server, because voice context is small and tool choice degrades fast. Complements `ask_agent`, which delegates to a local coding agent instead and needs no credentials of its own.
- `names.rs`: known-name list from the people projection plus `vocabulary.toml`, injected into the system prompt as spelling bias, and `resolve_person`, a phonetic-plus-edit-distance matcher over that list.
- `session.rs`: the runner. Owns the mic stream, the socket, the playback, the tool executor thread, the transcript log, and a state machine (`Connecting`, `Ready`, `Listening`, `Thinking`, `Speaking`, `Closed`). Emits `VoiceLiveEvent`s to the host (CLI or Tauri).
- `mod.rs`: `VoiceLiveConfig`, public entry points, and the failure-isolation contract.

Hosts:

- CLI: `minutes talk` (phase 1; `minutes voice` is already speaker enrollment). Open mic with server VAD where echo cancellation exists, push-to-talk elsewhere; `--ptt` and `--open-mic` force either. Prints both transcripts and tool calls. This is the test harness that needs no app rebuild.
- Tauri (phase 2): a third `ShortcutSlot::Voice` in the shortcut manager reusing the hold/lock state machine, `cmd_start_voice`, `cmd_stop_voice`, `cmd_voice_status`, and a voice state in the dictation overlay (listening, thinking, speaking) using the `--capture` blue for active capture per DESIGN.md. API key stored through the existing Keychain secret store and hydrated into the process at startup, mirroring the OpenAI-compatible key.

## Configuration

New section `[voice_live]` (the existing `[voice]` section is speaker identification and is not reused):

```toml
[voice_live]
enabled = false
provider = "gemini"                 # only provider in phase 1
model = "gemini-3.8-live"
api_key_env = "GEMINI_API_KEY"      # name of the env var, never the key
language = "en-US"
allow_cloud = false                 # explicit opt-in, same contract as copilot.allow_cloud
tool_scheduling = "when_idle"       # when_idle | interrupt
max_tool_chars = 12000
known_people = 200                  # names injected as spelling bias
brain_search = true                 # expose knowledge base search/read when [knowledge].path is set
screen_on_request = false           # expose look_at_screen (one frame, explicit ask)
log_sessions = true                 # ~/.minutes/voice-sessions/*.md
echo_cancellation = true            # macOS voice-processing unit; plain capture elsewhere
speech_start_sensitivity = "low"    # provider VAD on open mic; matches the proven browser clients
speech_end_sensitivity = "low"
```

## Tool surface (phase 1)

Read: `list_meetings`, `search_meetings`, `get_meeting`, `get_meeting_insights`, `research_topic`, `get_person_profile`, `relationship_map`, `track_commitments`, `consistency_report`, `get_status`, `resolve_person`, `search_brain`, `read_brain`.

Write: `add_note`. Nothing else writes in phase 1. `confirm_speaker` by voice is deferred until the spoken echo-back design is settled, per the "wrong rewrite is worse than none" rule.

Excluded by design: recording and dictation control, live transcript control, copilot control, processing and ingest, agent annotations.

The desktop app is the operator's own surface and reads with `include_restricted: true`, matching the rest of the app. The CLI host follows the same rule because it is the same operator.

## Prompt contract

The system instruction states: spoken register, one to three sentences; facts only from tools; opinions welcome when grounded and attributed; names are approximate and must be resolved before declaring someone missing; prep and debrief scripts; brain versus meeting source attribution; ask before any write. It embeds the known-name list and vocabulary terms. It is a Rust constant with the dynamic parts interpolated, and it is unit-tested for the presence of each rule so a later edit cannot drop one silently.

## Failure isolation and safety

- The session never touches recording state. It opens its own `AudioStream`; if a recording is active the CLI and app refuse to start voice rather than share or steal the device.
- Provider errors and socket closes change voice state only. They cannot stop, pause, or degrade capture, WAV preservation, or the event log (RFC 0004 boundary applies).
- Tool execution runs on its own thread with a per-call timeout. A wedged tool never blocks the socket reader or playback.
- Every session writes a markdown transcript with tool calls to `~/.minutes/voice-sessions/` with `0600` permissions, because the conversation is itself memory.
- No platform gets a self-interrupting default. Open mic is the default only where the speaker signal can be cancelled out of the microphone; elsewhere `minutes talk` defaults to push-to-talk, which never sends microphone audio while the assistant speaks, and `--open-mic` warns that it needs headphones. A desktop voice surface must not ship to a platform before that platform can cancel echo or drive push-to-talk from the shortcut.
- Cloud egress is gated by `allow_cloud`. The first-run UI (phase 2) states plainly that microphone audio and tool results go to the provider.

## Phases

1. Core module, config, tests, `minutes talk` CLI. Dogfood on macOS.
2. Tauri shortcut slot, commands, overlay states, Keychain-backed key, settings UI. Tested in `Minutes Dev.app`.
3. `look_at_screen` (one frame via the app's screen module), `ask_codebase` (delegates to a coding agent), session resumption past the fifteen-minute cap.
4. Alternate providers, including a fully local pipeline.

## Outside the phases

`music.rs` is a labs toy behind `[voice_live] music`, off by default and not part of any phase above. The assistant reads a meeting, a prep or the calendar, writes a music brief itself, and the module renders it through the provider's music model and hands back samples. The model sings when asked and writes its own words, which come back with the audio. It exists because Minutes is the only thing that knows both what your next conversation is and what you wrote about it beforehand. It refuses while a recording is running: music through the speakers reaches the microphone and then the transcript, and RFC 0004's rule that an optional consumer never degrades capture applies to it exactly as it does to everything else.

## Open questions

- Whether a single on-demand image frame moves a Live session into the shorter audio-plus-video session cap. Measure in phase 3.
- Whether `confirm_speaker` by voice can meet the identity-rewrite bar with an echo-back confirmation. Decide before phase 3.
