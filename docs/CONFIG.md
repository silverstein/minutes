# Minutes configuration reference

Minutes uses a single optional TOML file for all configuration. Everything has a compiled-in default, so Minutes works with no config file at all.

## Where it lives

- **macOS / Linux:** `$XDG_CONFIG_HOME/minutes/config.toml` when `XDG_CONFIG_HOME` is set, otherwise `~/.config/minutes/config.toml`.
- **Windows:** `%APPDATA%\minutes\config.toml`.

Settings → **Advanced** → **Open** in the desktop app opens this file in your default editor. The Settings panel only exposes the fields users ask about most often; everything else lives here.

## Precedence

`compiled defaults → config file override → CLI flag override`

So `minutes record --device "MacBook Pro Microphone"` always wins over `[recording] device = "AirPods"`, which itself wins over the default "use system default".

## Sections

### `[transcription]` — local ASR

| key | default | meaning |
|---|---|---|
| `engine` | `"whisper"` | `"whisper"` (default) or `"parakeet"` |
| `model` | `"base"` | Whisper model: `tiny` / `base` / `small` / `medium` / `large-v3` |
| `parakeet_model` | `"tdt-ctc-110m"` | Parakeet model: `tdt-ctc-110m` or `tdt-600m` |
| `language` | auto-detect | BCP-47 tag (e.g. `"en"`, `"es"`) to force a specific language |
| `noise_reduction` | `true` | RNNoise pre-filter (requires `denoise` feature) |
| `vad_model` | `"silero-v6.2.0"` | Silero VAD model name; empty string disables |
| `min_words` | `3` | Drop utterances with fewer than this many words |
| `parakeet_binary` | `"parakeet"` | PATH lookup or absolute path to the parakeet binary |
| `parakeet_sidecar_enabled` | `false` | Opt-in warm sidecar path (beta) |
| `parakeet_fp16` | `true` | GPU fp16 inference for lower memory use |
| `parakeet_boost_limit` / `parakeet_boost_score` | `0` / `2.0` | Knowledge-graph phrase boosting; 0 = off |

### `[diarization]` — speaker attribution

| key | default | meaning |
|---|---|---|
| `engine` | `"none"` | `"none"` or `"pyannote-rs"` |
| `threshold` | `0.4` | Cosine similarity cutoff; lower merges more aggressively |
| `embedding_model` | `"cam++"` | `"cam++"` or `"cam++-lm"` (lower EER, lower similarities) |

### `[summarization]` — post-record summaries

| key | default | meaning |
|---|---|---|
| `engine` | `"none"` | `"none"`, `"agent"`, `"ollama"`, `"claude"`, `"openai"`, `"mistral"` |
| `agent_command` | `"claude"` | CLI to shell out to when engine = `"agent"` |
| `ollama_url` | `http://localhost:11434` | Ollama server URL |
| `ollama_model` | `"llama3.2"` | Model name pulled in Ollama |
| `mistral_model` | `"mistral-large-latest"` | Mistral API model |
| `chunk_max_tokens` | `4000` | Max tokens per chunk when splitting long transcripts |

### `[recording]` — capture behavior

| key | default | meaning |
|---|---|---|
| `device` | unset | Explicit input device name; falls back to system default |
| `silence_reminder_secs` | `300` | Seconds of silence before a reminder notification; 0 = off |
| `silence_threshold` | `3` | RMS energy level (0–100) below which audio is silence |
| `silence_auto_stop_secs` | `1800` | Seconds of silence before auto-stop; 0 = off |
| `max_duration_secs` | `28800` | Hard recording cap (default 8h); 0 = off |
| `min_disk_space_mb` | `500` | Auto-stop when free disk space drops below this; 0 = off |
| `auto_call_intent` | `false` | Infer call intent from process detection (high false-positive rate) |
| `allow_degraded_call_capture` | `false` | Allow call capture when selected input isn't a system-audio route |

### `[recording.sources]` — multi-source capture

| key | default | meaning |
|---|---|---|
| `voice` | unset | Voice (mic) device name, or `"default"` |
| `call` | unset | Call (system audio) device name, or `"auto"` to detect loopback |

### `[identity]` — who you are (for attribution)

| key | default | meaning |
|---|---|---|
| `name` | unset | Your canonical name used in `"Mat said…"` attribution |
| `email` | unset | Legacy single email (backward-compat) |
| `emails` | `[]` | All addresses you send from; folded onto your canonical person entity |
| `aliases` | `[]` | Alternate name spellings (`["Mathieu", "Matt"]`) |

### `[dictation]` — dictation-mode behavior

| key | default | meaning |
|---|---|---|
| `destination` | `"clipboard"` | `"clipboard"`, `"file"`, or `"command"` |
| `destination_file` | unset | Target file when `destination = "file"` |
| `destination_command` | unset | Shell command when `destination = "command"` |
| `accumulate` | `true` | Append successive utterances rather than replacing |
| `daily_note_log` | `true` | Append every dictation to the daily note |
| `auto_paste` | `false` | Paste the result immediately after dictation ends |
| `silence_timeout_ms` | `2000` | Silence threshold that ends a dictation session |
| `max_utterance_secs` | `120` | Force-finalize an utterance at this length |
| `model` | `"base"` | Whisper model for dictation |
| `cleanup_engine` | unset | Optional LLM to clean up filler words |

### `[watch]` — folder watcher

| key | default | meaning |
|---|---|---|
| `paths` | `[]` | Folders to watch for new audio files |
| `extensions` | `["wav","m4a","mp3","ogg"]` | Extensions to process |
| `type` | `"auto"` | `"auto"`, `"meeting"`, `"memo"` — routing override |
| `diarize` | `false` | Run diarization on watched files |
| `delete_source` | `false` | Move source to `processed/` after success |
| `settle_delay_ms` | `2000` | Wait for file to stop growing before processing |
| `dictation_threshold_secs` | `60` | Files shorter than this route as memos (skip diarization) |

### `[knowledge]` — LLM wiki integration

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Write facts to a markdown wiki after each meeting |
| `path` | unset | Wiki root (e.g. `~/Documents/life`) |
| `adapter` | `"wiki"` | `"wiki"`, `"para"`, `"obsidian"` |
| `engine` | `"none"` | `"agent"`, `"ollama"`, `"none"` |
| `agent_command` | `"claude"` | Agent CLI when engine = `"agent"` |
| `log_file` / `index_file` | `log.md` / `index.md` | Chronological + content-oriented index |
| `min_confidence` | `"strong"` | `"explicit"`, `"strong"`, `"inferred"`, `"tentative"` |

### `[vault]` — Obsidian / Logseq / markdown vault sync

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Sync meeting markdown into a vault |
| `path` | unset | Vault root |
| `meetings_subdir` | `"areas/meetings"` | Subfolder inside the vault |
| `strategy` | `"auto"` | `"auto"`, `"symlink"`, `"copy"`, `"direct"` |

### `[hooks]` — pipeline extensibility

| key | default | meaning |
|---|---|---|
| `post_record` | unset | Shell command run after each recording; transcript path appended as final arg |

### `[call_detection]` — automatic call awareness

| key | default | meaning |
|---|---|---|
| `enabled` | `true` | Detect active calls automatically |
| `poll_interval_secs` | `1` | How often to check for active calls |
| `cooldown_minutes` | `5` | Wait before re-triggering after a hangup |
| `apps` | `["zoom.us","Microsoft Teams","Webex"]` | App names to recognize |
| `stop_when_call_ends` | `false` | Show an auto-stop countdown when the call ends |
| `call_end_stop_countdown_secs` | `30` | Seconds before auto-stop fires |

### `[palette]` — command palette

| key | default | meaning |
|---|---|---|
| `shortcut_enabled` | `true` | Global shortcut on |
| `shortcut` | `"CmdOrCtrl+Shift+K"` | Chord; Settings dropdown offers preset alternatives |

### `[live_transcript]` — live transcription during recording

| key | default | meaning |
|---|---|---|
| `model` | inherits dictation model | Whisper model for live mode |
| `max_utterance_secs` | `30` | Force-finalize an utterance at this length |
| `save_wav` | `false` | Keep raw WAV alongside JSONL for post-meeting reprocessing |
| `shortcut_enabled` | `false` | Global shortcut on |
| `shortcut` | `"CmdOrCtrl+Shift+L"` | Chord |

### `[dictation]` global shortcut

Separate from the dictation-mode behavior section above. Controlled by the Settings UI or:

```toml
[dictation]
shortcut_enabled = true
shortcut = "CmdOrCtrl+Shift+Space"
hotkey_enabled = false
hotkey_keycode = 57   # Caps Lock (macOS) — requires Input Monitoring
```

### `[voice]` — voice enrollment

| key | default | meaning |
|---|---|---|
| `enabled` | `true` | Learn voices across recordings |
| `match_threshold` | `0.65` | Cosine similarity cutoff for voice enrollment matching |

### `[screen_context]` — ambient screenshots

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Capture periodic screenshots during recording |
| `interval_secs` | `30` | Seconds between captures |
| `keep_after_summary` | `false` | Retain screenshots after summarization |

### `[search]` — search backend

| key | default | meaning |
|---|---|---|
| `engine` | `"builtin"` | `"builtin"` or `"qmd"` |
| `qmd_collection` | unset | Collection name when engine = `"qmd"` |

### `[daily_notes]` — daily note integration

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Append dictations / memos to daily notes |
| `path` | derived | Override daily-note folder |

### `[security]` — access restrictions

| key | default | meaning |
|---|---|---|
| `allowed_audio_dirs` | `[]` | If non-empty, only these dirs can be opened via `minutes open` |

### `[privacy]` — privacy toggles

| key | default | meaning |
|---|---|---|
| `hide_from_screen_share` | `true` | Exclude the Minutes window from screen sharing |

### `[assistant]` — Claude / Codex / Gemini integration

| key | default | meaning |
|---|---|---|
| `agent` | `"claude"` | Which CLI to spawn for the Recall panel |
| `agent_args` | `[]` | Extra launch flags (`--dangerously-skip-permissions`, `--model sonnet`, etc.) |

### `[calendar]` — calendar source

| key | default | meaning |
|---|---|---|
| `enabled` | `true` | Read upcoming meetings from the system calendar |

### `output_dir` — top-level

Default: `~/meetings` on Unix, `%USERPROFILE%\meetings` on Windows. Change to route everywhere meeting output lives — recordings, memos, processed/, failed-captures/.

## What's not in this file

Runtime-only signals (detected audio devices, model provenance metadata, speaker voice embeddings) live under `~/.minutes/` and aren't user-configurable via TOML. Most are regenerated on demand; a few (voice profiles) persist across rebuilds via `~/.minutes/voices.db`.

## Contributing

If you add a config field, please update this reference so the Advanced → View docs surface doesn't drift. The CI guard in `tauri/src-tauri/src/commands.rs` (`every_cmd_set_setting_arm_has_a_caller`) catches one specific class of drift — arms with no UI AND no internal caller — but it doesn't enforce documentation. That's on us.
