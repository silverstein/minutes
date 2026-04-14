# Parakeet Boost Tuning — 2026-04-14

## Question

Can Parakeet knowledge-graph phrase boosting be turned on by default safely, or should it remain opt-in?

Current defaults before this investigation:

- `parakeet_boost_limit = 0`
- `parakeet_boost_score = 2.0`

The goal of this pass was not to prove that boosting can help in some cases. It was to find a regime that is safe enough to enable globally without making realistic transcriptions worse.

## Setup

- Build used: `cargo build --release -p minutes-cli --features parakeet`
- Runtime path used for every sweep row: the real `minutes parakeet-helper` path, not a synthetic harness
- Control: `parakeet_boost_limit = 0`
- Boosted runs: `parakeet_boost_limit = 25`
- Scores tested: `1.0`, `2.0`, `3.0`, `5.0`
- Model: `tdt-600m`
- VAD: native Silero VAD enabled
- Ground truth: existing meeting transcript sections on disk, extracted by timestamp from the corresponding `.md` file

Notes on pairing:

- Several recovered job JSONs had `title = null` and `output_path = null`.
- Pairing by job `started_at` was wrong for recovered jobs because that reflects when processing ran, not the meeting identity.
- The final pairing used `recording_started_at` plus a spot-transcribed sample to confirm the correct meeting markdown.

One available long WAV (`job-20260413130210152-33883-0.wav`) mapped to a failed "Garrett Weekly Media Meeting" recovery artifact without a corresponding stored transcript, so it was excluded from scored evaluation.

## Boost Phrase Set

With `boost_limit = 25`, the current graph-derived phrase list was:

`Matt`, `Wesley`, `Mat S.`, `Case`, `Mat Silverstein`, `Gordon`, `Andrew`, `Logan`, `Dan`, `Matt Mullenweg`, `Dieter`, `Alex C.`, `Alex`, `Case W.`, `Wesley Young`, `Mr. Young`, `Donald`, `Bobby McPherson`, `Garrett Gunderson`, `Tricia`, `Ryan`, `Dean`, `Brock`, `Mark Fister`, `Jerry`

This matters. The list includes some strong multi-token entities, but it also includes many short common first names (`Matt`, `Dan`, `Alex`, `Case`) that are plausible hallucination targets on imperfect audio.

## Clip Set

Seven scoreable clips were used:

- `clean_prep_opening` — louder single-speaker Wesley prep stretch
- `clean_asana_pillars` — longer explanatory monologue from the Wesley Asana call
- `proper_prep_x1_planning` — `X1` / `Claude` / `Planning Shepherd`
- `proper_prep_connectors` — `Asana` / `Google Drive` / `X1`
- `proper_prep_gordon_andrew` — `Gordon` / `Andrew`
- `noisy_team_intro` — low-volume Team Transition intro with multiple speakers
- `noisy_team_network_access` — low-volume Team Transition X1/network discussion

## Results Table

WER values below are relative to the stored transcript section for each clip.

| Clip | Category | 0.0 | 1.0 | 2.0 | 3.0 | 5.0 |
|---|---:|---:|---:|---:|---:|---:|
| `clean_prep_opening` | clean | 0.443 | 0.474 | 0.474 | 0.526 | 0.598 |
| `clean_asana_pillars` | clean | 0.144 | 0.144 | 0.144 | 0.144 | 0.144 |
| `proper_prep_x1_planning` | proper_noun | 0.130 | 0.130 | 0.130 | 0.130 | 0.130 |
| `proper_prep_connectors` | proper_noun | 0.340 | 0.330 | 0.330 | 0.369 | 0.388 |
| `proper_prep_gordon_andrew` | proper_noun | 0.257 | 0.257 | 0.257 | 0.257 | 0.257 |
| `noisy_team_intro` | noisy | 0.825 | 0.825 | 0.833 | 0.833 | 0.625 |
| `noisy_team_network_access` | noisy | 0.158 | 0.178 | 0.178 | 0.178 | 0.178 |

Category means:

| Category | 0.0 | 1.0 | 2.0 | 3.0 | 5.0 |
|---|---:|---:|---:|---:|---:|
| clean | 0.294 | 0.309 | 0.309 | 0.335 | 0.371 |
| proper_noun | 0.242 | 0.239 | 0.239 | 0.252 | 0.259 |
| noisy | 0.492 | 0.502 | 0.506 | 0.506 | 0.402 |
| overall | 0.328 | 0.334 | 0.335 | 0.348 | 0.332 |

## What Actually Happened

### 1. Low positive scores helped one connector-heavy clip a little

`proper_prep_connectors` improved from `0.340` WER at control to `0.330` at scores `1.0` and `2.0`.

That is real, but it is also small.

### 2. Clean audio got worse as score increased

`clean_prep_opening` degraded steadily:

- `0.443` at control
- `0.474` at `1.0`
- `0.474` at `2.0`
- `0.526` at `3.0`
- `0.598` at `5.0`

The failure mode was not subtle. Higher scores started injecting graph-colored words into otherwise ordinary speech:

- score `3.0`: `Mr. Gran`
- score `5.0`: `Mr. Garan`, `San Mark Casey`, repeated `Mr.`

That is exactly the kind of "safe on some clips, weird on others" behavior that makes a default-on setting dangerous.

### 3. Proper-noun-heavy clips mostly stayed flat

Two of the three proper-noun clips were unchanged across all scores:

- `proper_prep_x1_planning`
- `proper_prep_gordon_andrew`

So the sweep did not show broad proper-noun upside. It showed one small win, not a stable regime.

### 4. Noisy/degraded audio was mixed, not safe

The noisy lane did not show a stable "boost helps degraded audio" story.

- `noisy_team_network_access` got slightly worse at every non-zero score (`0.158` → `0.178`)
- `noisy_team_intro` only improved materially at `5.0` (`0.825` → `0.625`)

That means the only clearly positive noisy result came at the same high score that most aggressively harmed the clean lane.

## Decision

Keep boost opt-in.

Specifically:

- keep `parakeet_boost_limit = 0`
- keep `parakeet_boost_score = 2.0`

Reasoning:

1. There is no score in this sweep that improved graph-known phrases consistently enough to offset the clean-audio regression risk.
2. Scores `1.0` and `2.0` gave only a tiny win on one connector-heavy clip while already making a clean conversational clip worse.
3. Score `5.0` helped one degraded clip, but it also produced the strongest hallucination behavior on a cleaner clip.
4. The current boost phrase selection is itself not safe enough for default-on use because it includes many short common names that can bleed into unrelated speech.

This means the gating problem is not just `boost_score`. It is also phrase selection quality.

## Recommended Follow-Up Before Reconsidering Default-On

If we want to revisit default-on later, the next step should be phrase-set tuning, not score tuning alone.

The current evidence suggests trying one or more of these before another sweep:

- drop single-token common first names from the default boost set
- prefer multi-token entities over generic names
- favor meeting-title entities and rarer proper nouns over people-table frequency alone
- consider a stricter allowlist for graph-derived boosts used by Parakeet defaults

If phrase selection becomes more specific, then rerunning the same score sweep would be worthwhile.

## Final Recommendation

Do not change `crates/core/src/config.rs` defaults in this bead.

The safest honest outcome from the data is:

- keep the feature opt-in
- document the current limits of the regime
- revisit only after phrase selection gets more selective
