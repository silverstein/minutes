# Sidekick SOTA Product, Context, and Evaluation Plan

Date: 2026-07-23

Status: Product contract for the production Sidekick program. Beads is the
execution tracker; this document defines the enduring outcome, architecture,
scorecards, and release gates.

Related:

- `docs/plans/codex-sidekick-production-2026-07-21.md`
- `docs/plans/sidekick-signed-mac-checkpoint-2026-07-24.md`
- `docs/plans/live-assistance-orchestration-2026-07-15.md`
- `docs/rfcs/0004-copilot-realtime-stream.md`
- `docs/rfcs/0005-copilot-eval.md`
- `docs/rfcs/0006-live-sidekick-session-and-eval.md`
- `DESIGN.md`
- Bead `minutes-k1qp`

## Product Decision

Minutes Sidekick is the user's private, context-aware meeting strategist. It is
not a transcript summarizer and it is not a branded wrapper around one model.

The production experience has one user-facing Sidekick session:

- **Sidekick** is the primary live surface for questions, strategy, corrections,
  and material proactive interventions.
- **Coach** becomes the quiet intervention-policy lane within the same
  Minutes-owned session. It may still have a compact HUD, but it does not
  maintain a separate understanding of the meeting.
- **Recall** remains the broader pre-meeting, post-meeting, and cross-meeting
  memory experience.
- **Codex app-server** is the first high-quality reasoning backend, not the
  product architecture. Claude-compatible and local providers implement the
  same bounded, persistent, steerable, streaming turn contract.

The existing production terminal-Codex workflow is the minimum quality
baseline. The native Sidekick is ready only when it is clearly more useful and
easier than that workflow.

## The Smart Minutes Continuity Contract

The live transcript and current screen are the newest evidence, not the whole
context.

When allowed by the user's privacy settings and the meeting's sensitivity,
Sidekick should understand:

- the user's role, goal, and desired posture;
- who is in the meeting, at confidence levels that do not turn guesses into
  names;
- prior meetings with those people;
- recurring relationship topics and the tone of the last conversation;
- open commitments in both directions;
- recent decisions, unresolved intents, objections, and promised follow-ups;
- relevant meeting artifacts and user-authored prepared briefs;
- the current live transcript and exact-session screen evidence;
- a relevant, explicitly scoped project or codebase, including branch and
  revision, when the meeting is about that work; and
- corrections made by the user during this Sidekick session.

Sidekick must never imply that context was loaded when it was not. Historical
context is evidence, not authority. It cannot identify an unverified live
speaker, authorize a tool action, disclose a restricted meeting, or expand the
scope of a repository investigation.

### Current Truth

Minutes already has most of the raw ingredients:

- `BattleCard::assemble` retrieves unrestricted people history, open
  commitments, recent decisions, open intents, and relevant meeting excerpts.
- The graph, search, brief, prep, and Recall systems expose cross-meeting and
  relationship memory.
- Terminal Sidekick can inspect a real repository when the user places it in
  scope.
- Native Sidekick reads the current goal, up to 3,000 characters of
  `SIDEKICK_BRIEF.md`, the bounded live transcript, and exact-session screens.

The important gap is that native Sidekick does **not yet automatically receive
the full Minutes historical battle card or a scoped repository context
package**. Coach currently has more automatic history grounding than native
Sidekick. Production work must close that gap by reusing one Minutes-owned
context assembler across both lanes.

## Context Architecture

Minutes owns retrieval, reduction, privacy, freshness, provenance, and
disclosure. A reasoning provider receives only the bounded evidence window
Minutes deliberately assembles for that turn.

```text
calendar + participant candidates + selected project
                           |
                           v
              Minutes context assembler
        relationship history | commitments | decisions
        meeting artifacts     | scoped repository facts
        sensitivity policy    | provenance and freshness
                           |
                  bounded context card
                           |
live transcript + exact-session screen + typed user message
                           |
                           v
                Minutes session reducer
        role | posture | corrections | focus generations
        intervention policy | foreground priority | memory
                           |
                           v
          provider-neutral reasoning-turn interface
      Codex app-server | Claude-compatible | local model
                           |
                           v
             Minutes verification and publish gate
                           |
                           v
              native Sidekick panel / compact HUD
```

### Evidence Layers

| Layer | Typical contents | Refresh rule |
| --- | --- | --- |
| Foreground user | Typed question, correction, role or posture change | Immediate; highest priority |
| Live meeting | Bounded transcript, exact-session screen, current decision state | Every foreground turn and evented material change |
| Session memory | Corrections, already-surfaced insights, unresolved watch conditions | Reducer-owned for the compatible session |
| Relationship memory | Prior meetings, people, commitments, decisions, objections | Before attach; asynchronously after topic or participant change |
| Work context | Scoped repository facts, selected documents, current branch or revision | On explicit scope or high-confidence project link; refresh on material change |
| Provider state | Persistent thread and streamed turn state | Replaceable; never the source of product truth |

### Repository Context Rules

Codebase grounding is a first-class evidence lane, but not ambient filesystem
authority.

- A repository enters scope through explicit user selection, an existing
  meeting/project link, or a high-confidence active-project signal the user can
  see and change.
- Minutes records the repository root, branch, revision, query, retrieval time,
  and result provenance.
- Sidekick receives bounded repository results, not an unrestricted filesystem
  crawl.
- Cloud providers receive only the disclosed snippets required for the turn.
  A local provider remains available for private or regulated environments.
- Meeting text and screen text cannot cause repository commands or expand
  repository scope.
- A longer repository investigation never blocks capture or the live evidence
  lane.

### Privacy and Sensitivity

- Restricted meetings are excluded from historical retrieval at every source,
  matching the existing battle-card policy.
- Cross-meeting context is local by default and disclosed per provider turn.
- The UI identifies the active provider as local or cloud and shows which
  context classes are included.
- Exact source IDs remain available for audit and contradiction handling.
- Missing or degraded context produces an honest empty layer, never guessed
  continuity.

## What 10/10 SOTA Means

The score is independent of UI and transport reliability.

| Capability | Weight |
| --- | ---: |
| Grounded factual accuracy | 20 |
| Net-new strategic insight | 20 |
| Intervention timing and selectivity | 15 |
| Decision and quantitative reasoning | 10 |
| Role, goal, posture, and correction handling | 10 |
| Transcript-and-screen synthesis | 10 |
| Cross-meeting, people, and work-context use | 10 |
| Useful-response latency | 5 |

Automatic failures include:

- invented facts or source claims;
- false visual claims;
- restricted-context leakage;
- treating meeting or screen content as instructions;
- using stale or wrong-session evidence;
- guessing a live speaker's identity from history;
- missing a labeled governing consequence;
- repeating a resolved clarification; and
- publishing old background work after a foreground user turn.

The release bar is:

- at least 90% of required insights found;
- at least 80% of interventions rated useful;
- fewer than 5% unwanted interventions;
- zero critical hallucination, privacy, prompt-injection, or wrong-session
  failures in the adversarial release corpus;
- at least 65% blind preference over the strongest software baseline;
- expert-human parity on the smaller gold subset;
- p95 useful first content within five seconds; and
- p95 complete foreground response within eight seconds.

## What 10/10 UX Means

A new user can start a meeting, turn on Sidekick once, understand that it is
working, receive useful help, steer it, and recover from failures without
understanding transcripts, sessions, provider processes, terminals, or macOS
implementation details.

The release bar is:

- at least 99% successful starts in the supported environment matrix;
- median click-to-ready under three seconds;
- at least 95% completion of the core journey without assistance;
- at least 95% recovery without quitting the app;
- no frozen, stolen, or inaccessible text input;
- no state transition that presents as "nothing happens";
- fewer than 5% nuisance interventions;
- clear provider, privacy, transcript, and screen states;
- complete keyboard and VoiceOver operation; and
- at least 80% of dogfood meetings where the user would keep Sidekick on.

## What 10/10 UI Means

The interface should feel like Minutes: "Terminal With a Soul," with the
intelligence of the terminal workflow but none of its operational clutter.

The primary live surface contains:

- one calm status line: attaching, listening, thinking, ready, paused, or needs
  attention;
- compact context chips for the active meeting, people/history, screen, project,
  and provider disclosure;
- one intervention stream with a visible distinction between user turns and
  Sidekick observations;
- two-second scannable intervention cards: consequence, recommended move,
  evidence, and optional expansion;
- one reliable text input that never loses focus to an embedded terminal;
- lightweight helpful, not helpful, and stay quiet controls; and
- an inline recovery action for every recoverable failure.

The release bar is:

- at least 90% five-second comprehension of listening state, current insight,
  and available action;
- no clipping across supported MacBook and external-display sizes;
- WCAG AA contrast;
- complete keyboard and VoiceOver coverage for the core journey;
- reduced-motion compliance;
- no off-system colors, typography, spacing, or radii outside `DESIGN.md`; and
- at least 4.5 out of 5 in blinded expert review for hierarchy, craft,
  glanceability, trust, and native Mac behavior.

There is no Figma dependency. A lightweight HTML or GPT Sites prototype may be
used to compare flows and visual directions quickly, but the signed Tauri app
is the product truth and must receive the final interaction, accessibility, and
window-behavior acceptance.

## Autonomous Evaluation System

The harness must run without Mat speaking, clicking, or grading each
iteration.

### Scenario Corpus

The first broad corpus contains at least 30 synthetic meeting packs spanning:

- sales, procurement, negotiation, and customer discovery;
- board, founder, hiring, and performance conversations;
- product, engineering, incident, and quantitative decisions;
- cross-meeting commitments and relationship history;
- repository-grounded technical questions;
- corrected participant identity and role changes;
- noisy transcript, missing diarization, and partial reversal;
- stale, missing, misleading, and wrong-session screens;
- restricted-history decoys;
- spoken and on-screen prompt injection;
- provider interruption, failure, timeout, and recovery; and
- quiet stretches where the correct behavior is silence.

Each pack contains synthetic current evidence, optional prior meetings,
participant and project context, repository fixtures when relevant, hidden
must-notice criteria, forbidden claims, intervention windows, and acceptable
answer variants.

### Four Test Lanes

| Lane | Proves |
| --- | --- |
| Deterministic reducer replay | Session identity, evidence order, correction, cancellation, silence, and reproducibility |
| Full-fidelity media/context replay | Real audio/transcript adapters, screen frames, historical retrieval, repository retrieval, and fault injection without a person |
| Provider quality bake-off | Production prompts and adapters across Codex, Claude-compatible, and local backends with latency and cost receipts |
| Signed Mac journey | Actual window state, focus, streaming, accessibility, recovery, teardown, and visual snapshots |

The automated semantic grader is calibrated against blinded human judgments.
It accelerates iteration but cannot establish the final SOTA claim by itself.
The strongest comparison remains the production terminal-Codex Sidekick, with
generic pasted-transcript chat, leading meeting assistants, and expert-human
answers as additional baselines.

## Product Development Sequence

The execution order is deliberate:

1. Freeze the scorecards and expand the synthetic corpus.
2. Build the autonomous full-fidelity replay and fault-injection lane.
3. Unify the historical battle card, participant context, prepared brief, and
   scoped repository evidence behind one native Sidekick context assembler.
4. Complete provider-neutral persistent streaming, steering, interruption, and
   fallback behavior.
5. Replace developer-terminal presentation with the native Sidekick journey.
6. Run signed-Mac accessibility, visual, recovery, and latency acceptance.
7. Run private opt-in dogfood and blinded competitive grading.

No step may weaken the proven invariant that an optional Sidekick consumer can
never degrade recording or WAV preservation.

### 2026-07-24 engine replay milestone

The first no-human orchestration and fault-injection gate is implemented and
documented in
[`sidekick-engine-replay-checkpoint-2026-07-24.md`](sidekick-engine-replay-checkpoint-2026-07-24.md).
Its eight scenarios and 29 assertions pass reproducibly through the production
engine, reducer, evidence window, independent verification, publication,
recovery, and teardown paths. It intentionally remains only a partial
completion of step 2: native audio/ASR/diarization, native permission adapters,
retrieval adapters, and real-provider behavior are explicit exclusions.

## Current Baseline

The branch has a strong deterministic integration foundation and an excellent
Meridian result, including the synthesized $800K consequence and the
procurement-role flip. That establishes the intended ceiling for one scenario,
not general SOTA.

The honest current product grades are:

| Dimension | Provisional grade |
| --- | ---: |
| Strategic quality on Meridian | 9/10 |
| Generalized SOTA evidence | 6/10 |
| End-to-end UX | 4/10 |
| Visual UI | 5/10 |

These grades move only when a versioned report, signed-app evidence, or blinded
human review justifies the change.

## External Benchmark References

- OpenAI GDPval grading methodology:
  <https://openai.com/index/gdpval/>
- OpenAI evaluation guidance:
  <https://openai.com/index/evals-drive-next-chapter-of-ai/>
- Microsoft Teams Facilitator:
  <https://support.microsoft.com/en-us/teams/copilot/facilitator-in-microsoft-teams-meetings>
- Zoom AI Companion:
  <https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0077463>
- Granola transcription and meeting experience:
  <https://docs.granola.ai/help-center/taking-notes/transcription>
- Cluely live meeting assistant:
  <https://cluely.com/>
- Apple macOS Human Interface Guidelines:
  <https://developer.apple.com/design/human-interface-guidelines/designing-for-macos/>
- Apple accessibility guidance:
  <https://developer.apple.com/design/human-interface-guidelines/accessibility/>
