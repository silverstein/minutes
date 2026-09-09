# Configuration

Optional — minutes works out of the box.

## Meeting libraries on slow storage

The MCP server and JavaScript SDK use a bounded verification deadline before returning meeting results. The standard profile allows 60 seconds. A library with thousands of small files on a spinning disk can exceed this even when its total size is small, because file seeks and reader round trips add work beyond byte throughput.

Set `MINUTES_CORPUS_STORAGE_PROFILE=slow` in the environment of the MCP server or SDK process to allow up to 240 seconds. For example, add this entry to the existing MCP server configuration's `env` object, then restart the server:

```json
{
  "MINUTES_CORPUS_STORAGE_PROFILE": "slow"
}
```

For a direct source build in PowerShell:

```powershell
$env:MINUTES_CORPUS_STORAGE_PROFILE = "slow"
node crates/mcp/dist/index.js
```

Use a client request timeout of at least 300 seconds for this profile, where the client supports configuring it. The verification cap does not extend a client's own timeout. A client timeout also does not guarantee the server has stopped its work; verification retains its own bounded deadline. Calls that complete sooner return immediately. Remove the setting or use `standard` to restore the 60-second profile. Other values are rejected.

The slow profile changes only the time allowance. It preserves all file and memory limits, full rereads, watcher fences, and worker termination. It does not make a library that exceeds resource limits valid, guarantee completion on every disk, or change native CLI search deadlines. The separate numeric `MINUTES_CORPUS_AUTH_TIMEOUT_MS` variable remains test-harness-only and is not a production setting.

## Capture and processing

```toml
# By default: ~/.config/minutes/config.toml
# Or: $XDG_CONFIG_HOME/minutes/config.toml when XDG_CONFIG_HOME is set

[transcription]
engine = "auto"           # Apple Silicon release builds use Parakeet v3 when installed, otherwise Whisper
# engine = "whisper"      # Keep Whisper explicitly on every platform
# engine = "sherpa"       # Select the in-process Parakeet v3 engine explicitly
model = "small"           # whisper: tiny (75MB), base, small (466MB), medium, large-v3 (3.1GB)
# language = "ur"          # Force transcription language (ISO 639-1 code, e.g. "en", "ur", "es", "zh")
                          # Default: auto-detect. Set this for similar-sounding languages (Urdu/Hindi, etc.)
# engine = "apple-speech"  # Retained for compatibility; currently resolves to Whisper until signed runtime acceptance passes.
#                          # See docs/architecture/apple-speech.md for the candidate byte-transport boundary.
# engine = "parakeet"      # Retained for compatibility, but currently resolves to Whisper until secure byte transport lands.
# parakeet_model = "tdt-600m"                    # parakeet: tdt-ctc-110m (English), tdt-600m (multilingual v3)
# parakeet_binary = "parakeet"                   # Path to parakeet.cpp binary (or name in PATH)
# parakeet_boost_limit = 25                      # Experimental: boost top graph-derived phrases (0 disables)
# parakeet_boost_score = 2.0                     # Experimental tuning for parakeet.cpp --boost-score
# parakeet_fp16 = false                          # Retained legacy setting; inert while Parakeet batch selection resolves to Whisper
# parakeet_vocab = "tdt-600m.tokenizer.vocab"      # Safer when multiple Parakeet models are installed
# vad_model = "silero-v6.2.0"     # Silero VAD model (auto-downloaded by setup). Empty = disable.
                                   # Prevents whisper hallucination loops on non-English/noisy audio.

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
poll_interval_secs = 1
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
