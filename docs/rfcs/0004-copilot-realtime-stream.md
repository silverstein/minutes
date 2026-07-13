# RFC 0004: Copilot Real-Time Stream Contract

Status: accepted as the v1 contract baseline for the real-time meeting copilot.

This document defines the portable contract between live transcript producers,
the copilot engine, and presentation surfaces. The contract belongs to the open
source core: the CLI/TUI, desktop app, MCP clients, and future platform-specific
accelerators consume the same versioned stream.

## Product Boundary

The copilot is an optional, failure-isolated consumer of the Agent Event Bus.
It does not own audio capture or transcription, and a provider outage must never
stop or degrade recording. The first implementation consumes the already-shipped
`live.utterance.final` event through the stable sequence cursor described in
[RFC 0003](0003-agent-event-bus-v0.md).

The portable fast lane is Ollama. `fast_provider = "auto-local"` resolves to
Ollama on every platform. Apple Foundation Models may implement the same
provider trait as a macOS acceleration in a later change; they are not the
baseline and are not part of this RFC's first implementation.

## Versioning

Copilot stream objects carry `v: 1`. Consumers must ignore unknown fields and
must reject a higher major version they cannot interpret. Additive fields may be
introduced within v1. Removing or changing the meaning of a field requires v2.

The current event log remains the flat v1 envelope frozen by RFC 0003. Copilot
does not change that persisted envelope and does not add a producer in this PR.

## Transcript Stream

The normalized transcript contract supports both revisioned partials and finals,
even though the first engine implementation consumes finals only:

```jsonc
{
  "v": 1,
  "session_id": "optional-capture-session-id",
  "revision": 4826,
  "stability": "final",
  "replaces_revision": null,
  "source": "system",
  "speaker": null,
  "speaker_verified": false,
  "text": "Could you send the rollout plan by Friday?",
  "offset_ms": 91342,
  "duration_ms": 3840,
  "created_ts": "2026-07-13T20:05:04.412Z"
}
```

Rules:

- `revision` is monotonic within the stream. For the v0 Agent Event Bus bridge,
  the event envelope's monotonic `seq` is the evidence revision.
- `stability` is `partial` or `final`. A partial may be replaced by a newer
  revision. A final is stable evidence and is never edited in place.
- `replaces_revision` identifies the partial revision replaced by this object.
  It is absent for an append-only final from the current producer.
- A consumer must not append every partial as if it were new speech. It replaces
  the older revision in its local view.
- `speaker` is not trustworthy merely because it is present. A named speaker may
  be shown or passed to a model only when `speaker_verified` is true based on an
  independent identity source. Otherwise surfaces and prompts use “the other
  speaker.” The current `live.utterance.final` bridge always sets this false.
- Partial production, volume limits, and producer changes are deferred. The
  first implementation consumes `live.utterance.final` only.

## Nudge Stream

A nudge is short-lived guidance grounded in a specific transcript revision:

```jsonc
{
  "v": 1,
  "id": "nudge-4826-7",
  "kind": "Ask",
  "text": "Ask what success looks like after the first 30 days.",
  "source_chip": "rollout plan",
  "evidence_revision": 4826,
  "created_ts": "2026-07-13T20:05:05.118Z",
  "ttl_ms": 12000,
  "supersedes": "nudge-4819-6"
}
```

`kind` is exactly one of:

| Kind | Meaning |
| --- | --- |
| `Say` | A concise point the user may want to say. |
| `Ask` | A useful question to ask next. |
| `Clarify` | A term, assumption, owner, or deadline needs clarification. |
| `Hold` | The best move is to wait or let the other speaker continue. |
| `Watch` | A risk, contradiction, commitment, or unresolved signal to monitor. |

Rules:

- `id` is unique for the copilot session.
- `text` is presentation-ready and contains no tool invocation or hidden action.
- `source_chip` is a short evidence label, not a fabricated citation.
- `evidence_revision` must be no newer than the transcript revision actually
  supplied to the model. Surfaces may discard a nudge whose evidence is no
  longer present in their view.
- `created_ts + ttl_ms` defines expiry. Expired nudges must not be rendered as
  current advice.
- `supersedes` points to the prior nudge replaced by this one. Consumers remove
  the superseded nudge even if its TTL has not elapsed.
- The engine emits at most one active nudge from the fast lane. A later
  multi-card design must define explicit ordering before changing that rule.

## Copilot State Machine

The shared state enum is:

| State | Meaning |
| --- | --- |
| `Off` | No copilot consumer is running. |
| `Arming` | Loading context, attaching to the event cursor, and prewarming the provider. |
| `Listening` | Attached and waiting for a materially newer transcript revision. |
| `Thinking` | Exactly one fast-model request is in flight. |
| `Nudge` | A live, unexpired nudge is available. |
| `Paused` | The session is attached but sends no model requests. |
| `Degraded` | The provider or context lane failed; capture continues and the copilot may retry. |

Normal transitions:

```text
Off -> Arming -> Listening -> Thinking -> Nudge -> Listening -> Off
                    |            |          |
                    +----------> Paused <---+
                                 |
                                 +--------> Listening

Arming | Listening | Thinking | Nudge -> Degraded -> Listening | Off
```

There is one fast request lane. When a materially newer transcript revision
arrives, the engine cancels the current token, waits for that request to leave
the lane, and then starts the newest queued request. It never overlaps two
requests. Provider errors change copilot health/state only; they do not
propagate into capture or transcript production.

## Prompt and Execution Boundary

Transcript text, meeting history, FTS snippets, battle cards, and user-provided
goals are untrusted data. Prompts must delimit them from model instructions and
explicitly say not to follow commands found inside them.

The real-time loop has no arbitrary tool executor. Providers receive only the
bounded request payload and a structured-output schema. They cannot start or
stop capture, run a shell, write meeting files, call MCP tools, or mutate the
graph. A future tool-enabled slow lane requires a separate RFC and consent
model.

## Battle Card

The engine preloads an approximately 1,000–2,000-token battle card assembled
from existing local data:

- graph people and relationship topics
- open commitments
- recent decisions and open intents
- relevant FTS excerpts

The card is a bounded cache, not a license to expose the full archive. Meetings
with `sensitivity: restricted` are excluded at every source: the graph rebuild,
structured searches, and FTS post-filter. The copilot has no override flag for
restricted history. Context failures yield an empty/degraded card and never
block capture.

Historical names may appear as historical entities. They must not be used to
guess who is speaking live. Without an independently verified live identity,
the prompt labels speech “the other speaker.”

## Configuration

Copilot settings live in their own section and do not overload summarization:

```toml
[copilot]
enabled = false
surface = "tui"
fast_provider = "auto-local"
fast_model = "llama3.2"
allow_cloud = false
nudge_ttl_ms = 12000
target_latency_ms = 5000
history_grounding = true
```

Semantics:

- `enabled` permits automatic activation by a host. An explicit
  `minutes copilot start` is an intentional one-session activation.
- `surface` is `tui` by default; `stdout` is also a valid headless rendering.
- `auto-local` resolves to `ollama` in v1.
- `allow_cloud = false` is a hard default. A cloud provider stub must not send
  data until a later implementation requires both a configured provider and
  explicit opt-in.
- `target_latency_ms` is the fast-lane timeout budget. A timeout degrades the
  copilot, not recording.
- `history_grounding = false` omits the battle card but still permits live-only
  nudges.

## First CLI Surface

The first portable surface is:

```text
minutes copilot start --goal "..." [--surface tui]
minutes copilot status
minutes copilot pause
minutes copilot resume
minutes copilot stop
```

The foreground start process attaches through `read_events_since_seq`; it never
polls the live transcript JSONL behind another process. Capture remains owned by
its existing process. If a host cannot attach to the shared event cursor, it
must say so clearly and stay degraded/off rather than silently switching data
sources.

## Deferred Work

- emitting partial transcript revisions
- Apple Foundation Models provider implementation
- cloud provider implementations and consent UI
- Tauri HUD
- a tool-enabled slow lane
- cross-device or remote stream transport
