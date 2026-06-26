# Plan — Person Entity Resolution (canonicalizing people across fragments)

Author: Claude (research + strategy session 2026-06-26)
Status: proposed; research-grounded; entity-level sibling to "Every Name, Right"

## Why this, why now

A recurring contributor (Geert Theys, @gtheys) reported one real person showing
up as six separate knowledge-graph entities:

```
jun-rei   junlei   junlei-tech-lead   junrei   junrei-core-team   junwei
```

Two failures are stacked:

1. **Spelling drift.** The transcriber spells the same name several ways
   (`junlei` / `junrei` / `junwei` / `jun-rei`). The graph keys people by an
   exact slug, so each spelling becomes its own person.
2. **Role/title contamination.** `junlei-tech-lead` and `junrei-core-team` fuse
   a descriptor into the name during entity extraction (filed as #370).

This is the **entity-level twin** of the "Every Name, Right" epic
(`docs/plans/every-name-right-2026-06-11.md`). That epic keeps the transcript
*token* right; this one keeps the *person* right across fragments. They share
one philosophy: a wrong rewrite is worse than no rewrite. Here that becomes
**a wrong merge (collapsing two distinct real people) is worse than a split.**

It is foundational, not cosmetic: the knowledge graph canonicalizes entities by
name. Fragmented people poison commitments, decisions, and cross-meeting
queries, and the agent layer is only as good as its names. It is June 2026, we
ran a SOTA pass (below), and the verified primitives plus our existing rule core
mean this is mostly composition plus an eval plus one focused new edge
predicate, not a ground-up rebuild.

## What already exists (build on, do not rebuild)

- `crates/core/src/name_correction.rs` — token-level post-pass corrector.
  Two tiers (accent restoration + bounded edit distance with a corroborating
  signal: same first letter OR matching Double Metaphone via `rphonetic`), with
  a name-position tier that relaxes only for confirmed meeting participants.
  Off by default, provenance-preserving (raw token kept), never a silent
  rewrite.
- `crates/core/src/name_eval.rs` — `HarnessReport`
  (`recovered` / `missed` / `false_corrections` / `structural_mismatches`) over
  a 12-case synthetic corpus (6 corrections + 6 negatives). **Token-level, not
  cluster-level.** `false_corrections` must be zero: the gating invariant.
- `crates/core/src/pipeline.rs` — where a person entity's name and slug are
  actually built (`slugify`, plus the `strip_email_domain` /
  `strip_name_disambiguation` helpers, and `fold_user_identity`, which already
  folds aliases/emails onto one canonical entity). This is the layer to extend
  for role stripping, NOT `knowledge_extract.rs` (which only consumes
  already-built entities).
- `crates/core/src/graph.rs` — `push_file_person` dedups by exact slug;
  `detect_aliases` runs an O(n^2) pairwise `names_likely_same` check and returns
  `AliasSuggestion { name_a, name_b, shared_meetings }` (suggestion only, no
  merge action). Note: `names_likely_same` **requires identical first-name
  tokens**, so it does not match drift variants like `junlei`/`junrei`/`junwei`.
  `merge_person_aliases` is an alias-*list* dedup helper for one slug, not a
  person-to-person merge (no person merge exists today). `person_role_priority`.
- `crates/core/src/person_identity.rs` — `PersonCanonicalizer` (alias-table
  resolution of raw mentions to identities); vocabulary person entries are
  loaded into it on graph rebuild.
- `crates/core/src/overlays.rs` — `~/.minutes/overlays.db`, additive
  human-confirmed state layered over immutable meeting markdown and re-applied
  on every graph rebuild. The existing home for confirmations that must survive
  the `graph.db` wipe.
- `crates/core/src/vocabulary.rs` — local vocabulary store + CLI
  (`minutes vocabulary add --kind person <canonical> --alias ...`); feeds both
  the decode hints and the name-correction pool.
- Speaker attribution L0-L3 with confidence gating; voice profiles in
  `voices.db` (survives graph rebuild; `graph.db` does not).

## The gap the junrei case exposes

- The graph keys people by exact slug, so fragments survive as distinct people.
- `detect_aliases` flags some pairs, but it (a) is unmeasured, (b) has no
  confirm-merge action, (c) does not run as a transitive clustering pass, and
  (d) cannot even propose the motivating drift fragments, because
  `names_likely_same` requires identical first-name tokens.
- Role/title contamination is an entity-construction bug (#370), distinct from
  drift.
- Nothing scores cluster quality, and nothing measures the cardinal
  wrong-merge error.

## SOTA pass (June 2026) — verified vs leads

A deep-research fan-out was run; its verification phase was throttled by
transient API rate limits (1 of 20 claims cleared the 3-vote bar), so the four
items below were confirmed by **direct source fetch**, and everything under
"leads" is explicitly unverified.

**Verified (direct fetch):**

- **Sortformer "Sort Loss"** (NVIDIA, arXiv 2409.06656, ICML 2025; 3/3
  adversarial). Orders speakers by arrival time so speaker-attributed ASR trains
  with plain cross-entropy instead of permutation-invariant training. The
  backbone primitive for joint diarization + naming.
- **Streaming Sortformer** (arXiv 2507.18446, Interspeech 2025). Online,
  real-time diarization via an Arrival-Order Speaker Cache with dynamic speaker
  count (not the 4-speaker cap of the offline v3). The live-attribution path.
- **`speakrs`** (github.com/avencera/speakrs). Pure-Rust pyannote `community-1`
  pipeline, no Python in the library path, ONNX Runtime / CoreML. Reports
  7.1% DER at 529x realtime on M4 Pro vs pyannote 7.2% at 24x. A candidate
  upgrade to our `pyannote-rs` diarization.
- **cpWER / tcpWER** (CHiME-7/8 DASR review, arXiv 2507.18161). The standard
  identity-attributed transcription metric for meeting audio.

**Leads (unverified; benchmark before adopting):**

- **Symphonym** (arXiv 2601.06932, Jan 2026 preprint). Learned ~128-dim
  phonetic name embeddings, IPA-free distilled student (ONNX/Rust-plausible),
  aimed at non-standard spelling variation. Reported figures are on cross-script
  *place* names, not same-script personal-name ASR drift; transfer is asserted,
  not shown. Candidate to bench head-to-head against our Double Metaphone +
  edit distance.
- **LLM-as-judge confirmation gating** (three-tier: auto-accept high cosine,
  auto-reject low, judge the middle band). Sensible shape; the headline
  precision figure from the source was actively refuted, so do not cite it. For
  a privacy-first tool the judge must be local (Ollama).
- SBERT semantic blocking; character-identification coreference on dialogue
  transcripts (ACL K17-1023). Background, not load-bearing.

## Design principles (carried from name correction)

1. **A wrong merge is worse than a split.** Precision-favoring,
   confirmation-gated, provenance-preserving, suggestion-not-silent.
2. **Local-first.** Any LLM judge defaults to Ollama; no PII leaves the device.
3. **Measure first.** No lever ships without moving a number on the eval.

## Scope (slice order)

### Slice 0 — Entity-resolution eval harness (measure first)

Extend `name_eval` into a clustering eval. Synthetic fixture: sets of name
fragments with ground-truth person clusters, covering (a) spelling drift,
(b) role/title contamination, and critically (c) **true-distinct-people
negatives** (two real people with similar names that must NOT merge). Metrics:
B-cubed precision/recall/F, CEAF, and V-measure for cluster quality, plus an
**asymmetric cost** that weights wrong merges far above misses. Single gating
number, mirroring `false_corrections = 0`: **wrong merges must be zero at
default thresholds.** Add cpWER for the attribution layer once audio fixtures
exist. Public-repo discipline: synthetic names only.

### Slice 1 — Role/title suffix stripping (#370)

At person-entity construction in `pipeline.rs` (alongside the existing
`strip_email_domain` / `strip_name_disambiguation` helpers, before `slugify`),
strip role/title descriptors so "Junrei the tech lead" resolves to person
`Junrei` with the role as a separate attribute, not the slug `junrei-tech-lead`.
(`knowledge_extract.rs` only consumes already-built entities, so the fix does
not belong there.) The slug is also re-derived during graph rebuild, so confirm
the stripped form is what reaches both the frontmatter entities and the graph.
Regression cases land in the Slice 0 eval. Independent of the rest; shippable
immediately.

### Slice 2 — alias clustering pass + confirm-merge

Two parts, and the first is genuinely new work, not just promotion. The existing
`names_likely_same` edge predicate requires identical first-name tokens, so it
cannot link drift variants (`junlei`/`junrei`/`junwei`). Add a new edge
predicate that reuses the phonetic + bounded-edit matchers that today live in
`name_correction.rs` (Double Metaphone via `rphonetic` + Levenshtein), shared
into the graph layer and gated by `shared_meetings`. Then run transitive
clustering (union-find) over those edges. Add a user-facing confirm-merge
(`minutes graph merge` CLI and/or an MCP tool); default stays suggestion-only,
auto-merge only at extreme confidence and always with provenance.

Persistence: a confirmed merge must survive the `graph.db` wipe. Two existing
homes, pick one deliberately: `overlays.rs` (`~/.minutes/overlays.db`, the
purpose-built additive human-confirmed-state layer re-applied on rebuild), or
the vocabulary store (already loaded into `PersonCanonicalizer` on rebuild).
Vocabulary has a catch: `validate_alias_conflicts` fails closed if the alias
already belongs to a different canonical, so a confirm-merge write can be
rejected and needs a conflict path. Overlays is likely the cleaner fit for
merge confirmations.

### Slice 3 — Embedding clustering (bench-gated)

Add an embedding-similarity stage for drift cases phonetics miss. Bench a
Symphonym-style character embedder (ONNX) against the current Double Metaphone +
Jaro-Winkler on the Slice 0 eval. Adopt only if it raises `recovered` with zero
new wrong merges.

### Slice 4 — Local-LLM confirmation-gated merge judge (optional)

For the mid-similarity band rules and embeddings cannot settle, route to a local
Ollama judge that must cite discriminative evidence to merge; default
auto-reject. Determinism: low temperature plus decision caching, and the eval
rejects nondeterministic flip-flops outright.

### Adjacent track — diarization / attribution (separate, non-blocking)

Evaluate `speakrs` as a faster pure-Rust alternative to `pyannote-rs`, and
streaming Sortformer for live attribution. Subprocess-isolate per the known
pyannote/parakeet `ort` + `ndarray` in-process conflict (pyannote-rs#27). These
improve the upstream speaker-to-name binding that feeds the graph, but are an
engine track, not part of the canonicalization epic.

## Sequencing and dependencies

- Slice 0 first; it gates 1, 3, 4.
- Slice 1 (#370) is independent and can ship in parallel.
- Slice 2 builds on existing `detect_aliases`; high value, low risk.
- Slices 3-4 are bench-gated by Slice 0.
- The Adjacent track runs separately and blocks nothing here.

## Risks

- **Wrong-merge regressions** — mitigated by the asymmetric eval and
  confirm-gating; the gating number is wrong-merges = 0.
- **Persistence** — confirmed merges must survive `graph.db` rebuild, so they
  write to vocabulary/canonicalizer, never only to `graph.db`.
- **Scope creep** into a full diarization rewrite — kept in the Adjacent track.
- **Symphonym transfer risk** (place names -> personal names) — bench before
  adopt; rules stay the default until beaten.

## Open questions

- Can streaming Sortformer run in-process alongside `pyannote-rs` given the
  `ort` conflict, or must it be subprocess-isolated?
- What confirmation threshold and routing lets a local Ollama judge hit the
  precision target while staying deterministic enough for a reproducible eval?
- Is there any 2025-2026 benchmark scoring person-name canonicalization with
  asymmetric wrong-merge penalties, or do we build our own labeled corpus
  (likely the latter)?

## Appendix: sources

- arXiv 2409.06656 — Sortformer (verified).
- arXiv 2507.18446 — Streaming Sortformer (verified).
- arXiv 2507.18161 — CHiME-7/8 DASR review, cpWER/tcpWER (verified).
- github.com/avencera/speakrs — Rust diarization (verified).
- arXiv 2601.06932 — Symphonym phonetic embeddings (lead, unverified).
- ACL K17-1023 — character identification / coreference on dialogue (background).
- Practitioner blogs on LLM-as-judge ER and semantic entity resolution were read
  for design patterns only; their headline figures are unverified and one was
  refuted, so none are cited as fact.
