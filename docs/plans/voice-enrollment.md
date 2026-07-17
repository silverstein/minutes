# Plan: First-Class Voice Enrollment for Minutes

Status: DRAFT for review (Fable-authored; codex adversarial review pending)
Date: 2026-07-17
Related issues: #490 (Recall copy), #491 (configured name ignored on solo captures), #492 (onboarding "test recording" isn't enrollment + no desktop enroll path)

## 1. Thesis

Minutes' promise is "your AI remembers every conversation *you* have." That only lands if Minutes knows which voice is **you** — and, over time, who your regulars are. The engine for this already exists (embeddings, `voices.db`, blended profiles, L2/L3 attribution); there is simply **no product around it** (no desktop UI, no MCP tool, no passive learning, no backfill). This plan makes voice identity first-class.

The SOTA UX insight (from Otter/Fireflies + speaker-verification research): **don't make users sit through a voice-training wizard. Confirm once, learn passively.** So the design is three legs, not one screen:

1. **Active enrollment** — a deliberate, SOTA-sized "teach Minutes your voice" flow in onboarding *and* Settings.
2. **Passive enrollment** — every time you confirm a speaker ("that's Mat", "that's Sarah"), Minutes blends that person's voiceprint from the *real meeting audio*. This is the magic and the differentiator.
3. **On-device, private by construction** — the voiceprint never leaves the machine (`~/.minutes/voices.db`, 0600, separate from `graph.db`). Cloud tools can't say that.

Passive enrollment (#2) also **solves existing-user backfill for free**: no migration ceremony — retro-build voiceprints from the `.embeddings` sidecars every past meeting already has, gated on user confirmation.

## 2. What already exists (build minimally on this)

- **Storage/blending (solid, reuse as-is):** `voice.rs` — `voices.db` (SQLite, 0600, survives graph rebuilds), `voice_profiles{person_slug, name, embedding(512-d cam++ f32), sample_count, source, model_version}`. `save_profile_blended` = running weighted average (**the passive-learning primitive**). `match_embedding` = cosine vs `config.voice.match_threshold` (default 0.65). `load_self_profile` = profile whose slug == `slugify(identity.name)`.
- **Backfill source (gift):** per-meeting `~/meetings/.{stem}.embeddings` sidecars map `SPEAKER_X → vector` for every processed diarized meeting. Existing users can be enrolled from history with **no re-recording**.
- **Attribution already consumes profiles:** L2 `match_speakers_by_voice` (High/Enrollment), self-anchoring `single_stem_speaker_self_attribution`, L3 `confirm --save-voice` (blended "confirmed"). MCP `confirm_speaker{save_voice}` proves "enroll from real meeting audio" end-to-end. Only **High**-confidence rewrites transcript labels (the "wrong names are worse than anonymous" rule stays intact).

## 3. Two gating issues to resolve up front

- **G1 — the `diarize` feature + 34MB models.** Embedding extraction is `#[cfg(feature="diarize")]` and needs `minutes setup --diarization` models. No feature/models → no enrollment or voice matching at all. **First-class voice ID therefore requires diarization in the default desktop build + a model-download step in onboarding.** Decision needed (see Open Questions): ship `diarize` on by default in the app bundle and make the ~34MB download a first-run step gated behind "turn on voice recognition."
- **G2 — model-version-blind matching (latent bug).** Profiles store `model_version` but `match_embedding` ignores it. Switching `embedding_model` (cam++ ↔ cam++-lm, different vector spaces) silently produces garbage matches. The feature must (a) tag every profile with its model, (b) refuse/skip cross-model matches, and (c) offer re-embed on model switch. Fix this as part of the work regardless of scope.

## 4. Primitives to build (the minimal new surface area)

1. **`embed_solo_clip(wav, config) -> Vec<f32>`** in `voice.rs`/`diarize.rs` — embed one known-single-speaker clip as one speaker, skipping clustering (today enrollment abuses the full `diarize()` and cherry-picks the first speaker). Needed by active enrollment, "test my voice", and backfill.
2. **Tauri commands** (none exist today): `cmd_enroll_start/stop` (record N seconds → embed → blend), `cmd_enroll_status` (are you enrolled? sample count, last match, model version), `cmd_enroll_from_history` (backfill), `cmd_list/rename/delete_voice`, `cmd_test_voice` (record a clip, show match confidence — turns the dev diagnostic at cli:5518 into a real feature).
3. **MCP enroll tool** — an `enroll_voice`/`update_voice` tool so agent-driven setup works (currently MCP is read/confirm only).
4. **Multi-sample guided capture** — active enrollment records ~2–3 short prompts (research: ~20s effective speech minimum; 3× ~10s best practice) and blends, with a cross-sample consistency gate (reject a sample whose cosine vs the others is an outlier → catches "someone else spoke" / bad mic).
5. **Passive-enroll hook** — on any High-confidence speaker confirmation (desktop, CLI, MCP), blend the confirmed speaker's meeting embedding into their profile, gated by a `voice.passive_enrollment` toggle (default: confirm-gated on).
6. **Model-version-aware matching + migration** (G2).
7. **Explicit "self"** — a `is_self` flag (or reuse identity-name-slug but surface it) so the app can say "you're enrolled" distinctly from "3 people enrolled".

## 5. Where it lives in the UX

- **Onboarding (rework #492):** replace the confusing "10-second test recording" with an honest two-part first-run: (a) "Save your first note" (the pipeline test, with a real post-Stop confirmation of what was saved), and (b) an **optional** "Teach Minutes your voice (~30s, stays on this Mac)" step that does real enrollment — only if diarization models are present/being downloaded. Skippable, resumable later from Settings.
- **Settings → People/Voice (new):** self-enrollment status + re-record; a list of enrolled people (rename/delete/merge — fixes the name-change-orphans-profile gap); the passive-enrollment toggle; "test my voice"; "enroll me from my history" (backfill); model/threshold info.
- **In-transcript / Recall:** when a speaker is confirmed, offer "remember this voice" inline (this is the passive-enrollment entry point in daily use, mirroring Otter's tag-once).
- **Fixes #491 in passing:** anchor the configured `identity.name` (+aliases) as self even for solo captures with no calendar attendees, and once a self-voiceprint exists, use it to attribute solo recordings — so "Mat" stops rendering as "Matt".

## 6. Existing-user migration

No forced re-onboarding. On update, a one-time gentle nudge: "Minutes can now recognize you and your regulars — all on this Mac. Set it up?" → offers (a) 30s active self-enrollment, and/or (b) **backfill**: scan the last N meetings' `.embeddings` sidecars, cluster to find the recurring/most-likely-self speaker, show the user candidate meetings to confirm ("is this you?"), and blend confirmed embeddings into the self profile. Users with diarization already have the raw material; users without get prompted to download the models once.

## 7. Privacy posture (first-class message)

100% on-device. The voiceprint is a math vector in a local 0600 SQLite file, never transmitted, never used for cloud calls, and deletable in one click. This is a headline trust differentiator vs cloud meeting tools and should be stated plainly in the enrollment UI and the site.

### 7a. Audio retention vs voice learning (the key privacy interaction — verified)

Minutes does NOT destroy audio on transcription; retention is a configurable window (`RetentionConfig` defaults: `successful_audio_days=30`, `restricted_audio_days=7`, `failed_audio_days=90`) and deletion only runs on `minutes cleanup` (`auto_cleanup` default false). **Voice learning is decoupled from audio retention**: it operates on the per-meeting `.embeddings` sidecar (512-d identity vectors, written at diarization time — pipeline.rs:2205/2903), and retention's `is_audio_path` (retention.rs:239) matches only `wav|m4a|mp3|ogg|webm|mov`, so `.embeddings` is **excluded from cleanup and survives audio deletion**. So continual learning works even when the user destroys recordings — Minutes learns from the vector, then the audio can go. This is a feature, not a conflict.

Design consequences:
- **Two separate, intuitive knobs**: *audio retention* and *voiceprint retention* are independent. Deleting recordings must not silently wipe learned voiceprints; "forget my voice" (delete voiceprints) is a distinct one-click action. Surface both plainly.
- **Honest framing**: a voiceprint is biometric even though it is not reconstructable audio. Say so; don't over-claim "we keep nothing." The claim is "we keep a one-way identity vector on your Mac, not your speech."
- **Restricted meetings excluded from passive enrollment by default.** A restricted meeting's `.embeddings` sidecar is a biometric derivative of restricted content; per the conversation-trust derived-artifact doctrine it inherits restricted sensitivity. Passive enrollment and backfill must skip restricted meetings unless the user explicitly opts a given profile in. (Also revisit whether restricted `.embeddings` sidecars should themselves be retention-bound rather than kept forever.)
- **Sidecar lifecycle question** (for codex/impl): today `.embeddings` sidecars are kept forever (excluded from cleanup). Once folded into a profile they may no longer be needed except as a backfill/audit source — decide whether they get their own retention window or a "used for enrollment" marker.

## 8. Failure modes / risks (for codex to attack)

- **Passive-enrollment poisoning:** a misattributed speaker blends the *wrong* voice into a profile, silently degrading future matches. Mitigations to pressure-test: only blend on **High**-confidence/explicit confirmation; cross-sample consistency gate; cap blend weight; keep a small ring of recent contributing embeddings so a bad blend is reversible; never auto-blend below threshold.
- **False accept / cross-person collision** (esp. similar voices, family members): threshold tuning, per-profile margin (best vs second-best), and "unsure → stay anonymous" beats a wrong name.
- **Multi-speaker household / same machine:** N named profiles already supported; needs UI + a "who are you enrolling" step so one person's session doesn't overwrite another's self.
- **Model switch / migration (G2):** stale-vector-space matches.
- **Name change orphaning the slug-keyed profile.**
- **Short/low-quality enrollment audio** producing a weak voiceprint that mis-hits.
- **Backfill mis-clustering** self vs a frequent counterpart.

## 9. Phasing (target: next release + fast-follow)

- **Phase 1 (next release, MVP):** `embed_solo_clip` primitive; G2 model-version-safe matching; desktop active enrollment (multi-sample) + status + delete/rename in Settings; onboarding rework (#492) with honest copy + optional enrollment; fix #491 self-anchoring for solo captures; #490 copy fix rides along (trivial). Privacy messaging.
- **Phase 2 (fast-follow):** passive/confirm-once enrollment hook + toggle; existing-user backfill from sidecars; "test my voice"; MCP enroll tool; household/multi-user UI; merge duplicate profiles.

## 10. Open questions for Mat

1. **G1 build decision:** ship `diarize` on by default in the desktop bundle (accepting the ~34MB model download as a first-run step), or keep it opt-in and make voice ID a "download to enable" feature? First-class strongly implies default-on.
2. **Passive-enrollment default aggressiveness:** confirm-gated-on (safe, Otter-like — my recommendation) vs auto-learn-silently (Fireflies-like, riskier). 
3. **Active enrollment ask:** how much friction is acceptable — one 30s free-speech sample (lowest friction) vs 3× short prompts (more robust)? Research favors the latter; onboarding conversion favors the former. Recommendation: one ~20-30s sample at onboarding, offer "improve it" (more samples) later.
4. Scope for next release: Phase 1 only, or pull passive-enroll (Phase 2) forward since the blend primitive already exists?

## 12. Codex adversarial review — architecture revisions (FOLDED IN, supersedes where noted)

Codex (gpt-5.6, read-only, against origin/main) largely validated the product thesis but found the *safety architecture* of the draft too thin for a biometric feature. Key revisions, now authoritative:

- **Passive learning is CANDIDATE CAPTURE, not auto-enroll (P0, supersedes §4.5/§8).** Passively-captured embeddings are quarantined candidates that **never affect matching** until an explicit, multi-condition **promotion**. Passive capture defaults OFF unless the user has explicitly enabled biometric-template retention. Initially, only manual enrollment and explicit `confirm --save-voice` create trusted samples. This kills the poisoning class at the root rather than mitigating it after the fact.
- **Replace the centroid-only schema (P0, supersedes §2's "reuse as-is").** The current single mutable blended vector per slug loses provenance and is irreversible. Move to immutable `voice_samples` rows: person slug, exact model id+dimension, normalized embedding, **trust class** (manual / manually-confirmed / source-backed-candidate / voice-match-candidate), meeting+sidecar provenance + speaker label, capture source/device, speech duration + segment count + SNR/clipping/consistency quality, similarity + top-two margin + threshold version, sensitivity, created/revoked time. Derive one **active profile per (slug, model_version)**; NEVER blend across models/dimensions; transactional updates. (This also fully fixes G2.)
- **Versioned sidecar envelope (P0).** Replace the raw `SPEAKER_X→vector` map with `{schema_version, embedding_model_id, embedding_dimension, normalization, meeting_sensitivity, speakers[label]{embedding, speech_seconds, segment_count, quality, source_stem}}`. Legacy raw-map sidecars are treated as **model=unknown and excluded from automatic enrollment** (they can still be promoted with explicit user action).
- **`embed_solo_clip` must be quality-producing (P0, sharpens §4.1).** Guarantee 16 kHz (in-process resample), unique temp files, compute several normalized window embeddings, **reject multimodal/inconsistent clips**, return embedding + quality evidence. Requires only the embedding model, not the segmentation model — so it's lighter than a full diarize run.
- **Explicit granular privacy settings (P0, sharpens §7a).** Separate toggles: store per-meeting embeddings; passive candidate collection; candidate retention days; restricted-meeting eligibility (**default false**); non-self embedding retention (**default false or separately disclosed**). `minutes cleanup`, meeting delete, archive, profile delete, and "delete all voice data" must consistently sweep profiles + samples + sidecars + SQLite WAL + orphans. **Missing/malformed markdown → treat as restricted during backfill** (fail-closed, consistent with the trust doctrine). Orphan risk: meeting deletion that leaves an unclassified sidecar behind is a leak — retention/delete must reason about sidecars too.
- **Promotion policy replaces the 4 poisoning gates (P1, supersedes §8 mitigations).** Promoting a candidate into a *trusted* profile requires ALL of: same exact model; valid normalized dimension; source-isolated **local** stem; adequate clean speech across multiple windows; high similarity under a **model-specific calibrated** threshold; sufficient margin from **all other** enrolled profiles; agreement across **multiple independent meetings**; bounded **total** passive weight vs trusted samples; robust outlier rejection; no restricted meeting and no degraded capture. A new *self* profile is NEVER bootstrapped passively — self must be manually established first.
- **Backfill redesign (P1, supersedes §6's clustering).** Do NOT infer self from "largest cluster." Anchor on manually-confirmed self overlays/labels; prefer source-isolated local stems when audio still exists; exclude source-aware sidecars that contain only the *remote* speaker; exclude unknown-model legacy from automatic use; produce a **dry-run report** of eligible/rejected candidates with reasons; require explicit promotion for ambiguous legacy data.
- **Fix matching before adding adaptation (P1).** `match_embedding` must return structured evidence (winner, score, runner-up, margin, model, threshold version, rejection reason), enforce **one-to-one** speaker↔profile assignment per meeting, and **abstain on ambiguity**. Calibrate CAM++ and CAM++-LM thresholds independently (their cosine scales differ — config already hints ~0.65 vs ~0.1–0.2).
- **Adversarial evaluation is a release gate (P1).** Before passive promotion ships, measure FAR/FRR across: both embedding models; multiple mics/rooms; open-air vs headphone calls; crosstalk/overlap; recurring colleagues; TV/podcast/replay/synthetic voice; short/quiet/clipped audio; model switches; repeated poisoning attempts; parallel processing + crash recovery. Release criterion includes **cumulative-drift** simulations, not just single-sample similarity.
- **CLI/Tauri/MCP parity (P2).** Candidate listing, provenance, promote/reject/revoke, model-incompatibility surfacing, and delete-all-voice-data must exist across all three surfaces; MCP must capability-check the installed CLI. A revocation history without controls isn't a real privacy mechanism.

### Re-phasing after the review
- **Phase 1 (next release) = the SAFE FOUNDATION + active enrollment only.** `voice_samples` schema + versioned sidecar + model-version-safe matching-with-evidence + quality-producing `embed_solo_clip` + desktop active enrollment (multi-sample, quality-gated) + status/rename/delete + granular privacy settings + delete-all-voice-data + onboarding rework (#492) + #491 self-anchoring + #490 copy fix. **No passive promotion, no auto-backfill yet.**
- **Phase 2 (fast-follow, gated on the adversarial eval) = candidate capture → promotion + confirmed-label backfill (dry-run first) + MCP enroll + household/multi-user.** Passive promotion does not ship until the FAR/FRR gates pass.

This is bigger and safer than the original single-release MVP, and correctly so — a biometric identity feature that mis-learns silently is worse than none. The blend-primitive-already-exists optimism in the draft was right about the *storage layer* but wrong to imply passive learning is a small addition; the safety architecture is the real work.

## 11. Coordination

Voice enrollment touches `voice.rs`, `diarize.rs`, `pipeline.rs` (attribution/self-anchoring), `config.rs`, `tauri/src-tauri/src/commands.rs`, `tauri/src/index.html`, `crates/mcp/src/index.ts`. The in-flight **conversation-trust** work (minutes:2) heavily edits `commands.rs`/`index.html`/`index.ts`/`pipeline.rs`. Sequence voice-enrollment implementation to land **after** conversation-trust merges, or scope Phase 1's core to `voice.rs`/`diarize.rs`/new CLI first (no overlap) and add the desktop/MCP surface once trust is in.
