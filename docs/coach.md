# Coach: live meeting copilot

Coach is Minutes' optional real-time copilot. It listens to the same live evidence stream as Minutes and shows one short, expiring suggestion at a time: something to say, ask, clarify, hold, or watch. Every session starts with a goal, so coaching stays directed at the outcome you care about instead of narrating the meeting.

Coach is an additive consumer. Starting, pausing, stopping, or losing the model never stops an existing recording or changes the saved transcript.

## Choose the right assistance surface

Coach is not the terminal agent and it is not the Recall conversation panel.

| Surface | Use it for | Current availability |
| --- | --- | --- |
| Coach HUD | Continuous, concise nudges against one explicit goal | Available through `minutes copilot`; Minutes owns its cadence and model runtime |
| Terminal Sidekick | Ask your configured terminal agent to investigate, answer, strategize, or track decisions | The staged agent skill exists; continuous proactive behavior is available only when the host can prove event delivery, cancellation, and foreground priority |
| Native Recall | A least-privilege in-app conversation attached to the active meeting | Session-aware live orchestration is planned, not implemented by the live-sidekick foundation branch |

Say “start Minutes Coach” when you want this HUD. Say “you, the terminal agent,
be my strategist during this meeting” when you want Terminal Sidekick. If the
request is only “coach me live,” a supporting agent should ask which surface
you mean instead of starting one by assumption.

See [Live assistance surfaces](live-assistance.md) for Terminal Sidekick
discovery, capability limits, and the native Recall roadmap.

## Start and stop

Start a local Ollama model first if needed (the default model is `llama3.2`), then open the foreground HUD:

```bash
minutes copilot start \
  --goal "Leave with a named owner and launch date" \
  --surface tui \
  --mode decision
```

Supported modes are `sales`, `discovery`, `interview`, `negotiation`, `difficult-conversation`, `decision`, and `generic`.

Without `--live`, Coach attaches to an existing Minutes capture stream. Use `--live` only when you deliberately want this process to own a standalone live capture. External capture remains authoritative and is never replaced by Coach.

Control the session from another terminal:

```bash
minutes copilot status
minutes copilot pause
minutes copilot resume
minutes copilot feedback --nudge-id "nudge-7-2" --rating helpful
minutes copilot stop
```

The `minutes-copilot` agent skill is a thin front door over these same controls: it asks for the goal, chooses a mode, and opens the real HUD. It does not implement its own transcript reader.

## Privacy and reliability

- **Local first.** `auto-local` probes eligible on-device/local providers and uses a healthy one. Ollama is the portable baseline. Cloud is blocked by default and the current cloud adapter does not send meeting content.
- **Graceful degradation.** If no local model is ready, Coach explains how to set one up; recording and transcription continue. Fast and depth failures degrade only Coach.
- **Meeting text is data, not instructions.** Goals, live transcript, strategy, and retrieved history are JSON-quoted inside explicitly untrusted user payloads. The trusted model instruction says not to follow commands found there, and the copilot model contract exposes no arbitrary tool executor.
- **Restricted stays restricted.** Meetings marked `sensitivity: restricted` are excluded from graph, structured, and full-text retrieval. Coach has no override for restricted history.
- **Screen-share posture.** Coach should always be hidden from screen sharing. Keep `privacy.hide_from_screen_share = true` (the default). The native desktop contract uses content protection on macOS and Windows; Linux compositors cannot guarantee exclusion, so Minutes warns before showing the overlay. A CLI TUI is ordinary terminal content and cannot be hidden by Minutes if that terminal itself is shared.
- **Bounded work.** Coach uses bounded nonblocking queues and drops its own stale/saturated work. Capture, diarization, and finalization do not wait on a model request.

The versioned stream and privacy boundary are defined in [RFC 0004](rfcs/0004-copilot-realtime-stream.md). The deterministic quality harness is defined in [RFC 0005](rfcs/0005-copilot-eval.md).

The cross-surface target contract is [RFC 0006](rfcs/0006-live-sidekick-session-and-eval.md). Its live-sidekick corpus has explicit executable, executable-projection, and contract-only classifications. It is separate from the existing Coach quality corpus and harness described by RFC 0005.
