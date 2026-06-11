# Plan — Every Name, Right (multilingual name accuracy epic)

Author: Claude (strategy session 2026-06-10, written 2026-06-11)
Status: scoped, beaded; runs parallel to consent Phase 2 Wave 1.

## Why this, why now

Three of the project's most engaged users hit the same wall independently:
a Dutch-speaking contributor whose own name transcribes as "Bert", a French
hospital physician (RFC 0001's `medical-fr` owner), and a bilingual
Spanish/English tester. Published ASR research confirms the gap is general:
accented speech draws materially higher error rates, and named-entity
biasing is the active frontier, not a solved problem.

The decisive observation: **this is not a European edge case.** Any diverse
workplace is full of names general ASR mangles. "Geert" -> "Bert" is the
same failure as "Nguyen" -> "Wynn" in an Austin standup.

And it is foundational, not cosmetic: the knowledge graph canonicalizes
entities by name. Wrong names in transcripts poison people-entities,
commitments, and cross-meeting queries. The agent layer is only as good as
its names.

## What already exists (build on, don't rebuild)

- `crates/core/src/vocabulary.rs`: local vocabulary store (names, terms,
  aliases) with `decode_phrases()` feeding transcription hints; CLI surface
  (`minutes vocabulary list/add/remove/suggest/rebuild`).
- `build_decode_hints` (pipeline.rs): calendar attendees + identity name +
  vocabulary -> whisper initial prompt ("Preserve spelling exactly") and
  parakeet boost phrases. Caps: 8 priority / 12 combined.
- Identity (`[identity] name/emails/aliases` + Settings UI fields).
- Speaker attribution L0-L3 with confidence gating; the desktop "Remember"
  button saves high-confidence people to vocabulary.
- Voice profiles (`voices.db`) for voice-based identification.

The epic is mostly composition + a correction pass, not new infrastructure.

## Scope (wave order)

### 1. Name-accuracy eval harness (measure first)
A fixture corpus of short audio clips with non-Anglo names (Dutch, French,
Spanish, Indian, Chinese, Vietnamese name sets) + ground-truth transcripts.
Score: name-token WER before/after each lever. Without this, every change
below is vibes. Public fixtures must use synthetic/consented voices.

### 2. Make the existing levers discoverable (UX, cheap)
- Vocabulary gets a Settings section in Tauri (list + add + remove), not
  CLI-only; "Remember" already writes to it.
- First-run and post-meeting nudges: when attribution confidence is high
  but the transcript spelling differs from a calendar attendee name,
  offer one-tap "use calendar spelling".
- Docs: a "Getting names right" guide page (site + repo).

### 3. Hint pipeline upgrades (engine, medium)
- Feed graph people-entities into decode hints (recent/frequent people),
  not just calendar + vocabulary; revisit the 8/12 caps with eval data.
- Per-meeting hint assembly logged into frontmatter provenance so users
  can see which names were hinted.

### 4. Post-pass name correction (the big lever)
Fuzzy/phonetic match of transcript name-tokens against the expected-name
pool (attendees, vocabulary, graph people) with conservative thresholds:
phonetic distance (e.g. Double Metaphone) + edit distance + speaker-turn
context. Correction is annotated, never silent: corrected tokens carry
provenance, and the raw token is preserved (same philosophy as
whisper-guard's keep_dedup_annotations). Wrong corrections are worse than
wrong transcriptions, so default thresholds are strict and the pass is
config-gated.

### 5. Multilingual backend evaluation (exploratory, last)
Evaluate SenseVoice via a native path (sherpa-onnx or .cpp port; never the
Python funasr package) against the eval harness, alongside existing
whisper multilingual models and parakeet tdt-600m v3. Issue #265 tracks
community interest. Adopt only if the harness shows a real name/accent win.

## Constraints

- Local-first: every lever runs on-device. No cloud correction services.
- Annotated, reversible corrections only; agents never mutate raw human
  transcript text silently (RFC #194 spirit).
- Per-surface tests; eval harness runs in CI on the fast (text-level)
  layers, audio layers behind a feature gate like real-whisper tests.
- Design partner: gtheys (Dutch/ESL environment, Linux); medical-fr
  context from ed0c informs the French name set.

## Out of scope

- Per-person redaction/exclusion (consent Phase 2 v3 territory).
- Cloud ASR backends of any kind.
- Real-time correction during live transcript (post-pass first; live can
  follow once thresholds are proven).
