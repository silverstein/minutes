# Configuration

Optional — minutes works out of the box.

In the desktop app, choose **Meeting audio** before the first Meeting recording:
**Microphone only** for a room, or **Microphone + system audio** for a computer
call. The choice is remembered and works without call detection. Memos use the
microphone. Native desktop call capture requires macOS 15+, Microphone access
and Screen Recording access; a failed two-source start does not fall back to
microphone-only recording.

For CLI system capture on macOS 14.4+, configure **both**
`recording.capture_backend = "core-audio-tap"` and
`recording.sources.call = "auto"`, then run `minutes record --call`.
Process Tap captures the default system output and uses macOS system-audio
access. It is separate from the desktop ScreenCaptureKit helper. See the
[audio setup guide](architecture/audio-devices.md) for permissions and loopback
alternatives.

```toml
# By default: ~/.config/minutes/config.toml
# Or: $XDG_CONFIG_HOME/minutes/config.toml when XDG_CONFIG_HOME is set

[recording]
# Desktop preference; does not change CLI source routing:
# desktop_audio_source = "microphone-and-system" # Or "microphone"
# Optional CLI system capture on macOS 14.4+; BOTH this and sources.call are needed:
# capture_backend = "core-audio-tap"

[recording.sources]
# voice = "default"
# call = "auto"          # Core Audio Process Tap, or auto-detect a CPAL loopback

[transcription]
# cross_source_echo_suppression = true # Conservative mic echo cleanup for calls; false keeps all source lines.
engine = "auto"           # Apple Silicon release builds use Parakeet v3 when installed, otherwise Whisper
# engine = "whisper"      # Keep Whisper explicitly on every platform
# engine = "sherpa"       # Select the in-process Parakeet v3 engine explicitly
model = "small"           # Whisper meeting baseline: small (466MB); base, medium, large-v3 also supported
                          # tiny (75MB) is an explicit advanced low-resource choice with lower accuracy
# language = "ur"          # Force transcription language (ISO 639-1 code, e.g. "en", "ur", "es", "zh")
                          # Default: auto-detect. Set this for similar-sounding languages (Urdu/Hindi, etc.)
# engine = "apple-speech"  # Batch transcription still resolves to Whisper. Standalone live transcript can use
#                          # Apple Speech in the signed app on macOS 26+: set [live_transcript] backend = "apple-speech".
#                          # See docs/architecture/apple-speech.md.
# engine = "parakeet"      # Retained for compatibility, but currently resolves to Whisper until secure byte transport lands.
# parakeet_model = "tdt-600m"                    # parakeet: tdt-ctc-110m (English), tdt-600m (multilingual v3)
# parakeet_binary = "parakeet"                   # Path to parakeet.cpp binary (or name in PATH)
# parakeet_boost_limit = 25                      # Experimental: boost top graph-derived phrases (0 disables)
# parakeet_boost_score = 2.0                     # Experimental tuning for parakeet.cpp --boost-score
# parakeet_fp16 = false                          # Retained legacy setting; inert while Parakeet batch selection resolves to Whisper
# parakeet_vocab = "tdt-600m.tokenizer.vocab"      # Safer when multiple Parakeet models are installed
# vad_model = "silero-v6.2.0"     # Silero VAD model (auto-downloaded by setup). Empty = disable.
                                   # Prevents whisper hallucination loops on non-English/noisy audio.

[calendar]
enabled = true
# use_event_title_for_meeting_title = true  # Use the matching calendar event's title
# ignore_title_contains = ["focus time", "lunch"]  # Case-insensitive title exclusions
# require_attendees_or_url = true          # Skip events with neither attendees nor a meeting URL

[summarization]
engine = "none"           # Default: Claude summarizes conversationally via MCP
                          # "auto" = auto-detect an installed agent CLI for pipeline summaries
                          # "agent" = uses your Claude Code, Codex, OpenCode, or Pi subscription (no API key)
                          # "ollama" = local, free
                          # "openai-compatible" = OpenRouter, Vercel/Cloudflare gateways, llama.cpp, LM Studio, etc.
                          # "claude" / "openai" = direct API key (legacy)
agent_command = "claude"  # Which CLI to use when engine = "agent" (claude, codex, opencode, pi, etc.)
ollama_url = "http://localhost:11434"
ollama_model = "llama3.2"
openai_compatible_base_url = "http://localhost:11434/v1"
openai_compatible_model = "llama3.2"
openai_compatible_api_key_env = "" # Blank means no Authorization header for local endpoints. Desktop cloud endpoints can still use a saved Keychain key without rewriting config.

[copilot]
fast_model = "qwen3.5:4b" # Portable fallback before hardware-aware setup

[diarization]
engine = "auto"           # "auto" (default — uses pyannote-rs if models downloaded, otherwise skips),
                          # "pyannote-rs" (always on — native Rust, no Python),
                          # "pyannote" (legacy — requires pip install pyannote.audio),
                          # "none" (explicitly disabled)
# embedding_model = "cam++"  # "cam++" (default) or "cam++-lm" (~12% lower EER on benchmarks).
                          # Note: cam++-lm produces lower cosine similarities, so if you switch
                          # to it you should also lower voice.match_threshold to ~0.1–0.2.
# threshold = 0.5         # Speaker similarity threshold (0.0–1.0). Lower = fewer speakers.

[voice]
# enabled = true          # Voice profile matching during diarization (default: true if enrolled)
# match_threshold = 0.65  # Cosine similarity threshold for voice matching (higher = stricter).
                          # If using embedding_model = "cam++-lm", lower this to ~0.1–0.2.

[voice_live]
# Spoken assistant over your meeting memory, run with `minutes talk` (RFC 0007).
# Off by default: it sends microphone audio and tool results to a cloud realtime model.
# enabled = true
# allow_cloud = true              # Required acknowledgement of that egress
# provider = "gemini"             # Only provider today
# model = "gemini-3.8-live"
# api_key_env = "GEMINI_API_KEY"  # Name of the env var holding the key. The key itself never goes in this file.
# language = "en-US"
# voice_name = "Sulafat"          # Optional warmer voice; voice selection is separate from persona
# persona = "morris"              # Optional concise, dry chief-of-staff personality
# brain_search = true             # Expose knowledge base search/read when [knowledge].path is set
# log_sessions = true             # Write each session transcript to ~/.minutes/voice-sessions/
# echo_cancellation = true        # macOS: cancel the speaker signal out of the mic so open mic does not hear itself
# proactive_audio = true          # Let it stay silent when speech was not meant for it.
#                                 # Turns open mic from something you talk to into something
#                                 # you can leave running. Ignored in push-to-talk.
# speech_start_sensitivity = "low"  # Provider speech detection on open mic: "low" or "high"
# speech_end_sensitivity = "low"
# prep_artifacts = true           # Let voice read your /minutes-prep and /minutes-brief files
# calendar = true                 # Let voice read upcoming calendar events
# screen_on_request = true        # Let voice take ONE frame of your screen when you ask. Sends that image to the provider.
# clipboard = true                # Explicit plain-text read/copy requests only; no monitoring
# text_input = true               # Named-app selection reads and verified text insertion; never Send/Submit
#                                 # Optional text_input_apps overrides the exact bundle-ID allowlist.
#                                 # See docs/voice-text-transfer.md for safeguards and supported-editor limits.
# ask_agent = true                # Let voice relay a question to your local coding agent (see [assistant] agent).
#                                 # Off by default. Pair it with delegate_agent_args to bound what it can do.
# delegate_agent = ""             # Override which agent. Empty follows [assistant] agent.
# delegate_timeout_secs = 120
# delegate_agent_args = []        # Empty follows [assistant] agent_args. The relayed agent
#                                 # has no terminal, so it needs flags that let it run without
#                                 # stopping to ask permission, or the call just times out.
# delegate_cwd = "~/Sites"        # Where the relayed agent starts looking
# desktop_control = true          # Let it open apps and files, control playback, add reminders
# desktop_outward = true          # Also let it send an iMessage or email. Every send is read
#                                 # back to you and requires the displayed local host review.
# music = true                    # Labs toy: generate and play music, sung or instrumental,
#                                 # steered by a meeting or a prep. Refuses while recording.
# music_max_secs = 0             # 0 plays the whole piece; set a number to cap it
# delegate_writes = false         # Let the relayed agent change things. Off by default:
#                                 # a misheard sentence should not open an issue or edit a file.

# Reach tools Minutes does not implement. Each server is launched as a child
# process and its tools appear to the voice assistant as `name__tool`.
# Secrets never go here: the server inherits your environment and reads the
# variable it already expects.
# [[voice_live.mcp_servers]]
# name = "hubspot"
# command = "npx"
# args = ["-y", "@hubspot/mcp-server"]
# tools = ["search_contacts"]     # Allowlist. Omit to take up to max_tools.
# max_tools = 8                   # Voice context is small; keep this low.

[search]
engine = "builtin"        # builtin (regex) or qmd (semantic)

[watch]
paths = ["~/.minutes/inbox"]
settle_delay_ms = 2000              # Cloud sync safety delay (wait for file to finish syncing)
dictation_threshold_secs = 120      # Files shorter than this → memo (skip diarize). 0 = disable.
# Add cloud sync folders to watch for phone voice memos:
# paths = ["~/.minutes/inbox", "~/Dropbox/minutes-inbox"]

[screen_context]
enabled = false           # Opt-in: capture screenshots during recording for LLM context
interval_secs = 30        # How often to capture (seconds)
keep_after_summary = false # Delete screenshots after summarization (default: clean up)

[call_detection]
enabled = true            # macOS-only today
poll_interval_secs = 1     # device/app checks; input attribution refreshes every 5s
cooldown_minutes = 5
# Default apps stay conservative:
# apps = ["zoom.us", "Microsoft Teams", "Webex"]
#
# Browser-based integrations such as Google Meet are opt-in on purpose.
# If you want to dogfood browser detection, add the sentinel explicitly:
# apps = ["zoom.us", "Microsoft Teams", "Webex", "google-meet"]

[assistant]
agent = "claude"          # CLI launched by the Tauri AI Assistant
agent_args = []           # Optional extra args, e.g. ["--dangerously-skip-permissions"]
```

`llama3.2` remains only as the legacy post-meeting summarization default above.
Coach does not use it. `minutes coach setup` selects the strongest reviewed
model that fits the machine and passes its latency probe:

| Memory | Apple Silicon | Other platforms |
|---|---|---|
| 64 GB or more | `qwen3.5:35b-a3b-nvfp4` | `qwen3.5:35b-a3b` |
| 32–63 GB | `gemma4:26b-mlx` | `gemma4:26b` |
| 16–31 GB | `qwen3.5:9b-mlx` | `qwen3.5:9b` |
| Less than 16 GB | `qwen3.5:4b-mlx` | `qwen3.5:4b` |

See [Coach model selection](architecture/coach-model-selection.md) for probe
and fallback behavior.

When screen context is enabled, Minutes records its observed state separately
from desktop app/window metadata. Inspect the current state without exposing an
image, or retrieve up to three verified PNGs nearest a meeting moment:

```bash
minutes context status --json
minutes context screen --session <context-session-id> --at <rfc3339-time> --limit 1 --json
```

MCP clients can request the same bounded images with `get_screen_context`.
Images are never attached to every prompt automatically, and an assistant
should only claim it can see the screen after it has opened or received a
specific returned image. Unless `keep_after_summary = true`, Minutes deletes
the PNGs and their readable references after summarization.

## Installing models without changing your preferences

`minutes setup` keeps your configured meeting model (Small by default), installs
the selected Live/dictation models (Base by default), and preserves all model and
engine selections. On supported Apple Silicon builds with the sherpa plugin, it
also installs Parakeet for the Auto engine.

To install a companion model without selecting it for every meeting:

```bash
minutes setup --model base --download-only
```

An explicit `minutes setup --model small` installs and selects Small for Whisper
meeting transcription. It does not change the separate dictation or Live model.
The command explains the selection before installation.

Tiny remains available through `--model tiny` or the advanced model options.
Minutes does not select it as an automatic fallback. Existing Tiny preferences
are preserved; choose a larger model explicitly to upgrade.

Background MCP model installation uses `--download-only`. If an older CLI does
not support that flag, update Minutes; the MCP server will not retry with a
command that changes your preferences.
