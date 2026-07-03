# Investigation: Cristy's Blank 50-Minute Transcript

**Date:** 2026-03-31
**Reporter:** Cristy Valdés (Product Designer, C3 AI/Etsy/Oracle)
**Reported via:** LinkedIn DM + structured feedback PDF

---

## What happened

Cristy recorded a 50-minute interview via Minutes (through Claude Desktop MCP). When she stopped the recording:

1. The stop command **timed out twice** before going through
2. The transcript came back **completely blank**
3. The **raw audio file was gone** — no WAV on disk to retry with
4. A 50-minute interview was unrecoverable

## Root cause analysis

There are likely **two separate issues stacked**, not one. Either alone could explain the blank transcript, and together they made it unrecoverable.

### Issue 1: Stop + transcribe were coupled (FIXED)

**Before commit `cd59237` (March 30, 3:28 PM)**, the old `cmd_record` flow was:

```
record_to_wav()          → WAV written to disk continuously
  ↓ (stop signal)
WAV finalized on disk
  ↓
pipeline::process()      → transcribe + diarize + summarize (INLINE, same process)
  ↓
Write result JSON
  ↓
Delete WAV               → fs::remove_file(&wav_path)
Remove PID file
```

The critical problem: `cmd_stop` polled for PID file removal with a **120-second CLI timeout**, and the MCP server had a **180-second timeout** (`runMinutes(["stop"], 180000)`). A 50-minute recording with the `small` whisper model could take 5-10+ minutes to transcribe. When the MCP timeout fired at 180s, Claude got an error — but the recording process was still running in the background, mid-transcription.

When Cristy retried `stop`, the state was confused. Eventually the original process finished or was killed, and the old code **deleted the WAV** (`std::fs::remove_file(&wav_path).ok()`). That's why the audio was gone.

**This is now fixed.** Commit `cd59237` decoupled stop from processing:

```
record_to_wav()          → WAV written to disk continuously
  ↓ (stop signal)
WAV finalized on disk
  ↓
Queue background job     → returns immediately with "queued" status
Spawn worker process     → transcription runs in separate process
Remove PID file          → cmd_stop sees this quickly, returns to user
```

The WAV is preserved in the job queue and only cleaned up after successful processing.

### Issue 2: Headphone mic isolation (OPEN — the bigger problem)

Cristy uses Microsoft Teams for calls. Her feedback PDF mentions a "mic conflict with Teams" and she notes: "Other recordings that worked were likely not in concurrent Teams calls."

**Research finding: Teams does NOT lock the mic.** CoreAudio on macOS allows multiple apps to share the same physical microphone simultaneously. There is no `kAudioDevicePropertyHogMode` conflict. Minutes and Teams can both read from the same mic at the same time.

**The real issue is different.** When recording a video call:

| Audio setup | What Minutes captures |
|---|---|
| **In-person meeting** | Everyone in the room (mic hears all voices) |
| **Call on speakers** | Both sides — your voice directly + remote voices through speakers |
| **Call on headphones** | **Your voice only** — remote audio goes to headphones, never hits the mic |

If Cristy was wearing headphones during a 50-minute Teams interview where she was mostly listening, Minutes captured mostly silence with occasional speech from her. Whisper's anti-hallucination pipeline (7 layers of filtering) would strip most segments, and the `min_words = 3` threshold would mark it as `NoSpeech`.

**This is the product gap.** Minutes' value proposition is recording both sides of any conversation. The fact that headphone users silently get a one-sided (or blank) recording is a UX failure. There's no warning, no error — it just doesn't work, and you find out after the meeting is over.

### How these two issues compound

1. Recording captures mostly silence (headphones → only her voice, and she was mostly listening)
2. Whisper transcription runs for minutes on 50 minutes of mostly-silence audio
3. Anti-hallucination filters strip the few segments that were detected
4. MCP timeout fires at 180 seconds — user gets an error
5. User retries — state confusion
6. Original process finishes, deletes WAV
7. User is left with nothing

## What's already been fixed

| Fix | Commit | Date |
|---|---|---|
| Decouple stop from transcribe | `cd59237` | 2026-03-30 |
| Background processing queue | `b4430a3` | 2026-03-30 |
| Show record button during processing | `946b083` | 2026-03-31 |
| Filter diagnostics for blank transcripts | `699d279` | 2026-03-31 |
| Default model changed tiny → small | `d4c76c3` | 2026-03-31 |
| Calendar prompt can be disabled | (pending) | 2026-03-31 |

## What still needs to be fixed

### 1. System audio capture for call recording (HIGH PRIORITY)

Minutes needs to capture system audio — the audio output that goes to speakers/headphones — to record both sides of a call regardless of audio setup.

**Options, in order of feasibility:**

**A. CoreAudio Taps (macOS 14.4+) — recommended long-term**
- `AudioHardwareCreateProcessTap` lets any app capture audio from a specific process or the entire system
- No virtual device setup needed — zero config for the user
- Requires one-time Screen Recording or audio capture permission
- Apple's recommended approach going forward
- Rust bindings via `coreaudio-rs` / `coreaudio-sys`
- Reference: [Apple docs](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps), [AudioCap sample](https://github.com/insidegui/AudioCap)

**B. BlackHole auto-detection — medium-term**
- Minutes already documents BlackHole support in `capture.rs` comments
- Could auto-detect if BlackHole is installed and offer to use it
- Requires user to set up an Aggregate Device (fragile, can reset)
- `--device` flag already exists for manual device selection

**C. Conferencing app detection + warning — quick win**
- Check running processes for Teams/Zoom/Meet/FaceTime at recording start
- If detected + recording from default mic + headphones likely connected: warn user
- "You're on a Teams call. Minutes will only capture your microphone. To capture both sides, use speakers or set up BlackHole."
- Low effort, prevents the "blank transcript surprise"

**D. ScreenCaptureKit (macOS 13+)**
- Can capture application audio without screen content
- Requires Screen Recording permission
- Apple recommends CoreAudio Taps over this for audio-only use cases on 14.4+

### 2. Conferencing app detection warning (QUICK WIN)

Even before implementing system audio capture, we should detect when a conferencing app is active and warn the user. This prevents the blank transcript surprise without any audio engineering.

Implementation:
- At recording start, check if Teams/Zoom/Meet/FaceTime is running (process list or `NSWorkspace`)
- If detected, log a warning and emit it to the user
- Could also check if headphones are connected via CoreAudio device properties
- Already have `call_detection` config with `apps: ["zoom.us", "Microsoft Teams", "FaceTime", "Webex", "Slack"]`

### 3. Calendar prompt dismissal (DONE — pending commit)

Added `[calendar] enabled = false` config option. When set, the Tauri app skips the calendar polling loop entirely — no more AppleScript permission prompts on launch.

Files changed:
- `crates/core/src/config.rs` — `CalendarConfig { enabled: bool }` (default true)
- `tauri/src-tauri/src/main.rs` — calendar polling gated on config
- `crates/cli/src/main.rs` — post-recording calendar lookup gated on config
- `crates/core/src/health.rs` — health check shows "disabled" instead of triggering AppleScript

## Questions for assessment

1. **CoreAudio Taps vs. BlackHole** — Is CoreAudio Taps (`AudioHardwareCreateProcessTap`) mature enough to rely on? It's macOS 14.4+ only, which cuts out some users. Should we implement both paths (taps on 14.4+, BlackHole guidance on older)?

2. **Default behavior** — Should Minutes automatically capture system audio when it detects a conferencing app, or should it always require explicit opt-in? Auto-capture is better UX but requires Screen Recording permission.

3. **Scope** — Should system audio capture be a `--system-audio` flag, a config option (`[recording] capture_system_audio = true`), or auto-detected based on context?

4. **Cristy's recording** — Is there any possibility the WAV survived somewhere? The old code had `std::fs::remove_file(&wav_path).ok()` but that was in the success path. If the process was killed mid-transcription, the WAV might still be at `~/.minutes/current.wav` (unlikely but worth checking if she still has it).
