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
| `engine` | `"whisper"` | `"whisper"` (default), or a retained `"parakeet"` preference. Parakeet is not currently selectable on any platform, so retained values resolve visibly to Whisper. |
| `model` | `"base"` | Whisper model: `tiny` / `base` / `small` / `medium` / `large-v3` |
| `parakeet_model` | `"tdt-ctc-110m"` | Parakeet model: `tdt-ctc-110m` or `tdt-600m` |
| `language` | auto-detect | BCP-47 tag (e.g. `"en"`, `"es"`) to force a specific language |
| `noise_reduction` | `true` | RNNoise pre-filter (requires `denoise` feature) |
| `vad_model` | `"silero-v6.2.0"` | Silero VAD model name; empty string disables |
| `min_words` | `3` | Drop utterances with fewer than this many words |
| `parakeet_binary` | `"parakeet"` | PATH lookup or absolute path to the parakeet binary |
| `parakeet_sidecar_enabled` | auto | Records future warm-sidecar intent. The current pathname-only process cannot receive Minutes' anonymous/sealed private-audio capability, so Linux, macOS, and Windows all use Whisper. Auto/`true` cannot bypass this gate; `"off"` forces off. A legacy bool `false` is treated as auto (pre-0.18.8 saves wrote it into every config) (#295). |
| `parakeet_fp16` | `false` | Reserved for a future supported GPU transport; currently not forwarded on Linux |
| `parakeet_boost_limit` / `parakeet_boost_score` | `0` / `2.0` | Knowledge-graph phrase boosting; 0 = off |
| `name_correction` | `"off"` | Post-pass name correction against attendees and vocabulary. Values: `"off"` or `"conservative"`; off by default. |

### `[diarization]` — speaker attribution

| key | default | meaning |
|---|---|---|
| `engine` | `"none"` | `"none"` or `"pyannote-rs"` |
| `threshold` | `0.4` | Cosine similarity cutoff; lower merges more aggressively |
| `embedding_model` | `"cam++"` | `"cam++"` or `"cam++-lm"` (lower EER, lower similarities) |

### `[summarization]` — post-record summaries

| key | default | meaning |
|---|---|---|
| `engine` | `"none"` | `"none"`, `"auto"`, `"agent"`, `"ollama"`, `"openai-compatible"`, `"claude"`, `"openai"`, `"mistral"` |
| `agent_command` | `"claude"` | CLI to shell out to when engine = `"agent"` (`claude`, `codex`, `opencode`, `pi`, etc.) |
| `ollama_url` | `http://localhost:11434` | Ollama server URL |
| `ollama_model` | `"llama3.2"` | Model name pulled in Ollama |
| `openai_compatible_base_url` | `http://localhost:11434/v1` | OpenAI-compatible base URL. Minutes appends `/chat/completions` unless it is already present. |
| `openai_compatible_model` | `"llama3.2"` | Model name for the compatible endpoint. |
| `openai_compatible_api_key_env` | unset | Optional environment variable name containing the API key. Leave blank for local servers. The desktop app can also save a gateway key in macOS Keychain and use that runtime secret for non-local endpoints without rewriting shared config. |
| `mistral_model` | `"mistral-large-latest"` | Mistral API model |
| `chunk_max_tokens` | `4000` | Max tokens per chunk when splitting long transcripts |
| `speaker_mapping_timeout_secs` | `30` | Tight timeout for the Level-1 speaker-naming LLM call. On the agent path it also runs with no MCP servers and no tools so it can't hang on init. Clamped to [5, 120]. |

For Pi coding-agent support, use `engine = "agent"` with `agent_command = "pi"`.
Minutes invokes Pi in non-interactive, no-tools mode. This is distinct from
Inflection's Pi models; do not route transcript data to Inflection unless the
user explicitly opts into that provider and its data terms.

For OpenRouter, Vercel AI Gateway, Cloudflare AI Gateway, llama.cpp,
LM Studio, vLLM, LocalAI, or any other OpenAI-compatible server, use one
generic backend instead of adding a provider-specific engine:

The local path is simplest and does not require an API key. In the desktop app,
cloud gateways can be set up from Settings by pasting a key once; Minutes stores
it in macOS Keychain, keeps the raw secret out of `config.toml`, and leaves this
shared config field blank unless you explicitly choose an env-var-driven setup.

For CLI and power-user setups, set the key in your environment and put only the
variable name in config. Minutes never stores the raw key in `config.toml`.

```toml
[summarization]
engine = "openai-compatible"
openai_compatible_base_url = "https://openrouter.ai/api/v1"
openai_compatible_model = "openai/gpt-4o-mini"
openai_compatible_api_key_env = "OPENROUTER_API_KEY"
```

Local servers can omit the API key env:

```toml
[summarization]
engine = "openai-compatible"
openai_compatible_base_url = "http://localhost:11434/v1"
openai_compatible_model = "llama3.2"
openai_compatible_api_key_env = ""
```

Screenshot context requires an endpoint that accepts OpenAI-style image content
parts. Text-only summaries use plain string chat content for broader local
server compatibility.

### `[copilot]` — real-time nudge stream

This section is deliberately separate from `[summarization]`: the copilot is a
latency-bounded live consumer, while summarization runs after recording. An
explicit `minutes copilot start` may start a foreground session when
`enabled = false`; the flag controls implicit startup by future host surfaces.

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Allow implicit copilot startup; explicit CLI startup remains available. |
| `surface` | `"tui"` | Default CLI surface: `"tui"` or newline-delimited JSON with `"stdout"`. |
| `mode` | `"generic"` | Default session policy: `sales`, `discovery`, `interview`, `negotiation`, `difficult-conversation`, `decision`, or `generic`; `--mode` overrides it. |
| `fast_provider` | `"auto-local"` | Fast-lane provider request. `"auto-local"` probes eligible local providers at session start and selects a healthy model within the routing policy; Apple Foundation Models remains a provider stub until the separate native fast-follow lands. |
| `fast_model` | `"qwen3.5:4b"` | Ollama model used for structured nudges. `minutes coach setup` replaces manifest defaults with the strongest hardware-fit model that passes its latency probe. |
| `allow_cloud` | `false` | Cloud opt-in gate. Cloud adapters are intentionally not implemented in the first copilot release. |
| `meeting_goal` | unset | Optional default outcome Coach should optimize for. |
| `arming_behavior` | `"ask-each-meeting"` | Desktop behavior at recording start: `"automatic"`, `"ask-each-meeting"`, or `"off"` (manual starts remain available). |
| `critical_notifications_only` | `true` | Suppress non-critical Coach notifications. |
| `onboarding_seen` | `false` | Desktop first-run explainer state. Managed by the app. |
| `nudge_ttl_ms` | `12000` | Lifetime of a rendered nudge in milliseconds. |
| `target_latency_ms` | `5000` | Fast request timeout/latency target; timeout degrades only the copilot. |
| `history_grounding` | `true` | Refresh a bounded battle card asynchronously from unrestricted graph, structured intent, and FTS data. |
| `live_partials` | `true` | Enable ephemeral partial coaching when `minutes copilot start --live` owns a streaming Whisper session in-process. External capture remains `final_only`. |
| `partial_debounce_ms` | `250` | Coalesce rapid partial corrections before starting the fast model lane. |
| `depth_refresh_secs` | `60` | Slow strategy refresh cadence, clamped to 30–90 seconds; topic shifts and decisive finals may refresh earlier. |
| `grounding_refresh_secs` | `15` | Minimum stable-final grounding cadence; topic shifts bypass it. Retrieval never runs on capture or the fast path. |

Ollama defaults to `http://localhost:11434`. Set `OLLAMA_HOST` to use another
local endpoint. Meeting text and history are passed as delimited untrusted data,
the loop exposes no tools, and restricted meetings are excluded from every
battle-card source. Dismissed/helpful/not-helpful feedback changes only bounded
session cadence and confidence gates. See
[RFC 0004](../rfcs/0004-copilot-realtime-stream.md).

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
| `capture_backend` | `"cpal"` | System-audio backend: `"cpal"` for loopback devices, or opt-in `"core-audio-tap"` on macOS 14.4+ |

### `[recording.sources]` — multi-source capture

| key | default | meaning |
|---|---|---|
| `voice` | unset | Voice (mic) device name, or `"default"` |
| `call` | unset | Call (system audio) device name, or `"auto"` to detect loopback |

When `capture_backend = "core-audio-tap"`, set `call = "auto"`. The backend
captures the default macOS system output via Core Audio Process Tap instead of
opening a named loopback input device.

### `[consent]` — recording disclosure aid

For meeting recordings, Minutes can show a disclosure reminder, ask for an
interactive acknowledgement, and write the selected basis into frontmatter.
Non-interactive callers are never blocked: if `mode = "require"` is used from
a non-TTY process without `--consent`, Minutes records the basis as
`unattested` and prints a warning.

| key | default | meaning |
|---|---|---|
| `mode` | `"remind"` | `"off"` skips reminder text, `"remind"` prints the script, `"require"` asks for an interactive acknowledgement only when stdin is a TTY |
| `disclosure_script` | built-in local transcript disclosure | One-line script to read aloud or paste before recording |
| `default_basis` | unset | Optional basis stamped when no `--consent` flag is provided |

Supported basis values are `verbal_all_parties`, `notice_in_invite`,
`recorded_disclosed`, `na`, and `unattested`.

```toml
[consent]
mode = "remind"
disclosure_script = "Heads up: I'm using Minutes to transcribe this conversation locally on my device for my own notes. Let me know if you'd prefer I didn't."
# default_basis = "notice_in_invite"
```

CLI flags override the config for a single recording:

```bash
minutes record --consent verbal_all_parties
minutes record --consent notice_in_invite --consent-notice "Notice was included in the calendar invite."
```

### Sensitive meeting frontmatter

`minutes sensitive start` opens a no-capture meeting session for typed markers.
`minutes sensitive stop` writes a regular markdown meeting artifact, but marks
the capture and sensitivity policy explicitly:

```yaml
capture: none
sensitivity: restricted
consent: na
debrief: pending
```

`debrief: pending` is present only when the session is saved without any typed
debrief content. Non-interactive callers never wait for prompts; they save the
artifact immediately and leave the debrief status for a later assistant pass.

### `[retention]` — raw audio policy

Minutes treats markdown transcripts, summaries, graph/search data, and metadata
as the durable library. Raw audio is a short-lived recovery/reprocessing layer
unless pinned.

| key | default | meaning |
|---|---|---|
| `successful_audio_days` | `30` | Days to keep raw audio for successfully processed recordings |
| `failed_audio_days` | `90` | Days to keep raw audio for failed or needs-review recordings |
| `restricted_audio_days` | `7` | Days to keep raw audio for `sensitivity: restricted` meetings (tighter window for sensitive content) |
| `keep_pinned_audio` | `true` | Keep audio when meeting frontmatter has `audio_retention: pinned` |
| `auto_cleanup` | `false` | Reserved for future automatic cleanup runners; current CLI cleanup requires explicit `--apply` |
| `cleanup_on_startup` | `false` | Reserved for future startup cleanup |
| `warn_above_gb` | `2` | Threshold for surfacing raw-audio storage warnings |

`restricted_audio_days` applies to meetings marked `sensitivity: restricted`:
sensitive content is held on a tighter window than normal recordings, and the
sensitivity tier takes precedence over the successful/failed classification. An
explicit `audio_retention: pinned` still wins (operator intent). Set it to `0`
to make restricted audio delete-eligible the day after recording.

Inspect storage with `minutes storage`. Preview cleanup with `minutes cleanup`;
delete candidates only with the explicit `minutes cleanup --apply`. As with all
retention, nothing is deleted automatically unless `auto_cleanup` is enabled.

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
| `backend` | `"whisper"` | Final transcription backend. Retained `"apple-speech"` and `"parakeet"` values currently resolve to sealed Whisper because their pathname-only helpers lack a secure private-audio transport. |
| `destination` | `"clipboard"` | `"clipboard"`, `"file"`, or `"command"` |
| `destination_file` | unset | Target file when `destination = "file"` |
| `destination_command` | unset | Shell command when `destination = "command"` |
| `accumulate` | `true` | Append successive utterances rather than replacing |
| `daily_note_log` | `true` | Append every dictation to the daily note |
| `auto_paste` | `false` | Paste the result immediately after dictation ends when the platform can do that honestly |
| `silence_timeout_ms` | `2000` | Silence threshold that ends a dictation session |
| `max_utterance_secs` | `120` | Force-finalize an utterance at this length |
| `model` | `"base"` | Whisper model for dictation |
| `cleanup_engine` | `"rules"` | Text cleanup applied to each utterance: `"rules"` (deterministic on-device: capitalization, fillers, vocab) or `"none"`/`"off"` for raw ASR. `"ollama"` is reserved and currently falls back to rules |
| `cleanup_remove_fillers` | `true` | Remove conservative filler words ("um", "uh") |
| `cleanup_spoken_punctuation` | `false` | Convert spoken commands ("period", "new line") to punctuation. Opt-in: collides with those words used as content |
| `cleanup_apply_vocabulary` | `false` | Apply your vocabulary store (Term/Acronym entries) as casing/replacement fixes (e.g. "gpt" → "GPT") |

Dictation clipboard behavior is platform-specific:

- macOS uses `pbcopy` / `pbpaste`; desktop auto-paste requires Accessibility permission and reports whether Minutes verified typing or only pasted.
- Linux clipboard output uses `wl-copy` / `wl-paste` from `wl-clipboard` on Wayland, or `xclip` / `xsel` on X11. Desktop auto-paste copies first, then tries `xdotool` only in an X11 session; Wayland remains copy-only because compositors do not expose one universal paste-injection path.
- Windows CLI clipboard output uses `clip.exe`; desktop active-app insertion is not claimed yet.

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
| `agent_command` | `"claude"` | Agent CLI when engine = `"agent"` (`claude`, `codex`, `opencode`, `pi`, etc.) |
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
| `save_wav` | `true` | Save raw WAV so the stopped session can be preserved or processed |
| `promote_on_stop` | `"process"` | `"process"` creates a normal meeting; `"preserve"` keeps a timestamped WAV/JSONL pair only; `"off"` leaves the overwrite-prone fixed slot unchanged |
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

### `[screen_context]` — recording-time screenshots

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Capture periodic screenshots during recording |
| `interval_secs` | `30` | Seconds between captures |
| `keep_after_summary` | `false` | Retain screenshots after summarization |

This section is intentionally narrow:

- it only affects screenshots during an active recording
- it is off by default
- it is not a general ambient desktop-capture mode
- it is independent of `[desktop_context]`, which stores app/window metadata
- screenshots are retrieved on demand with `minutes context screen` or the
  read-only MCP `get_screen_context` tool; they are not attached to every query
- assistants must not claim they can see the screen unless they opened or were
  delivered a specific returned image

Use `minutes context status --json` to inspect the observed runtime state. Use
`minutes context screen --session <id> --at <rfc3339-time> --limit 1 --json` to
retrieve bounded, verified PNG paths nearest a meeting moment. A limit above
three is rejected. When `keep_after_summary` is false, cleanup removes both the
PNG files and their readable context-store references.

### `[desktop_context]` — meeting-adjacent app/window context

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | Capture app/window context during recordings and live sessions |
| `capture_window_titles` | `true` | Include focused window titles when macOS Accessibility is available |
| `capture_browser_context` | `false` | Opt in to browser-page title context (URL/domain enrichment remains deferred) |
| `allowed_apps` | `[]` | Optional allowlist of app or bundle-id fragments |
| `denied_apps` | `[]` | Optional denylist of app or bundle-id fragments |

This section is the policy layer for meeting-adjacent desktop context:

- it is off by default
- it applies to recording/live-session context, not a 24/7 ambient mode
- app filters are enforced today
- domain lists are forward-compatible policy hooks for future browser URL enrichment

For real desktop validation of the Windows and Linux collectors, use
[../checklists/desktop-context-runtime-checklist.md](../checklists/desktop-context-runtime-checklist.md).

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
