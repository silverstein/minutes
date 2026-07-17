# Voice Enrollment — Phase 1 Implementation Spec (ready-to-dispatch)

Status: READY. Companion to `voice-enrollment.md` (approved plan; decisions locked 2026-07-17).
**Sequencing gate:** every work unit that touches `pipeline.rs`, `diarize.rs`, `lib.rs`, `commands.rs`, `index.html`, or `crates/mcp/src/index.ts` is BLOCKED until the conversation-trust integration (minutes:2) merges to `main`. Units touching only `voice.rs`, `config.rs` (new keys), and new files can start earlier (see "Early-start eligible"). Dispatch each unit via `/arbitrage` (codex implements to the acceptance command; Fable specs + reviews).

Grounding (verified against `crates/core/src/voice.rs` @ origin/main):
- `voice_profiles(person_slug PK, name, embedding BLOB, enrolled_at, updated_at, sample_count, source, model_version)` — one **mutable blended vector per slug** (the centroid-only schema the review rejects).
- `save_profile` / `save_profile_blended(conn, slug, name, emb, source, model_version)` — mutate that single vector.
- `match_embedding(emb, &[profiles], threshold) -> Option<String>` — returns a name only. No margin, no model check, no one-to-one, no abstain.
- `save_meeting_embeddings(path, HashMap<String,Vec<f32>>)` — raw-map sidecar.
- `model_version(config) -> &'static str`, `cosine_similarity`, `db_path()`, `open_db_at` exist and are reused.

The centroid schema, raw-map sidecar, and name-only matcher are all **replaced**, not extended, per §12 of the plan. Keep the old functions compiling only where an in-flight caller needs them until that caller is migrated within the same unit.

---

## Work units (dispatch order)

### WU1 — Immutable `voice_samples` schema + derived active profile  ⟨EARLY-START ELIGIBLE — voice.rs only⟩
**Objective.** Replace the single-mutable-vector model with append-only samples + a derived active profile per `(person_slug, model_version)`.
**Files:** `crates/core/src/voice.rs` (+ new `voice_schema` submodule if it keeps the file readable). No cross-lane files.
**Design.**
- New table `voice_samples` (immutable rows, never UPDATEd): `id`, `person_slug`, `name`, `embedding BLOB`, `embedding_dim INT`, `model_id TEXT`, `normalization TEXT`, `trust_class TEXT` (`manual|manually_confirmed|source_candidate|voicematch_candidate`), `meeting_path TEXT NULL`, `sidecar_speaker TEXT NULL`, `capture_source TEXT NULL`, `speech_seconds REAL`, `segment_count INT`, `quality_json TEXT` (SNR/clip/consistency), `similarity REAL NULL`, `top2_margin REAL NULL`, `threshold_version TEXT NULL`, `sensitivity TEXT`, `created_at`, `revoked_at NULL`.
- Derived view/function `active_profile(conn, slug, model_id) -> Option<ActiveProfile>`: aggregate non-revoked samples of the SAME `model_id`+`embedding_dim` only (NEVER blend across models/dims), robust mean with outlier rejection. Cache to a `voice_active_profiles` table keyed `(slug, model_id)`, rebuilt transactionally on sample insert/revoke.
- Migration: keep `voice_profiles` readable; add `migrate_legacy_profiles(conn)` that imports each legacy row as one `manual` sample tagged with its stored `model_version` (dim inferred from blob length). Legacy rows with unknown dim → `model_id="unknown"`, excluded from active-profile derivation (surfaced, not auto-used).
- Transactional inserts (`BEGIN IMMEDIATE`).
**Acceptance:** `cargo test -p minutes-core --no-default-features voice_samples` — new tests: insert→active_profile aggregates same-model only; cross-model sample ignored; revoke removes from active; legacy import produces N manual samples; outlier sample rejected from mean. Plus `cargo clippy --all --no-default-features -- -D warnings`, `cargo fmt --all -- --check`.

### WU2 — Quality-producing `embed_solo_clip`  ⟨EARLY-START ELIGIBLE — voice.rs/diarize.rs embed path⟩
**Objective.** One known-single-speaker clip → one embedding + quality evidence, no clustering.
**Files:** `crates/core/src/voice.rs` (public entry) + a thin call into the existing `diarize` embedding model (reuse `embedding_model_for_config`); guard `#[cfg(feature="diarize")]`. *Coordinate: the embed-model call site borders minutes:2's diarize scope — if trust hasn't merged, keep WU2's new code in voice.rs and call only the stable public embedding fn; do not edit diarize.rs internals.*
**Design.** Guarantee 16 kHz (in-process resample via `crate::resample`), unique temp files, compute several overlapping normalized window embeddings, reject multimodal/inconsistent clips (pairwise cosine below a floor → `Err(LowQuality{reason})`), return `SoloEmbedding{ embedding, dim, model_id, speech_seconds, segment_count, quality: {snr, clipping, window_consistency} }`. Needs only the embedding model, not segmentation.
**Acceptance:** `cargo test -p minutes-core voice_embed_solo` (feature `diarize`) — synthetic single-tone clip yields embedding+quality; concatenated two-source clip is rejected as multimodal; sub-threshold speech duration rejected. (Tests gated on the diarize feature; also add a non-feature compile stub.)

### WU3 — Versioned sidecar envelope  ⟨BLOCKED on trust merge — pipeline.rs/diarize.rs write path⟩
**Objective.** Replace the raw `SPEAKER_X→vector` map with a self-describing envelope; quarantine legacy.
**Files:** `crates/core/src/voice.rs` (envelope type + read/write), `crates/core/src/pipeline.rs` (write call at the diarization step), `crates/core/src/diarize.rs` (supply per-speaker quality). *COLLIDES with minutes:2 — do after merge.*
**Design.** `{schema_version:2, embedding_model_id, embedding_dimension, normalization, meeting_sensitivity, speakers: {label: {embedding, speech_seconds, segment_count, quality, source_stem}}}` written as `.{stem}.embeddings` (JSON). Reader: `schema_version` absent → legacy raw map → surface as `model_id="unknown"`, excluded from automatic enrollment/backfill. Writer stamps `meeting_sensitivity` from frontmatter; **missing/malformed frontmatter → treat as `restricted`** (fail-closed).
**Acceptance:** `cargo test -p minutes-core sidecar_envelope` — round-trip v2; legacy raw map parses as unknown-model/excluded; malformed sensitivity → restricted.

### WU4 — Model-safe matching with evidence + abstain  ⟨BLOCKED on trust merge — voice.rs core, pipeline.rs caller⟩
**Objective.** Replace name-only `match_embedding` with evidence-returning, model-checked, one-to-one, abstaining matcher.
**Files:** `crates/core/src/voice.rs` (new `match_embedding_evidence`), `crates/core/src/pipeline.rs` + `crates/core/src/diarize.rs` (callers of the old fn migrated). Keep old `match_embedding` as a thin shim only until callers move, then delete. *pipeline caller COLLIDES — do after merge.*
**Design.** `MatchEvidence{ winner: Option<slug>, score, runner_up: Option<slug>, margin, model_id, threshold_version, rejection: Option<Reason> }`. Only compare against active profiles of the SAME `model_id`+dim (mismatch → `rejection=ModelMismatch`, never a false match). Enforce **one-to-one** speaker↔profile per meeting (Hungarian/greedy-with-margin), **abstain on ambiguity** (margin below a model-specific floor → no assignment). Per-model calibrated thresholds in config (cam++ ~0.65, cam++-lm separate). Only High-confidence winners rewrite transcript labels (preserves the "wrong name worse than anonymous" rule).
**Acceptance:** `cargo test -p minutes-core match_evidence` — model mismatch abstains; two-close-profiles abstain on low margin; one-to-one prevents double assignment; evidence fields populated.

### WU5 — Granular privacy settings + delete-all-voice-data sweep  ⟨config.rs EARLY-START; cleanup sweep BLOCKED if it touches watch/pipeline⟩
**Objective.** Explicit per-concern toggles + a complete deletion sweep.
**Files:** `crates/core/src/config.rs` (new `[voice]` keys), `crates/core/src/voice.rs` (delete-all), the cleanup path (likely `watch.rs`/a `cleanup` command in `crates/cli`) for sidecar-aware sweeping.
**Design.** Config keys (all default privacy-safe): `store_meeting_embeddings` (bool), `passive_candidate_capture` (default **false**), `candidate_retention_days`, `restricted_meetings_eligible` (default **false**), `retain_non_self_embeddings` (default **false**). `delete_all_voice_data()` transactionally removes profiles + samples + active-profile cache + `.embeddings` sidecars + SQLite WAL/SHM + orphaned sidecars whose meeting is gone. `minutes cleanup` and meeting deletion must also drop the meeting's sidecar (orphan-leak fix).
**Acceptance:** `cargo test -p minutes-core voice_privacy_sweep` — delete-all removes samples+profiles+sidecars+WAL; orphan sidecar swept; defaults assert passive-off/restricted-ineligible.

### WU6 — Desktop active enrollment commands + Settings UI  ⟨BLOCKED on trust merge — commands.rs/index.html⟩
**Objective.** Real enrollment surface (one ~20–30s quality-gated sample; improve-later).
**Files:** `tauri/src-tauri/src/commands.rs` (`cmd_enroll_start/stop`, `cmd_enroll_status`, `cmd_list_voices`, `cmd_rename_voice`, `cmd_delete_voice`, `cmd_test_voice`, `cmd_delete_all_voice_data`), `tauri/src/index.html` (Settings → Voice panel). *COLLIDES heavily with minutes:2 — do after merge; visual-validate in `~/Applications/Minutes Dev.app`.*
**Design.** Enroll: record ~20–30s → `embed_solo_clip` → if quality passes, insert a `manual` sample; else show the rejection reason and re-prompt. Status shows "you're enrolled" (self) distinct from others, sample count, model, last match confidence. `test_voice` turns the CLI diagnostic into a real "record a clip, see match confidence" control. Rename fixes the slug-orphan gap. All strings honest, on DESIGN.md, no new fonts/colors.
**Acceptance:** builds via dev app; 33+ Tauri unit tests green; manual click-test checklist in the PR (enroll → status → test → rename → delete → delete-all). New `cmd_*` unit tests where logic is testable.

### WU7 — Onboarding rework (#492) + G1 diarize-default-on  ⟨BLOCKED on trust merge — index.html/build⟩
**Objective.** Honest first-run; voice recognition default-on with background model download; voiceprint opt-in.
**Files:** `tauri/src/index.html` (onboarding), `tauri/src-tauri` build/features (diarize default-on), setup/model-download flow (`crates/cli` setup + a Tauri first-run trigger).
**Design.** Replace the "10-second test recording" with (a) "Save your first note" showing a real post-Stop confirmation of what was saved, and (b) an **optional, skippable** "Teach Minutes your voice (~30s, stays on this Mac)" step gated on models present/downloading. Diarization + models default-on: kick the ~34MB download in the **background** during first-run, never blocking the first recording; offline → defer + enable from Settings. The persistent **voiceprint is created only on explicit opt-in** (diarization/labels on by default is fine; a stored biometric template is not). Copy states "on-device, deletable, one-way vector, not your speech."
**Acceptance:** dev-app build; onboarding click-test (fresh profile) shows honest copy + skippable enroll + non-blocking download; offline path defers gracefully.

### WU8 — #491 self-anchoring for solo captures  ⟨BLOCKED on trust merge — pipeline.rs⟩
**Objective.** Configured `identity.name` (+aliases) anchors as self even with no calendar attendees; once a self-voiceprint exists, attribute solo recordings by it.
**Files:** `crates/core/src/pipeline.rs` (self-attribution gate), small `config.rs` alias read. *COLLIDES — do after merge.*
**Design.** In `single_stem_speaker_self_attribution`/solo path, if attendees are empty but `identity.name` is set, anchor SPEAKER_0→self at High only when a self voiceprint match confirms it, else keep the configured name as a Medium label (never a wrong High rewrite). Fixes "Mat" rendering as "Matt".
**Acceptance:** `cargo test -p minutes-core self_anchor_solo` — solo capture with configured name + matching voiceprint → self High; no voiceprint → configured name at ≤Medium, never overwritten by a wrong guess.

---

## Not in Phase 1 (Phase 2, gated on the adversarial FAR/FRR eval)
Passive candidate capture → promotion policy; confirmed-label backfill (dry-run first); MCP `enroll_voice` tool; household/multi-user UI; profile merge. Do not build these until the eval gate in §12 of the plan passes.

## Dispatch sequencing summary
1. **Now (pre-merge), early-start:** WU1, WU2, WU5-config — all `voice.rs`/`config.rs`/new-file only, zero collision. These de-risk the schema foundation while minutes:2 finishes.
2. **On trust merge:** WU3 → WU4 (matching depends on envelope+schema) → WU8, then WU6 → WU7 (UI last, visual-validated). WU5-sweep after WU1 lands.
3. Each unit: one `/arbitrage` dispatch, Fable reviews the diff + runs the acceptance command + the pre-commit checklist (manifest/version/test-count sync), then merge (green + ours → merge, don't ask).
