# Plan — Consent Layer Phase 2: Sensitive Meetings + Agent-Layer Enforcement

Author: Claude (strategic rescope, 2026-06-10)
Builds on: `consent-layer-spec-2026-06-04.md` (v1, shipped in #276 at `0322c64`),
`consent-layer-addendum-2026-06-04.md` (Tauri settings), `consent-layer-fixes-2026-06-04.md`
(adversarial-review fixes).

## Why now (June 2026 context)

Two curves are moving at different speeds:

- **Adoption**: default-on workplace recording is normalizing (ambient notetakers,
  meeting bots, wearables). a16z's "Everything is Recorded Now" (Torenberg, June 2026)
  is the category's bull case, and it names the gap itself: governance gets
  "retrofitted on top" after the fact.
- **Governance demand**: lags adoption but is steeper when it kicks. EU AI Act
  transparency obligations phase in from August 2026; two-party-consent law isn't
  going anywhere; the first "recorded meeting corpus subpoenaed in discovery" story
  is a when, not an if. Medical/regulated users (RFC 0001's `medical-fr` family,
  HDS/RGPD context) already ask for this.

Nearly every product in the category is built for the first curve and will retrofit
the second. Minutes builds the second curve into the architecture. That is a moat
timed to a predictable shock, and it is also simply the right way to build it.

## The two objections the bull case can't answer (our wedge)

1. **Discovery/subpoena risk.** The only coherent answers are local-first custody,
   retention policy, and selective capture. Cloud-processed competitors structurally
   cannot offer the first; we can offer all three.
2. **The non-consenting participant.** Everyone in a recorded meeting enters the
   corpus, consenting or not. Minutes' graph models people as entities, so per-person
   exclusion/redaction is architecturally possible here in a way it is not elsewhere.
   (Explicitly v3 / out of scope below — on the map, not in this phase.)

## The core reframe

In an agent-first product, the transcript's primary consumer is not a human reading
markdown. It is the agent layer: MCP search, the knowledge graph, ingest, skills.
Therefore consent/sensitivity metadata is not documentation — it is an
**enforcement contract for agents**. A meeting designated sensitive must be
invisible-by-default to every agent surface, not just labeled in YAML. This turns
the consent layer into the policy gate of the audio→agent bridge, which is the
product's positioning expressed as architecture.

## Scope — two waves

### Wave 1 (target 0.18.8) — the designation

1. **Desktop Require modal.** Close the gap behind the v1 label
   ("Require confirmation (CLI blocks; app reminds for now)"). A real blocking
   confirmation before desktop recording starts; Cancel cancels. TODO marker:
   `tauri/src-tauri/src/commands.rs` (`TODO(phase 2)` in
   `maybe_save_and_show_recording_consent`). Small, UI render verification via
   dev app required.
2. **Sensitive Meeting designation + no-capture mode.** A first-class meeting
   artifact with NO audio capture:
   - Designate ahead (calendar match or manual) or at start ("record nothing").
   - **Quick typed markers** during: timestamped, no recorder running. Markers MUST
     ride the event bus (RFC #194 event types, append-only discipline) rather than a
     parallel notes path. This makes sensitive mode the first real consumer of the
     mid-meeting typed-semantic-events surface identified as empty competitive real
     estate (April 2026 snapshot) — territory cloud notetakers without webhooks
     cannot follow into.
   - **Guided debrief** after: structured human-written summary into standard
     frontmatter, provenance marked no-capture (e.g. `capture: none`,
     `sensitivity:` field).
   - Output is a normal meeting file for the human; sensitivity governs the agent
     layer (Wave 2).

### Wave 2 (target 0.18.9) — the enforcement

3. **Retention policy.** Per-sensitivity auto-delete: audio after N days, optionally
   transcript too. Prerequisite: audit current audio retention behavior in code
   (what is kept where, after which pipeline) before speccing the delta. This is the
   concrete thing legal/medical users request.
4. **Agent-layer enforcement.** Sensitivity respected by MCP tools, graph fact
   extraction, search, and ingest: restricted meetings excluded by default from all
   agent surfaces, with explicit, logged override. The genuinely novel piece; no
   competitor has an equivalent because none of them separate the artifact from the
   agent surface.

## Hard constraints (carry over from v1, plus new)

- **Copy discipline (non-negotiable):** no string anywhere may say "legal",
  "compliant/compliance", "lawful", or "no consent required". Allowed framing:
  "audio stays on your device", "disclosure aid, not legal advice",
  "ensure everyone present consents where required", "provenance you can show".
- **Never block non-interactive callers.** Headless/hook/automation paths degrade
  gracefully exactly as v1's `require` does (warn + unattested), including the new
  modal path.
- **Agents never mutate human frontmatter** (RFC #194). Markers and debrief
  annotations are append-only attributed events.
- **No enterprise land-grab.** Retention + agent-gating are single-user, local-first
  features valuable to one operator today. Pure-OSS direction (April 2026 decision)
  stands; this phase is product depth + positioning, not a pivot.
- 100% doc comments on new pub items; fmt + clippy clean; tests per surface
  (CLI gate, pipeline stamping, MCP exclusion, Tauri modal via dev-app click-test).

## Explicitly OUT of scope (v3+, do not build)

- Per-person redaction / "right to be forgotten" per attendee (on the map; needs
  graph-wide redaction design).
- Org policy distribution, admin consoles, anything multi-tenant.
- Video/screen capture governance.
- Telemetry, hosted anything.

## Process

Same pipeline that shipped v1: this plan → detailed build spec per wave → Codex
implements → two-pass adversarial review (Codex + fresh-Claude) → dev-app click-test
for any UI → merge. Release notes for 0.18.7 frame v1 as "governance built in, not
retrofitted"; the Wave 2 release carries the "controls your agents must obey" story.
