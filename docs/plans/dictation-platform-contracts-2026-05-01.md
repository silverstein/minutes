# Dictation Platform Contracts - 2026-05-01

## Why this exists

Minutes already has a useful dictation path: microphone capture, VAD,
streaming Whisper partials, clipboard output, saved dictation files, daily note
logging, and a compact desktop overlay.

That path is also too tightly shaped around "Whisper turns speech into text,
then Minutes writes text somewhere." The next dictation push needs a wider
contract:

- multiple local engines with different capabilities
- verified insertion into the active app, not just clipboard writes
- deterministic cleanup and command handling before optional LLM polish
- durable local history that can feed recents, search, Recall, and MCP
- platform-specific honesty for macOS, Linux, Windows, CLI, and desktop

The product promise is:

> Speak anywhere. Minutes types it, remembers it locally, and makes it useful to
> your agents.

That only works if the core contracts separate capability from implementation.

## Current anchors

Current code to preserve while extending the system:

- `crates/core/src/dictation.rs` owns the live dictation audio loop, VAD,
  streaming Whisper partials, utterance finalization, clipboard writes, dictation
  markdown output, and daily note append.
- `crates/core/src/config.rs` exposes `DictationConfig` with destination,
  accumulation, cleanup engine, auto-paste, shortcut, hotkey, model, and timing
  fields.
- `tauri/src-tauri/src/commands.rs` maps `DictationEvent` into overlay events
  and contains a separate macOS clipboard/paste helper for latest artifacts.
- `tauri/src/dictation-overlay.html` renders the live dictation state machine.
- `docs/architecture/apple-speech.md` says Apple Speech is standalone-live-only today, not
  dictation.
- `docs/architecture/parakeet.md` says dictation still uses Whisper because current Parakeet
  live integrations are utterance-granular and do not match dictation partials.
- `crates/core/src/vocabulary.rs` already defines the local vocabulary store for
  names, organizations, projects, acronyms, and recurring terms.

## Non-goals

- Do not automate Apple system Dictation.
- Do not make cloud speech-to-text the default.
- Do not switch the default dictation engine from intuition alone.
- Do not claim universal Linux Wayland typing until it is verified per
  compositor.
- Do not mutate raw transcripts with vocabulary guesses.
- Do not make a broad Swift rewrite a prerequisite for improving dictation.

## Contract 1: DictationEngine

`DictationEngine` is the speech-to-text boundary. It should describe what an
engine can do before the UI or runtime asks it to do that thing.

The first implementation can be an internal Rust trait or facade. The important
part is the payload shape, not the exact trait syntax.

```rust
pub struct DictationEngineDescriptor {
    pub id: DictationEngineId,
    pub label: String,
    pub local_only: bool,
    pub platform_availability: PlatformAvailability,
    pub readiness: DictationEngineReadiness,
    pub latency_profile: DictationLatencyProfile,
    pub supports_partials: bool,
    pub supports_streaming_end_of_utterance: bool,
    pub supports_context_hints: bool,
    pub vocabulary_mode: DictationVocabularyMode,
    pub vocabulary_budget: DictationVocabularyBudget,
}
```

Minimum engine ids:

- `whisper-streaming`: current dictation path
- `apple-speech`: macOS capability-gated Apple Speech path, not default until
  benchmarked on real dictation audio
- `parakeet-fluidaudio`: macOS helper or native path for Parakeet/FluidAudio
- `parakeet-sidecar`: future advanced path only if partials/end-of-utterance are
  proven good enough

Readiness states:

- `ready`
- `needs_model_download`
- `needs_platform_permission`
- `unsupported_platform`
- `unsupported_os_version`
- `misconfigured`
- `disabled_by_build`
- `failed`

Latency profile fields:

- cold start estimate
- warm first-partial estimate
- warm final estimate
- whether the engine can keep a warm resident model
- measured timestamp/source for the estimate

Vocabulary modes:

- `none`: engine cannot consume hints
- `prompt`: Whisper-style prompt or prefix hints
- `contextual_strings`: Apple Speech-style contextual strings
- `boost`: explicit weighted terms or phrases
- `postprocess`: engine cannot consume hints, but cleanup can
- `hybrid`: more than one of the above

Design rule: users should see capability language, not backend jargon. The UI
can say "Fast local dictation is ready" or "Names and terms can help this
engine," while diagnostics can show the engine id and raw readiness details.

## Contract 2: TextInsertion

`TextInsertion` owns "what happened to the text after transcription." This is
separate from dictation output files because typing into another app has a
different risk profile than saving markdown.

```rust
pub struct TextInsertionRequest {
    pub text: String,
    pub mode: TextInsertionMode,
    pub restore_clipboard: bool,
    pub target_context: Option<ActiveTargetContext>,
}

pub enum TextInsertionMode {
    CopyOnly,
    PasteViaClipboard,
    DirectInsert,
    BestEffortVerified,
}

pub struct InsertResult {
    pub outcome: InsertOutcome,
    pub method: InsertMethod,
    pub verified: bool,
    pub clipboard_restored: bool,
    pub target_context: Option<ActiveTargetContext>,
    pub message: String,
}
```

Outcomes:

- `typed`: text was inserted and verified
- `pasted`: clipboard paste automation ran and the app accepted focus, but
  content verification may be unavailable
- `copied`: text is on the clipboard and the user can paste manually
- `failed`: insertion and copy failed
- `blocked`: permission or platform capability is missing

Methods:

- `clipboard_only`
- `clipboard_paste`
- `accessibility_direct`
- `x11_type`
- `wayland_portal_or_compositor`
- `windows_clipboard`
- `unsupported`

Target context should be privacy-aware and sparse:

- platform
- app name
- bundle id or process id when available
- focused element role when available
- browser URL/title only when already permitted by the desktop context layer
- text mode hint: `prose`, `terminal`, `code`, `chat`, `unknown`

Platform contract:

| Platform | V1 behavior | Verified typing gate |
| --- | --- | --- |
| macOS | copy always, then direct AX or clipboard paste when capability allows | accessibility permission plus before/after proof where possible |
| Linux/X11 | copy always, optional clipboard paste with known toolchain | `xclip` or `xsel` for clipboard; `xdotool` installed before paste automation is attempted |
| Linux/Wayland | copy always | `wl-clipboard` for clipboard; compositor-specific proof only; no universal paste claim |
| Windows | copy always | later SendInput/UI Automation implementation |
| CLI/headless | copy/file/stdout only | no active-app typing claim |

UX rule: the overlay must tell the truth. It should distinguish "Typed",
"Pasted", "Copied", and "Could not type. Copied instead." It should never show
a success state that implies active-app insertion when Minutes only changed the
clipboard.

## Contract 3: DictationPostProcessor

Post-processing is the text transformation boundary. It should be deterministic
by default and explicit when it becomes expensive, private-context-consuming, or
LLM-backed.

Pipeline order:

1. Normalize whitespace, punctuation spacing, and capitalization according to
   text mode.
2. Apply conservative spoken-command handling, such as "new line" or "press
   return", only in an enabled command mode.
3. Apply user replacements and snippets.
4. Apply vocabulary-aware correction suggestions.
5. Optionally run LLM polish only when the user selected that mode.

Context scopes:

- `general_prose`
- `chat`
- `email`
- `terminal`
- `code`
- `agent_prompt`
- `daily_note`

Rules:

- Terminal/code scopes should avoid sentence capitalization and smart
  punctuation.
- Vocabulary hints should produce provenance, not silent raw transcript
  mutation.
- Correction learning must start as suggestion-first. Do not globally learn a
  term or name just because one utterance looked similar.
- LLM polish must be visibly different from deterministic cleanup because it
  changes latency and privacy expectations.

Result shape:

```rust
pub struct DictationPostProcessResult {
    pub raw_text: String,
    pub cleaned_text: String,
    pub transformations: Vec<DictationTransformation>,
    pub vocabulary_used: Vec<String>,
    pub mode: DictationPostProcessMode,
}
```

## Contract 4: DictationMemory

Every successful or partially successful dictation should create durable local
provenance, even when insertion fails. This is the moat: Minutes is not just a
typing toy; the utterance becomes local memory that agents can inspect later.

```rust
pub struct DictationMemoryRecord {
    pub id: String,
    pub captured_at: DateTime<Local>,
    pub raw_text: String,
    pub cleaned_text: String,
    pub duration_secs: f64,
    pub engine_id: DictationEngineId,
    pub engine_descriptor_version: String,
    pub vocabulary_mode: DictationVocabularyMode,
    pub vocabulary_used: Vec<String>,
    pub insertion_outcome: InsertOutcome,
    pub insertion_method: InsertMethod,
    pub target_context: Option<ActiveTargetContext>,
    pub file_path: Option<PathBuf>,
    pub daily_note_appended: bool,
}
```

Storage should preserve the existing markdown output path while adding enough
frontmatter/provenance for recents, re-paste, search, graph, and MCP access.
The raw and cleaned text can both be kept when cleanup changes meaningfully.

Minimum product surfaces enabled by this contract:

- recent dictations
- copy or re-paste latest dictation
- raw/clean diff when cleanup changed text
- "Remember this term" from a recent dictation
- MCP/search access to latest dictations

## UI and UX state contract

The dictation overlay should expose the transition the user is waiting on.

States:

- `loading`: engine or model is warming
- `listening`: microphone is active, no speech detected
- `accumulating`: speech is being captured
- `partial`: live text preview is available
- `processing`: final text is being produced
- `cleaning`: deterministic or selected polish is running
- `inserting`: Minutes is attempting active-app insertion
- `typed`: verified inserted text
- `pasted`: paste automation completed, verification limited
- `copied`: text is available on clipboard
- `failed`: text was not produced or could not be copied
- `cancelled`: user stopped without a final output
- `yielded`: recording took priority

Five-second silence check:

- During engine warmup, show `loading`.
- During speech capture, show audio level or compact listening state.
- During final transcription, show `processing`.
- During post-processing, show `cleaning` only if it is not instant.
- During app insertion, show `inserting`.
- On fallback, show the real fallback: "Copied" is success, but it is not
  "Typed."

Settings should expose:

- active dictation engine
- readiness/setup state
- whether Minutes can type into other apps on this platform
- destination behavior: copy, type when possible, file/daily note
- deterministic cleanup mode
- optional LLM polish mode with privacy/latency wording
- names and terms toggle/status, not raw boost knobs

## Benchmark contract

Before changing defaults, run a dictation-specific benchmark. Reusing the
meeting transcription benchmark is not enough because dictation has tighter
latency and insertion requirements.

Cases:

- short command phrases
- multi-sentence prose
- terminal/code prompts
- names and project terms from vocabulary
- noisy mic, AirPods, and built-in mic
- at least one Linux environment if a Linux claim is being promoted

Compare:

- current Whisper dictation
- Apple Speech
- Parakeet/FluidAudio helper
- any Parakeet sidecar/native partial path

Metrics:

- first partial latency
- final latency
- WER
- required term recovery
- forbidden/hallucinated term rate
- punctuation quality
- insertion result and verification state
- user-visible state transitions

Promotion criteria:

- no default engine switch without corpus evidence
- no vocabulary boost default without proper-name regression evidence
- no "types anywhere" copy unless the insertion result is verified on that
  platform

## Suggested module ownership

This is the likely implementation shape, subject to code-review pressure once
the first bead starts:

- `crates/core/src/dictation_engine.rs`: engine descriptor, readiness, and
  engine-facing request/result types
- `crates/core/src/dictation_postprocess.rs`: deterministic cleanup pipeline and
  transformation provenance
- `crates/core/src/dictation_memory.rs`: durable record/frontmatter helpers
- `tauri/src-tauri/src/text_insertion.rs`: desktop active-app insertion,
  platform probes, and clipboard preservation
- `tauri/src-tauri/src/commands.rs`: Tauri command/event glue only
- `tauri/src/dictation-overlay.html`: state rendering and user feedback

The existing `crates/core/src/dictation.rs` should keep owning audio capture and
utterance lifecycle until the contracts are real enough to extract a coordinator
without breaking the working path.

## Bead handoff

- `minutes-xvxs`: this contract document.
- `minutes-posf`: implement macOS `TextInsertion` with verified result states.
- `minutes-7d4u`: build the dictation benchmark corpus and report.
- `minutes-z679`: productize Apple Speech for dictation behind capability gates.
- `minutes-m76k`: prototype FluidAudio/Parakeet helper against the
  `DictationEngine` contract.
- `minutes-tj1g`: add Linux clipboard and injection capability probe.
- `minutes-hpg6`: persist dictation memory/history/recents with provenance.
