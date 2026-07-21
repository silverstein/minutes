# Codex Sidekick: demo cut and production architecture

Date: 2026-07-21

Status: demo preview implemented on `feat/codex-sidekick-demo`; production
orchestration remains proposed.

## Decision

Minutes should stop treating a stateless nudge generator as the ceiling for
live assistance. The high-quality experience already exists in the embedded
terminal: a persistent Codex agent can read the bounded live transcript,
retrieve exact-session screenshots, investigate real repositories, remember
the user's corrections, and accept interactive steering.

The demo cut packages that proven path as **Codex Sidekick**. The production
cut should replace the terminal wrapper with a Minutes-owned integration over
the Codex app-server protocol. Coach can remain a small glanceable HUD, but it
should consume the same session state and intervention policy rather than
maintaining a separate understanding of the meeting.

## Why the current Coach loop feels worse

The failure is architectural, not evidence that the model is unintelligent.
The current Coach path repeatedly asks a model for one isolated nudge. It loses
the persistent conversation, tool use, screen inspection, user steering, and
correction semantics that make terminal Sidekick useful. A prompt that always
expects a nudge also turns ordinary uncertainty into visible chatter. That is
how a direct confirmation such as “yes, battery life is the actual agenda” can
be misread as a new contradiction instead of resolving the prior uncertainty.

The correct abstraction is a session with an abstention decision, not a
sequence of forced summaries.

## Tonight's honest demo surface

The recording and Live bars expose one **Codex Sidekick** button. It opens the
existing Recall terminal and starts Codex with:

- the generated `minutes-live-sidekick` skill;
- the current Minutes assistant context, live transcript commands, and
  exact-session screen retrieval contract;
- session-only Codex Fast mode;
- a read-only sandbox with no inherited approval-bypass flags; and
- third-party Codex apps and plugins disabled for a smaller, quieter tool
  surface; and
- an immediate bounded status/transcript read so the first visible response
  demonstrates grounding rather than presenting an empty chat box.

This preview is interactive and persistent, but it does not claim continuous
proactive monitoring. The terminal host does not yet prove event delivery,
foreground preemption, and cancellation. If another Recall assistant is
already running, Sidekick asks the user to end it instead of replacing or
injecting into it.

That is still a legitimate Build Week increment: the newly packaged feature
uses Codex Fast, Minutes' local live transcript, optional screen evidence, and
an interactive agent session. The demo should describe it as a preview of the
new Sidekick product surface, not as a newly invented transcription engine or
as finished autonomous coaching.

## Production architecture

The production surface should run `codex app-server` as a long-lived child of
Minutes. App-server is designed for rich integrations and exposes persistent
threads, streamed item deltas, approvals, turn interruption, and steering. It
is a better fit than repeatedly invoking `codex exec`, which is intended for
non-interactive automation.

```text
live transcript + screen status + typed user input
                       |
                       v
             Minutes session reducer
          (identity, policy, corrections,
           foreground priority, provenance)
                 |              |
       deterministic wake       | foreground turn
       and abstention gate       |
                 |              |
                 +------v-------+
                    Codex thread
                  via app-server
                         |
                 streamed grounded turn
                         |
             Native Recall + optional HUD
```

Minutes owns the stable meeting/session identifiers, bounded evidence window,
screen disclosure, role and posture corrections, and publish decision. Codex
owns reasoning inside a turn. A typed user message interrupts or steers an
active background turn and is always the next visible interaction. Late output
from an older focus generation is discarded.

The app-server child should run with a read-only sandbox and a capability
allow-list. Minutes should prefetch ordinary transcript state. Screen pixels
are attached only for the exact session and only when needed. Mutating tools,
external messages, reminders, and settings changes remain unavailable without
a directly typed request and the normal confirmation policy.

## Intervention policy

State-of-the-art proactive systems separate “should I speak?” from “what
should I say?” ProACT explicitly diagnoses collaboration breakdown, chooses
silence versus intervention, and then routes to a targeted collaboration
skill. Recent wake-up research similarly uses a small structured temporal gate
before paying for an LLM turn. ProactiveBench finds that larger models alone do
not reliably create appropriate proactivity.

Minutes should therefore use two stages:

1. A deterministic or small local gate updates structured meeting state and
   emits a candidate only for a material decision, contradiction, risk,
   opening, requested watch condition, or stale commitment.
2. Codex receives the candidate plus bounded evidence and either abstains or
   produces a targeted response. Minutes applies provenance, freshness,
   deduplication, cadence, and focus-generation checks before publication.

“No output” is a first-class successful result. A user confirmation supersedes
the uncertainty it answers. It must not be reinterpreted as a contradiction
without new evidence.

## Evaluation and development loop

Production quality needs four gates that exercise the same adapters and
prompts used by the shipped app.

| Gate | Purpose | Required evidence |
| --- | --- | --- |
| Deterministic reducer replay | Prove session identity, foreground priority, correction, cancellation, and wrong-session rejection | Synthetic public fixtures replay twice with identical traces |
| Production prompt/model replay | Measure abstention, intervention usefulness, contradiction handling, schema validity, and latency against every supported model tier | Machine-readable report generated by the production adapter; release command fails below thresholds |
| Temporal desktop E2E | Prove actual event order, streaming, panel state, terminal/app-server teardown, screen provenance, and focus switching | Signed `Minutes Dev.app` run with a controllable fake transcript/screen provider plus screenshots and event trace |
| Human dogfood review | Catch strategic usefulness and annoyance that labels miss | Private opt-in meeting corpus; pairwise blinded rating; no private transcript committed to the repository |

The first regression added from the reported incident is
`synthetic-agenda-confirmation`. It labels the initial topic statement, the
direct confirmation, and the first substantive fact as a no-intervention
range. The deterministic mock must stay silent; every real model candidate is
scored for emitting an unnecessary clarification there.

The release rubric for the native app-server Sidekick should include:

- 100% foreground-user preemption and wrong-session rejection;
- 0 false visual claims and 0 evidence-derived tool actions;
- 0 repeated clarification after an explicit compatible confirmation;
- at least 95% no-nudge quality on synthetic quiet ranges;
- at least 90% useful-intervention precision and 85% opportunity recall;
- no duplicate or stale publications;
- p95 typed-turn acknowledgement below 500 ms and useful first content below
  2 seconds on the target Fast-mode hardware/network class; and
- signed-app accessibility and visual acceptance in light, dark,
  high-contrast, and reduced-motion configurations.

Any threshold change requires a versioned corpus or rubric update. A report
that merely prints poor scores and exits successfully is not a release gate.

## Research references

- OpenAI Codex app-server API: <https://learn.chatgpt.com/docs/app-server#api-overview>
- OpenAI Codex Fast mode: <https://learn.chatgpt.com/docs/agent-configuration/speed>
- OpenAI Realtime session selection: <https://developers.openai.com/api/docs/guides/realtime#choose-a-realtime-session>
- ProACT proactive collaboration framework: <https://arxiv.org/abs/2607.03730>
- ProactiveBench: <https://arxiv.org/abs/2603.19466>
- Temporal-graph proactive wake-up gate: <https://arxiv.org/abs/2605.30152>
- ICLR 2025 proactive-agent benchmark: <https://proceedings.iclr.cc/paper_files/paper/2025/hash/75c37811e830bf029584b1c6fac17726-Abstract-Conference.html>
