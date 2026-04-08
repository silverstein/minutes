# Call Capture Handoff — 2026-04-08

This document is the handoff point for the next Codex session.

## Source Of Truth

- Clean branch: `fix/call-capture-mainline`
- Clean worktree: `/Users/silverbook/Sites/minutes-mainline-call-capture`
- PR to review / merge: [#96](https://github.com/silverstein/minutes/pull/96)

Do **not** use the older `fix/call-capture-hardening` worktree as the main
source of truth. That workspace accumulated unrelated local design/theme edits.

## What Was Landed On `fix/call-capture-mainline`

Major call-capture commits on top of latest `origin/main`:

- `4f0d1ec` `fix(call-capture): harden desktop detection and attribution`
- `5c18758` `fix(call-capture): add real levels and self attribution`
- `0311707` `fix(call-detect): keep recent Meet wins sticky`
- `179d63f` `fix(call-capture): finalize waveform and self naming`
- `53919ee` `fix(call-capture): map unknown solo speaker to self`
- `65ad402` `fix(call-capture): relabel solo system-dominant voice`
- `f770acc` `fix(call-capture): map solo stem speaker to self`
- `32465d7` `fix(diarize): assign dominant speaker to short unknown clips`
- `01f00c2` `fix(call-capture): tighten solo self attribution`
- `362c118` `fix(call-capture): use effective speaker labels for self mapping`

## What Is Verified Working

### Native helper / capture

- Native macOS helper finalization bug was real and is fixed.
- Native helper now produces valid multi-second captures instead of ~2 KB junk
  files.
- Helper emits live health booleans and real numeric levels:
  - `mic_live`
  - `call_audio_live`
  - `mic_level`
  - `call_audio_level`

### Detection / desktop UX

- Google Meet detection works in the desktop app.
- Desktop UI can route the generic in-window CTA through call intent when a
  visible call session is active.
- App no longer force-focuses itself on call detection.
- Sticky recent-Meet logic exists so a successful browser Meet hit can stay
  authoritative for a short window.

### UI truthfulness

- Recording chips now reflect continuously refreshed helper health instead of a
  stale startup snapshot.
- Native call waveform is no longer a flat dead bar because helper-derived
  levels are threaded through.

### Diarization improvements

- Stem-based diarization collapses strongly correlated voice/system energy to a
  single speaker instead of inventing a second human too eagerly.
- Solo/system-dominant single-speaker case can be normalized back toward the
  local speaker.
- `apply_speakers` has a dominant-speaker fallback for short clips whose
  transcript starts before the first diarization segment.

## What Was Ruled Out

- This is **not** just “the helper only emitted one sample buffer.”
- This is **not** just “the dev app can’t see the enrolled voice.”
- This is **not** just “Google Meet detection wasn’t enabled.”
- This is **not** just “the user tested the wrong path” in every case:
  some runs were definitely native-call (`~/.minutes/native-captures/*.mov`).

## What We Learned From Dogfood Runs

### Wrong-path runs

Several confusing runs turned out to be generic mic/room path:

- artifact processed from `~/.minutes/jobs/job-...wav`
- no fresh `~/.minutes/native-captures/*call.mov`

Those runs are not useful for evaluating native-call self attribution.

### Native-call runs

Confirmed native-call runs processed from:

- `~/.minutes/native-captures/2026-04-08-072601-call.mov`
- `~/.minutes/native-captures/2026-04-08-073322-call.mov`
- `~/.minutes/native-captures/2026-04-08-083713-call.mov`

Observed behaviors:

1. `07:26` run
- native path confirmed
- diarize log: `segments=6`, `speakers=2`

2. `07:33` run
- native path confirmed
- transcript came out `UNKNOWN`
- diarize log: `segments=2`, `speakers=2`
- likely short-clip alignment issue

3. `08:37` run
- native path confirmed
- transcript still `UNKNOWN`
- diarize log: `segments=1`, `speakers=0`
- voice stem was effectively tiny / near-empty in at least one earlier similar
  native run (`call.voice.wav` around 6 KB), suggesting the mic stem can be
  weak or absent on some runs

## Current Best Theory

The remaining unsolved issue is **not** mostly about summary/LLM naming.

The real problem appears to be:

1. Native call capture sometimes yields a weak / near-empty voice stem.
2. On short clips, diarization and transcript timestamps can still misalign.
3. When the final transcript stays `UNKNOWN` (or never gets a stable speaker
   label tied to the local voice stem), self-attribution cannot promote the
   speaker to `Mat`.
4. The LLM/summarizer may still guess a name from transcript content, but that
   is not true enrolled-voice attribution.

## Important Distinction

There are two layers:

- Layer A: anonymous speaker labeling
  - `SPEAKER_0`, `SPEAKER_1`, `UNKNOWN`
- Layer B: identity mapping
  - `Mat`, `Matt`, etc.

The remaining bug is mostly that Layer A is still unstable on some short native
call captures, which prevents Layer B from working reliably.

## Why `identity.name = "Mat"` Was Not Sufficient

`config.identity.name` is **not** only used for `recorded_by`.

It is used in:

- deterministic 1-on-1 mapping
- single-speaker stem/self attribution
- transcript rewrite for high-confidence attributions

However, those only help **after** the transcript has a stable anonymous speaker
label the pipeline can confidently map. If the transcript stays `UNKNOWN`, the
identity logic has nothing stable to rewrite.

## Follow-up Issues Already Filed

- `minutes-74q` — Align tray and palette recording starts with active call session
- `minutes-8a5` — Fix Slack-vs-Meet sticky call detection precedence
- `minutes-8qr` — Surface browser automation permission/backoff state for Meet detection
- `minutes-yto` — Instrument startup phases and investigate first-open freeze
- `minutes-22q` — Surface capture backend/path in UI and artifact metadata

## What Still Needs To Be Done

### Highest priority

1. Add instrumentation for attribution decisions

Specifically log for each processed meeting:

- `capture_backend`
- `diarization_from_stems`
- raw `diarization_num_speakers`
- effective transcript speaker labels after `apply_speakers`
- whether self-attribution helper returned `Some` or `None`
- if `None`, the reason (no stable label, no self profile, empty voice stem,
  already mapped, etc.)
- final `speaker_map` contents with `source` and `confidence`

Without this, further debugging remains too guessy.

2. Add explicit capture backend metadata

Dogfood repeatedly confused generic mic/room path with native call path.
`minutes-22q` exists for this.

### Secondary

3. Startup freeze instrumentation (`minutes-yto`)
4. Slack-vs-Meet precedence polish (`minutes-8a5`)
5. Browser automation / backoff UX (`minutes-8qr`)
6. Tray / palette parity (`minutes-74q`)

## Suggested Next Session Plan

1. Work only in `/Users/silverbook/Sites/minutes-mainline-call-capture`
2. Stay on branch `fix/call-capture-mainline`
3. Add attribution instrumentation
4. Rebuild / reinstall `Minutes Dev.app`
5. Run one short native-call solo repro
6. Inspect:
   - the new markdown artifact
   - `~/.minutes/logs/minutes.log`
   - latest `~/.minutes/native-captures/*`
7. Decide whether the next fix belongs in:
   - helper / voice stem capture
   - `apply_speakers`
   - self-attribution helper
   - or explicit fallback policy for single-speaker native call captures

## Current Recommendation

Do **not** keep piling on blind heuristics without the instrumentation above.

The problem is now small enough that better logs will likely save more time than
another speculative patch.
