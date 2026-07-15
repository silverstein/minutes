# Live assistance: Coach, Terminal Sidekick, and Recall

Minutes has three deliberately different assistance surfaces. Choose the one
whose interaction style and authority match the work you want done.

| Surface | Best for | Who runs the model | Current status |
| --- | --- | --- | --- |
| Coach HUD | Continuous, concise nudges toward one meeting goal | Minutes' bounded Coach runtime | Available through `minutes copilot` |
| Terminal Sidekick | Flexible questions, strategy, repository investigation, and decision tracking in your terminal | Your configured terminal-agent host | Agent skill and routing are staged; continuous proactive operation still depends on proven host capabilities |
| Native Recall | Least-privilege in-app conversation attached to the exact live or finalized meeting | Fresh capability-gated calls orchestrated by Minutes | Session-aware live orchestration and its GUI are planned, not implemented in the live-sidekick foundation |

These products may share evidence, but they do not share authority. A Coach
nudge is not a terminal instruction. A transcript sentence cannot authorize a
message, command, setting change, or tool approval. Native Recall does not gain
the unrestricted authority of a terminal agent.

## Use Coach

Use Coach when you want a quiet first-party HUD that continuously evaluates the
meeting against one explicit goal:

```bash
minutes copilot start \
  --goal "Leave with a named owner and launch date" \
  --surface tui \
  --mode decision
```

See [Coach](coach.md) for modes, controls, privacy, and provider setup.

## Use Terminal Sidekick

Use Terminal Sidekick when you want the current terminal agent itself to help.
Ask explicitly, for example:

> You, the terminal agent, be my strategist during this meeting. Prioritize my
> typed questions and surface only material risks or decisions.

On hosts that expose Minutes skills as commands, select
`minutes-live-sidekick` (commonly `/minutes-live-sidekick`). Hosts without slash
commands can select the skill by name or respond to the explicit request above.
Codex, Gemini CLI, and Pi consume the portable `.agents/skills/minutes/` tree;
OpenCode uses `.opencode/skills/` and matching slash commands.

Claude Code uses the Minutes plugin. After a release containing the skill,
existing installations need the full refresh sequence inside Claude Code:

```text
/plugin marketplace update minutes
/plugin update minutes@minutes
# Then restart Claude Code
```

The first command refreshes Claude Code's cached marketplace mirror; running
only `/plugin update minutes@minutes` may incorrectly report that the stale
installation is current. If the skill remains absent after restart, use the
bounded CLI workflow below and confirm that the released plugin version
actually includes it. A repository checkout does not update an installed
plugin by itself.

Start or confirm a Minutes live transcript before asking the agent to attach:

```bash
minutes transcript --status
minutes transcript --since 2m
```

Both **Live** and **Start Recording** provide live transcript evidence.
Recording additionally preserves media and produces a finalized meeting after
processing.

The current skill is an honest operating contract, not a background daemon. If
the host cannot prove evented delivery, cancellation, and foreground
preemption, the sidekick must stay on-demand. It may make bounded transcript
reads when you ask a question, but it must not create a shell polling loop or
claim that merely leaving the terminal open provides continuous monitoring.
Use Coach when you need continuous low-latency nudges on such a host.

## Native Recall roadmap

The planned native experience attaches a Minutes-owned assistance session to
the exact capture. It will show visible attaching, ready, responding,
processing, finalized, permission, and recovery states; foreground questions
will outrank optional background insights; unsupported providers will fail
closed; and late output from another meeting will be discarded.

That interface is not implemented by the live-sidekick foundation. Mockups,
accessibility behavior, responsive layouts, error recovery, signed-app
click-testing, and real-capture dogfood are release requirements, not completed
work. Until the native integration lands, use existing Recall for its shipped
capabilities without assuming it has the live session contract described here.

## Screen evidence

“Screen available” and “screen included” are different states. A sidekick or
future native turn may make a visual claim only after an exact-session image is
explicitly disclosed to and successfully inspected by that turn. Desktop
window metadata is not an image. Disabled, waiting, denied, stopped, cleaned,
and unsupported states must be reported honestly.

The underlying screen status and bounded-retrieval contract is merged upstream,
but live-assistance disclosure is not connected yet. Signed-dev testing has
verified the permission-unavailable path. Real-image dogfood after refreshing
the macOS Screen Recording grant remains outstanding.

## Design and verification sources

- [RFC 0006](rfcs/0006-live-sidekick-session-and-eval.md) defines the target
  cross-surface contract and synthetic-fixture privacy policy.
- [The implementation plan](plans/live-assistance-orchestration-2026-07-15.md)
  distinguishes what exists from what remains deferred.
- [Developing Coach and live assistance](development/copilot.md) lists the
  checks contributors can run today.

The fourteen committed live-sidekick JSON files are synthetic, schema-valid,
and privacy-clean. Five are fully executable, four execute a named reducer
projection with explicit deferred assertions, and five remain contract-only for
future orchestration. A required independent CI job runs the public privacy,
schema, reducer, and canonical-routing gates without representing the
contract-only scenarios as implementation proof.
