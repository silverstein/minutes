# Sidekick signed-Mac checkpoint

Date: 2026-07-24

Status: Passing product-integration milestone; not a production-readiness or
general SOTA claim.

Tested commit: `c4bd54d8646962a15be8ee322bf317de89e152d5`

## Outcome

The locally built and Apple Development-signed `Minutes Dev.app` passed the
bounded native Sidekick UI/provider acceptance on Silverbook. The run:

- launched the installed app through macOS LaunchServices;
- traversed the real Recording, cloud-consent, and Sidekick controls;
- detected sustained room-microphone signal without sending ambient speech to
  the reasoning provider;
- captured and locally verified an exact-session screen marker;
- painted two grounded responses through one persistent strategist session;
- independently verified each visible response in a fresh verifier session;
- checked the exact painted response bytes with the calibrated semantic judge;
- completed recording, screen, provider, app, and temporary-data teardown; and
- left no active recording or Minutes Dev/provider process.

The signed acceptance score was 33/33. The initial legacy diagnostic score was
17/18 because its role-flip regex did not recognize the direct, valid phrasing
“Meridian should require…”. The independent semantic judge passed the same
answer as customer-side advice. The evaluator now has a regression case for
that natural construction; product wording was not changed to game the score.

## Exact visible behavior

Turn one:

> Financial exposure: at 90% accuracy, ~4,000 wrong resolutions/month could
> cost $800,000 in Meridian credits—so headline accuracy is not decisive. Stage
> confidence-gated automation; route below-threshold tickets to humans. What is
> the error-rate distribution by confidence band?

Turn two:

> Meridian should require a written confidence-threshold SLA tied to observed
> error rates, auditable case-level records, and Meridian’s unilateral right to
> revert affected work to human handling. For every wrong automated resolution,
> vendor owes Meridian a $200 credit. Which confidence-band error distribution
> supports the threshold?

Both exact answers passed a separate semantic judge. Turn one synthesized the
unspoken $800,000 monthly exposure from the fixture’s volume, accuracy, and
per-error credit. Turn two preserved the procurement/customer role and included
the written SLA, auditable records, per-error remedy, and unilateral human
reversion right.

## Latency

| Boundary | Turn one | Turn two | Gate |
| --- | ---: | ---: | ---: |
| Provider pipeline complete | 4,455 ms | 4,746 ms | under visible paint |
| Typed request through visible DOM paint | 4,709 ms | 6,744 ms | <= 10,000 ms |

The fresh seven-scenario behavioral corpus also passed:

- 7/7 scenarios;
- 25/25 required insights;
- 3,025 ms first-token p95;
- 7,851 ms complete-response p95; and
- zero quality, privacy, role, stale-evidence, or prompt-injection failures in
  that current corpus.

The corpus is still smaller than the 30-scenario release target and does not
exercise capture or diarization, so its own report correctly sets
`release_ready` to false.

## Signed UI audit

The two app-captured screenshots were inspected at original resolution.
They are retained as local private evidence rather than committed because the
desktop background contains unrelated private work.

| Journey step | Health | Evidence and remaining gap |
| --- | --- | --- |
| Installed signed launch | Pass | Bundle seal and designated requirement passed; installed bytes matched the signed build. |
| Recording and attach | Pass | Real main control, consent control, room-mic signal, exact recording identity, and Sidekick attach were witnessed. |
| Listening state | Pass | Calm header state, active recording state, and grounded-ready status were visible. |
| Grounding disclosure | Pass with polish remaining | Live meeting, screen, and prepared brief were visible as bounded sources; the lower receipt text is visually quiet and still needs contrast review across displays. |
| Turn-one strategy | Pass | The consequence, reversible operating path, and decision-forcing question were readable and fully visible. |
| Turn-two role change | Pass | The composer remained usable, the role flip persisted, and the second answer painted without clipping. |
| Thread scanability | Needs matrix | The large serif answer treatment is distinctive, but two longer answers create a dense scroll state. MacBook-size, external-display, zoom, and five-second comprehension tests remain. |
| Keyboard and VoiceOver | Not yet proven | Automated semantics exist, but signed VoiceOver, tab-order, labels, announcements, and focus recovery still require acceptance. |
| Inline recovery | Partially proven | Race, reload, stale status, failed send, and teardown paths pass automation; signed network/provider/permission recovery has not been walked. |
| Clean teardown | Pass | No recording, provider, app, or sensitive temporary artifact survived. |

Screenshot receipts:

- turn one SHA-256:
  `898a98e7d0c0796280431c1a54a4900f70e07e34b283374452d291d6d7583fdd`
- turn two SHA-256:
  `063f2060952f7dd56924611b722f537fba84542715849a6213ae989d39e9e03e`

## Evidence-backed grades

These are checkpoint grades, not aspirations:

| Dimension | Grade | Why it is not 10/10 yet |
| --- | ---: | --- |
| Meridian strategic quality | 9.5/10 | One synthetic decision meeting is excellent, but expert-human blind parity is not established. |
| Generalized SOTA | 7/10 | Seven diverse production-path scenarios pass; the 30-scenario corpus, strongest-baseline bake-off, capture/diarization replay, and real meetings remain. |
| End-to-end UX | 7/10 | Native start, consent, attach, input, two turns, and teardown pass; recovery, accessibility, start-rate, and ten-meeting dogfood evidence remain. |
| Visual UI | 7/10 | The signed app has a coherent Minutes-native surface and stable composer; size matrix, contrast, VoiceOver, five-second comprehension, and blinded craft review remain. |

## Explicit exclusions

This checkpoint does not prove:

- live speech recognition or two-speaker diarization;
- semantic understanding of arbitrary desktop screens;
- normal cold-start behavior outside the isolated acceptance;
- provider steering or interruption;
- same-user hostile process or filesystem resistance;
- accessibility across the supported Mac matrix;
- ten consecutive real meetings without intervention; or
- superiority to production terminal Codex, expert humans, or competing meeting
  assistants.

Those exclusions are release gates, not footnotes to be waived.
