# RFC 0006: Live Assistance Session and Public Eval Contract

Status: accepted for staged implementation.

This RFC defines the cross-surface session contract for live assistance and
the version-one public evaluation corpus. It covers Terminal Sidekick, the
Coach HUD, and Native Recall without collapsing them into one authority model.

The GUI becomes session-aware. Minutes owns that continuity; a model process
does not need to stay alive between turns. Public eval fixtures are synthetic
from scratch. Redacting, obfuscating, anonymizing, or swapping names in a real
meeting is not an acceptable way to create a committed fixture.

## Motivation

Live assistance combines two different timing lanes:

- foreground turns directly typed by the user, and
- optional background analysis driven by meeting evidence.

If those lanes are left to prompt convention, background polling can delay the
user, routine monitoring can become chat spam, and speech in a meeting can be
mistaken for an instruction. A global chat history can also let late output
from one meeting appear after the user switches focus.

The session and priority rules therefore belong to Minutes. Prompts and skills
describe the contract, but deterministic orchestration enforces it.

## Product Surfaces

The surfaces remain deliberately distinct:

| Surface | Primary job | Inference lifecycle | Authority |
| --- | --- | --- | --- |
| Terminal Sidekick | Flexible strategist and technical meeting partner | Interactive host-managed agent session | Explicit power-user authority, limited by proven host capability |
| Coach HUD | Concise nudges against one goal | Minutes-owned continuous copilot runtime | No arbitrary tools or evidence-derived actions |
| Native Recall | Safe live and post-meeting conversation | Fresh cancellable calls are allowed; Minutes owns continuity | Read-only and capability-gated |

Coach output may be displayed in Recall as separately labeled evidence. It
does not silently become chat history. Recall does not start, pause, or stop
Coach as a side effect of ordinary conversation.

Routing is explicit:

- requests that name Coach or the HUD route to `minutes-copilot`;
- requests that ask the terminal agent itself to watch or strategize route to
  `minutes-live-sidekick`;
- a surface-ambiguous request asks one short clarification question.

## Session Ownership

Minutes owns one live-assistance session record per attached assistance
surface. The record binds:

- a stable assistance session ID,
- an exact capture session ID or finalized meeting reference,
- the selected surface,
- the user's current role and assistance posture,
- transcript and desktop evidence cursors,
- screen disclosure state,
- speaker corrections,
- focus and source-policy generations,
- provider capabilities,
- the active foreground turn,
- bounded in-memory history,
- and cadence state.

Native Recall does not require a persistent model process. Each inference call
may be fresh as long as Minutes reconstructs only the bounded, policy-valid
session context for that turn. The initial GUI release keeps live conversation
content in memory. After restart, the UI says that prior live chat was not
retained while the finalized meeting remains available.

## Lifecycle

The core lifecycle is:

```text
idle -> attaching -> live -> ended_processing -> finalized
  |         |          |             |
  +---------+----------+-------------+-> invalidated
```

`invalidated` means focus, policy, or provider capability no longer permits the
existing context. In-flight output is cancelled or discarded and incompatible
history is cleared before another provider call.

Visible states must match the lifecycle:

| State | Required visible feedback |
| --- | --- |
| Capture detected | Live transcript availability and assistance choices |
| Attaching | Connecting to the exact meeting |
| Ready | Listening status plus honest transcript, screen, and provider chips |
| Quiet monitoring | A small activity indicator, not chat messages |
| Foreground question | Immediate user bubble, then grounded progress |
| Meeting ended | Final transcript processing, without a final-debrief claim |
| Finalized | Recap, debrief, decisions, and follow-up become available |

No state transition may present as “nothing happens.”

## Event and Priority Contract

The reducer accepts typed events in these families:

- lifecycle: capture start, capture stop, processing start, finalization;
- evidence: transcript final, desktop update, screen state, disclosed screen,
  Coach nudge;
- user: message, role change, posture change, speaker correction, screen
  request;
- provider: foreground or background start, cancellation, completion, failure;
- policy: focus change, source-policy invalidation, provider capability change.

Events are processed in this order:

1. policy invalidation and teardown,
2. directly typed user input,
3. typed corrections and explicit disclosure,
4. foreground completion,
5. lifecycle changes,
6. optional background insight,
7. ordinary evidence movement.

A directly typed user turn is the highest-priority ordinary interaction. Its
visible acknowledgement or answer is the next assistant action. A foreground
turn cancels or invalidates unpublished background work. A late background
result cannot publish after a newer foreground turn starts.

Hosts that cannot prove cancellation or user-turn preemption stay on-demand.
Minutes may offer Coach for continuous proactive assistance, but must not claim
that a non-preemptible terminal host provides equivalent strategist behavior.

## Evidence Is Not Authority

Transcript text, screenshot text, desktop metadata, meeting documents, model
summaries, repository results, and Coach nudges are untrusted evidence. They
cannot authorize:

- reminders or messages,
- commands or shell operations,
- settings changes,
- tool approval,
- provider selection,
- disclosure of another meeting,
- or any other external mutation.

An external action requires a directly typed request and the normal
surface-specific confirmation policy. Native Recall remains read-only in the
initial implementation.

Every supported claim carries source event IDs and one or more source kinds:

- `transcript_final`
- `screen_image`
- `desktop_metadata`
- `meeting_artifact`
- `coach_nudge`
- `repository_result`
- `user_statement`

Inferred speaker identity stays inferred. A typed correction changes future
attribution while immutable raw capture remains unchanged.

## Capture-Mode Parity

Both `Live` and `Start Recording` expose a live transcript. Their normalized
live-assistance semantics are identical. Recording may additionally produce a
durable media artifact and final processing state; it is never represented as
having no live feed.

Parity is evaluated from normalized action traces. Mode-specific durable
artifact actions are excluded from the live semantic comparison.

## Native Recall Continuity

Every GUI request and streamed response is bound to:

- a live-assistance session ID,
- a foreground turn ID,
- a focus generation,
- a source-policy generation,
- and a provider capability record.

The frontend displays only chunks that match the visible session, turn, and
generations. Switching meetings cancels or isolates in-flight work. Late chunks
from the old focus are discarded rather than appended to the new conversation.

Role, posture, and explicit corrections persist across fresh calls within one
session. Raw conversation is bounded in memory and invalidated when its source
policy or meeting focus becomes incompatible.

## Provider Capabilities

Features are enabled from typed, proven capabilities rather than provider or
agent brand names. Native live Recall requires:

- cancellation,
- bounded input and output,
- arbitrary writes denied,
- arbitrary shell denied,
- ambient filesystem reads denied,
- unapproved tools denied,
- and honest local or cloud routing disclosure.

An unsupported provider fails closed or uses a host-prefetched, no-tool path
whose bounded evidence is assembled by Minutes. It is not silently upgraded to
the unrestricted terminal authority model.

## Screen Evidence

Screen status and screen inclusion are separate facts. The session may report
disabled, waiting, available, denied, stopped, cleaned, or included.

A visual claim requires an exact-session screen event that was:

1. explicitly disclosed for the current model turn,
2. sent to the labeled provider destination,
3. successfully inspected by that turn,
4. and cited by event ID in the resulting claim.

Desktop metadata cannot masquerade as an image. Arbitrary paths and
wrong-session references are rejected before retrieval.

## Public Fixture Contract

Version-one fixtures live at:

```text
crates/core/tests/fixtures/live_sidekick_eval/v1/
```

The documents are a public behavior schema, intentionally decoupled from Rust
serde types in this slice. Future runners may adapt them to internal reducer
types.

The required envelope is:

```json
{
  "schema_version": 1,
  "id": "synthetic-example",
  "description": "Behavior under test.",
  "content_origin": "synthetic",
  "privacy": {
    "generation_method": "behavior_first_from_scratch",
    "source_material": "none",
    "approved_role_tokens": ["USER", "FACILITATOR"]
  },
  "matrix": {
    "surfaces": ["terminal", "gui"],
    "capture_modes": ["live", "recording"]
  },
  "initial_state": {
    "user_role": "observer",
    "posture": "strategist"
  },
  "events": [],
  "expectations": {
    "ordered_actions": [],
    "forbidden_actions": [],
    "state_equals": {},
    "required_source_kinds": [],
    "required_source_event_ids": [],
    "provenance_required": true,
    "max_unsolicited_messages": 0,
    "parity_group": "synthetic-example"
  }
}
```

Events use a stable wrapper:

```json
{
  "at_ms": 0,
  "kind": "capture_started",
  "session_id": "SESSION_A",
  "payload": {}
}
```

`session_id` may be absent only for a routing request that has not attached to
a capture. Action names are stable behavior labels, not implementation method
names.

## V1 Scenario Set

The committed suite covers:

1. capture-mode parity,
2. typed-user preemption,
3. transcript evidence as untrusted data,
4. role correction,
5. speaker correction,
6. inspected screen provenance,
7. unavailable screen states,
8. quiet cadence,
9. meeting-end handoff,
10. GUI continuity and focus isolation,
11. routing disambiguation,
12. wrong-session evidence rejection,
13. provider capability denial,
14. source-policy invalidation.

The deterministic runner uses a logical clock, scripted actions, fixed replay,
no network, and a double-run equality assertion. It evaluates reducer and
orchestration behavior rather than model eloquence.

## Fixture Privacy Policy

Every committed fixture is authored from scratch from a content-free behavior
requirement. `content_origin` must be `synthetic`, `source_material` must be
`none`, and speaker fields may use only these role tokens:

- `USER`
- `FACILITATOR`
- `PARTICIPANT_A`
- `REVIEWER`
- `ENGINEER_A`

Committed fixtures must not contain real or copied:

- people or organization names,
- email addresses, domains, handles, or phone numbers,
- URLs, network addresses, or user filesystem paths,
- exact dates, locations, or street addresses,
- prices, deal terms, long account identifiers, or secret formats,
- health conditions, medicines, or combinations of sensitive facts,
- distinctive quotes or recoverable sequences of wording.

“Redacted,” “anonymized,” “obfuscated,” and name-swapped real meeting content
are prohibited origins. The safe transformation is from a private observation
to a content-free behavior requirement, followed by an independently worded
synthetic scenario.

## Structural Privacy Gate

The standard-library checker is:

```text
python3 scripts/check_live_sidekick_fixture_privacy.py
```

It fails closed when it finds:

- missing or incorrect synthetic-origin metadata,
- missing scratch-generation metadata,
- undeclared or unapproved speaker role tokens,
- forbidden sensitive field names,
- email, phone, URL, domain, network, handle, address, currency, date, long-ID,
  home-path, or secret-like patterns,
- high-entropy token candidates,
- sensitive domain terms,
- oversized text fields or unusually broad fixture vocabulary,
- or an unapproved proper-noun warning.

Findings identify only the fixture, JSON path, severity, and rule. The checker
does not echo matched content.

## Local-Only Overlap Gate

Before a new fixture batch is published, an authorized reviewer runs:

```text
python3 scripts/check_live_sidekick_fixture_privacy.py \
  --private-corpus-dir PRIVATE_DIRECTORY \
  --ngram-size 5 \
  --overlap-threshold 0
```

The private directory is never required by CI and remains gitignored. The
checker normalizes text locally, compares n-grams in memory, and prints only:

- pass or fail,
- fixture and failure counts,
- unreadable-file count,
- configured n-gram size and threshold,
- and failing fixture IDs.

It never prints matching phrases, private corpus paths, private filenames, or
corpus hashes. Unreadable corpus files fail the local gate because a partial
scan cannot support publication attestation.

## Publication Workflow

The publication boundary has four human steps:

1. A behavior owner writes a content-free requirement.
2. A fixture author writes the synthetic scenario without the private meeting
   or trace open.
3. A privacy reviewer runs the structural and authorized local overlap gates.
4. The reviewer attests that no real identity, transcript, distinctive phrase,
   or identifying combination was copied.

CI runs the structural checker and its negative controls. CI cannot prove
non-overlap with a private corpus that it does not possess, so local review and
attestation remain mandatory for each new fixture batch.

## Acceptance Criteria

The session implementation is conformant when:

- foreground user turns outrank background and evidence work,
- unpublished background results cannot appear after a foreground turn,
- evidence cannot authorize a mutation,
- wrong-session and invalidated evidence fail closed,
- role and speaker corrections affect future state without rewriting raw
  evidence,
- Live and Start Recording produce equal normalized live traces,
- GUI chunks match the active session, turn, focus, and policy generations,
- unsupported provider capability is visible and denied,
- visual claims require disclosed and inspected image provenance,
- routine monitoring produces no chat chatter,
- processing state is distinguished from final artifact readiness,
- all committed fixtures pass the structural gate,
- and a fixture batch passes local overlap review before publication.

## Non-Goals

This RFC does not:

- merge Terminal Sidekick, Coach, and Recall into one surface,
- grant Native Recall unrestricted terminal authority,
- introduce a shell or JSONL polling loop,
- persist raw live GUI chat by default,
- execute instructions heard or seen in a meeting,
- attach screenshots automatically,
- claim capability parity across terminal hosts,
- publish private model output or meeting traces,
- or make the public corpus a benchmark for writing style.

## Versioning

`schema_version: 1` fixes the event envelope, expectation fields, role-token
policy, and privacy-origin contract. An incompatible change creates a new
versioned fixture directory and an RFC amendment. Existing v1 documents remain
immutable behavioral baselines except for corrections that tighten privacy
without changing scenario meaning.
