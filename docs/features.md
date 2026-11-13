# Features

### Record meetings
```bash
minutes record                                    # Record from mic
minutes record --title "Standup" --context "Sprint 4 blockers"  # With context
minutes record --language ur                      # Force Urdu (ISO 639-1 code)
minutes record --device "AirPods Pro"             # Use specific audio device
minutes record --template standup                 # Apply a summary template
minutes stop                                      # Stop from another terminal
```

The desktop app captures your microphone and call audio through ScreenCaptureKit
on macOS 15+. Grant Screen Recording and Microphone access, then use the
"Call detected" banner. Browser-based Meet and Teams detection is experimental.
The CLI needs BlackHole and a Multi-Output Device for system audio.
See [audio device setup](architecture/audio-devices.md).

### Consent disclosure aid

For meetings where participant notice is required, Minutes can show a reminder,
require an interactive acknowledgement, and stamp the chosen basis into the
markdown frontmatter. This is a disclosure aid, not advice; ensure everyone
present consents where required.

```bash
minutes record --consent verbal_all_parties
minutes record --consent notice_in_invite --consent-notice "Notice was included in the calendar invite."
```

```toml
[consent]
mode = "remind" # off | remind | require
disclosure_script = "Heads up: I'm using Minutes to transcribe this conversation locally on my device for my own notes. Let me know if you'd prefer I didn't."
# default_basis = "notice_in_invite"
```

When present, artifacts include:

```yaml
consent: verbal_all_parties
consent_notice: Heads up: I'm using Minutes to transcribe this conversation locally on my device for my own notes. Let me know if you'd prefer I didn't.
```

### Sensitive meetings

Use a sensitive meeting when you want timed typed markers and a saved meeting
artifact, but no audio capture. Stop recording or Live Transcript before starting
sensitive mode; sensitive mode also blocks starting Live Transcript.

```bash
minutes sensitive start --title "Board prep"
minutes note "Opened with pricing risk"
minutes sensitive stop
```

The stop flow writes a normal markdown meeting with `capture: none` and
`sensitivity: restricted`. In a terminal, Minutes prompts for a short debrief.
From scripts or other non-interactive callers it saves immediately with
`debrief: pending` so an assistant can help finish the written summary later.

### Take notes during meetings
```bash
minutes note "Alex wants monthly billing not annual billing"          # Timestamped, feeds into summary
minutes note "Logan agreed"                       # LLM weights your notes heavily
```

### Process voice memos
```bash
minutes process ~/Downloads/voice-memo.m4a        # WAV, M4A, MP3, OGG, WebM, MP4, MOV, or AAC
minutes process ~/.minutes/native-captures/2026-05-19-120148-call.voice.wav --type meeting
minutes watch                                     # Auto-process new files in inbox
```

### Search your authorized memory
```bash
minutes search "pricing"                          # Full-text search
minutes search "onboarding" -t memo               # Filter by type
minutes actions                                   # Open action items across normal meetings
minutes actions --assignee sarah                   # Filter by person
minutes list                                      # Recent recordings
```

### Relationship intelligence

`minutes people` and `minutes commitments` derive bounded relationship intelligence through a process-private projection of current policy-authorized Markdown. `minutes person` and `minutes research` use bounded live-source search instead. Both paths re-attest policy before results are returned; no durable graph cache is trusted.

### Cross-meeting intelligence
```bash
minutes research "pricing strategy"               # Search across normal meetings
minutes person "Alex"                              # Bounded profile from live normal sources
minutes consistency                                # Flag contradicting decisions + stale commitments
```

### Live transcript (real-time coaching)
```bash
minutes live                                     # Start real-time transcription
minutes transcript --since 2m --include-current # Finals + one provisional current draft
minutes stop                                     # Stop live session
```
Live and Recording modes stream local transcription. Finalized lines remain the
durable authority. `--include-current` and MCP
`read_live_transcript(include_current: true)` may return one provisional draft.
Drafts expire after three seconds and never enter files, event history, or logs.

`[live_transcript] promote_on_stop = "process"` preserves WAV and JSONL sources,
then creates a meeting. When remote batch transcription is selected, CLI stop-time
promotion uses local Whisper with the Live model; the saved batch setting is
unchanged. Use `"preserve"` to keep only those timestamped sources.
Use `"off"` for the legacy fixed slot. Live drafts use sealed local Whisper.
Apple Speech and Parakeet preferences currently resolve to Whisper without
writing named plaintext audio. See [Apple Speech](architecture/apple-speech.md)
and [Parakeet](architecture/parakeet.md). The desktop Live Mode toggle starts
the same stream.

### Dictation mode
```bash
minutes dictate                                  # Speak → text appears as you talk
minutes dictate --stdout                         # Output to stdout instead of clipboard
```
Text streams every two seconds and accumulates across pauses. When dictation
ends, Minutes writes the combined text to the clipboard, saves it as a Markdown
file under `dictations/`, and appends it to the daily note when daily notes are
enabled. Set `[dictation] accumulate = false` for per-pause output. The default
`backend = "auto"` uses Parakeet v3 on Apple Silicon when it is installed and the
language fits, and local Whisper otherwise; live partials always come from
Whisper. Retained `[dictation] backend = "apple-speech"` and `"parakeet"`
settings currently resolve to Whisper. Linux uses `wl-clipboard` on Wayland or
`xclip` / `xsel` on X11. X11 auto-paste requires `xdotool`.

### Command palette (desktop app)
Press `⌘⇧K` on macOS to open commands for recording, notes, meetings, search,
and rename. Available commands follow current backend state. Recent commands
retain their payload, including `Search transcripts: pricing`.

The shortcut defaults on and triggers one first-launch notice. Disable it with
`[palette] shortcut_enabled = false` in `$XDG_CONFIG_HOME/minutes/config.toml`
or `~/.config/minutes/config.toml`. Settings also offers `⌘⇧O` and `⌘⇧U`.

### Templates (RFC 0001, Phase 1)
```bash
minutes template list                             # Bundled + project + user templates
minutes template show standup                     # Inspect a template
minutes record --template standup                 # Apply when recording
minutes process voice-memo.m4a --template voice-memo
```
Templates guide extraction of `KEY POINTS`, `DECISIONS`, `ACTION ITEMS`,
`OPEN QUESTIONS`, `COMMITMENTS`, and `PARTICIPANTS`. Bundled templates are
`meeting`, `standup`, `1-on-1`, and `voice-memo`. Overrides load from
`.minutes/templates/` and `~/.minutes/templates/`. See [RFC 0001](rfcs/0001-templates.md).

### Try it without a mic
```bash
minutes demo --full                              # Seed 5 sample meetings (Snow Crash theme)
minutes demo --query                             # Cross-meeting intelligence demo
minutes demo --clean                             # Remove sample meetings
```

The interactive demo seeds interconnected meetings, then lets you pick a thread to explore. Two storylines, five meetings, zero setup.

### System diagnostics
```bash
minutes health                                   # Check model, mic, calendar, disk
minutes demo                                     # Run a pipeline test (bundled audio, no mic)
```
