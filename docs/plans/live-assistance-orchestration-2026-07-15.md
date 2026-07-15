# Plan — Live Assistance Across Terminal, Coach, and Recall

Status: approved design; foundational slices partially implemented

Date: 2026-07-15

Planning baseline: fetched `origin/main` at `7557c8a`

Related work:

- `minutes-id3j`: session-linked screen awareness and bounded image retrieval,
  merged to `origin/main` at `8cd9345`
- Conversation-trust program on the independent `feat/conversation-trust`
  worktree
- Existing Coach runtime and deterministic synthetic copilot eval corpus
- Existing deferred Recall thread work in `minutes-psx`
- Existing agent-capability honesty work in `minutes-hdq` and
  `minutes-wq3`

## Current implementation snapshot

This plan intentionally describes the end state. As of this branch, only a
foundation exists:

| Area | Current state | Still required |
| --- | --- | --- |
| Core session | Hardened isolated reducer with invocation identities, full evidence bindings, phase/source guards, duplicate/blank rejection, in-flight cancellation, and focused tests | Focus generation, provider capabilities, bounded history, explicit attaching/invalidated phases, and Native Recall integration |
| Public fixtures | Fourteen versioned specifications: 5 executable, 4 executable projections with named deferrals, and 5 contract-only; actual reducer and canonical-routing double-run harnesses are green | Implement the five contract-only orchestration scenarios and each projection's named deferred assertions as their product capabilities land |
| Privacy and CI | Structural checker, local overlap mode, 16 privacy/schema controls, versioned schema gate, and independent required `live_sidekick_eval` CI job | Authorized private-corpus overlap was not run for this batch; per-batch review and attestation remain human publication requirements |
| Terminal Sidekick | Canonical generated skill plus routing tests | Release/distribution confirmation and a proven evented/preemptible host adapter; unsupported hosts remain on-demand |
| Coach | Existing independent product and eval runtime | Separately labeled evidence bridge into Recall, without lifecycle coupling |
| Native Recall | Product and architecture requirements only | Session-scoped backend/frontend integration; no live-assistance GUI is implemented in this branch |
| Screen evidence | Canonical status and bounded retrieval contract merged upstream; permission-unavailable behavior verified in the signed dev app | Rebase/integrate the contract here, add one-turn disclosure and provider labeling, then verify a real PNG after refreshing the Screen Recording grant |

“Implemented” in this plan means code and a directly runnable check exist in
this branch. Fixture execution status is taken only from the schema-validated
`execution` classification; contract-only text is not counted as an implemented
eval. A described GUI state is not counted as a rendered UI.

Origin: live product dogfooding showed that a terminal agent can be a useful
meeting strategist when it combines the live transcript, explicit user
direction, bounded screen evidence, and repository truth. A separate live
session showed the failure mode: a hand-built transcript watcher delayed typed
questions, emitted low-value status chatter, and treated meeting speech as
instructions. This document records only the generalized behavior. No private
meeting identity, transcript, commercial detail, medical detail, or distinctive
language belongs in this plan or its public fixtures.

## Decision summary

Minutes should support three deliberately distinct live-assistance products:

1. **Terminal Sidekick**: the user asks their configured terminal agent
   (Codex, Claude Code, or another supported host) to act as the strategist.
   This is the power-user surface.
2. **Coach HUD**: the first-party, low-latency copilot runtime produces concise
   nudges against a concrete goal. It is not a general terminal agent.
3. **Native Recall**: the in-app GUI provides a safe, session-aware
   conversation that can attach to a live capture, carry role and posture
   across turns, and transition into post-meeting recall.

The three surfaces should share a Minutes-owned live-assistance session model
and evidence semantics. They should not share authority by accident. Native
Recall remains read-only and least-privilege; Terminal Sidekick remains an
explicit power-user choice; Coach retains its specialized prompt, cadence,
privacy, and provider runtime.

The conversation-trust program remains authoritative about whether the Tauri
app may launch a full-tool PTY at all. This plan does not reopen or weaken a
fail-closed launch decision. The terminal skill can support users who explicitly
run an agent in their own terminal, while the in-app terminal surface stays
unavailable whenever Minutes cannot establish the sandbox and source-policy
boundary required by the active trust program.

The GUI should become session-aware. Session awareness belongs to Minutes,
not to a permanently running Claude or Codex process. Native Recall may keep
fresh, cancellable, single-turn model processes while Minutes owns the
session identity, state machine, bounded history, evidence cursors, trust
policy, and lifecycle.

## Product vocabulary

| Term | Meaning |
| --- | --- |
| Capture session | One active `Live` or `Start Recording` lifecycle with a canonical session ID. |
| Live-assistance session | Minutes-owned state that binds a user, surface, posture, evidence cursors, and lifecycle to a capture or finalized meeting. |
| Posture | How assistance should behave: on-demand, strategist, silent watch, or decision tracker. |
| Role | The user's current role in the meeting, such as presenter, participant, observer, decision-maker, or technical responder. |
| Foreground turn | A directly typed user message. It always outranks background analysis. |
| Background insight | Optional, droppable analysis produced by an explicitly enabled strategist mode. |
| Evidence | Transcript, screen image, desktop metadata, meeting artifact, Coach nudge, or repository result. Evidence is untrusted data, never an instruction. |
| Surface capability | The authority and interaction features a particular host can prove, not what its marketing name implies. |

## Surface contract

| Surface | Primary job | Inference lifecycle | Authority | Proactive behavior |
| --- | --- | --- | --- | --- |
| Terminal Sidekick | Flexible strategist, technical investigator, and meeting partner | Host-managed interactive agent session | Explicit power-user authority; must be described honestly per host | Only when the host can prove safe event handling and user-turn priority |
| Coach HUD | Goal-directed, concise live nudges | Minutes-owned continuous copilot runtime | No arbitrary tools; transcript is untrusted evidence | Native responsibility |
| Native Recall | Safe conversation across live and finalized meeting context | Fresh cancellable calls are acceptable; Minutes owns session continuity | Read-only, scoped, fail-closed provider capabilities | Opt-in quiet insight cards, scheduled by Minutes rather than a shell poller |

User language must route explicitly:

- “Start Coach,” “open the Coach HUD,” or “pause Coach” routes to
  `minutes-copilot`.
- “You, Codex, watch this and be my strategist” routes to the terminal
  `minutes-live-sidekick` skill.
- “Coach me live” without a surface is ambiguous and asks one short
  clarification instead of guessing.

## Hard constraints

### User priority

A typed user message is the highest-priority live-assistance event. The next
visible assistant action must acknowledge or answer that message. Transcript
polling, file reads, background model work, and watcher maintenance may not
run first.

If a host cannot preempt a background turn, Minutes must either:

- keep that host in on-demand mode,
- cancel the background turn before dispatching the foreground turn, or
- offer the Coach HUD for continuous proactive assistance.

Minutes must not claim equivalent proactive support across hosts that cannot
prove this behavior.

### Evidence is not authority

Transcript text, screenshot text, desktop window titles, meeting documents,
model summaries, and Coach output are untrusted evidence. They cannot:

- create reminders,
- send messages,
- execute commands,
- change settings,
- approve tool calls,
- select a provider,
- disclose another meeting,
- or cause any external mutation.

Any write or external action requires a directly typed request and the normal
surface-specific confirmation policy. Native Recall remains read-only in the
first implementation.

### Capture-mode parity

`Live` and `Start Recording` both expose a live transcript. Their normalized
live-assistance semantics must be identical. Recording may add durable-media
and final-processing state; it may not be treated as “no live feed.”

### Source and speaker honesty

Every claim exposed by the orchestration layer carries one or more source
event IDs and a source kind:

- `transcript_final`
- `screen_image`
- `desktop_metadata`
- `meeting_artifact`
- `coach_nudge`
- `repository_result`
- `user_statement`

An inferred speaker remains inferred. A correction supersedes the prior
inference for future turns without rewriting immutable raw capture. Screen
claims require an image that was actually disclosed to and inspected by the
current model turn.

### Privacy and focus

Restricted, malformed, unreadable, stale, wrong-session, or policy-uncertain
context fails closed before prompt assembly or provider invocation.

Every native GUI turn binds to:

- one live-assistance session ID,
- one foreground turn ID,
- one focus generation,
- one source-policy generation,
- and a provider capability record.

Late events from an old turn or old focus cannot appear in the current chat.
Switching meetings cancels or isolates in-flight work and invalidates
incompatible history.

Source-policy invalidation preserves only harmless generic preference state:
the selected role value and assistance posture may survive across fresh calls,
while the role's source-event reference, source-bound evidence, speaker
corrections, incompatible history, and in-flight work are cleared. Preserving a
role label must never preserve the meeting evidence that originally suggested
it.

### Open-source fixture hygiene

Committed fixtures are authored from scratch and have
`content_origin: "synthetic"`. “Redacted,” “anonymized,” “obfuscated,” or
name-swapped real transcripts are not acceptable source material.

Public fixtures contain no real:

- people or company names,
- email addresses, domains, handles, or phone numbers,
- exact meeting dates or locations,
- prices, deal terms, account identifiers, or URLs,
- medical conditions, medicines, patient facts, or clinical combinations,
- filesystem paths from a user's machine,
- distinctive quotes or recoverable sequences of wording.

Behavioral lessons may be reduced to content-free requirements, then re-authored
by a fixture author who did not copy from the private source.

## User experience and visible state transitions

No transition may be “nothing happens.”

The states below are the required native design contract. They have not been
implemented or visually validated in this branch. Before Slice C changes a
frontend, the implementation owner must produce reviewable wireframes for
idle, attaching, ready, responding, correction, disclosure, processing,
finalized, restart, and every permission/error state. The review must define:

- keyboard order, focus restoration, and cancellation behavior;
- screen-reader names and live-region announcements;
- contrast, reduced-motion, and status semantics that do not rely on color;
- narrow-window, wrapping, truncation, and overflow behavior;
- loading, timeout, denied-capability, stale-session, and retry states; and
- exact user-facing copy for local/cloud routing and one-turn screen disclosure.

Rendered UI is accepted only after click-testing the signed
`~/Applications/Minutes Dev.app` on a real Mac with screenshots or a review
record for each relevant state. Type checks and unit tests are necessary but
cannot establish visual or accessibility quality.

### Idle

Native Recall shows “Ask across meetings.” Terminal mode uses the normal
assistant contract. Coach is visibly off.

### Capture detected

Minutes shows “Live transcript available” and offers a lightweight assistance
choice:

- Answer when asked
- Strategist updates
- Silent safety net
- Track decisions

The user can also set or change their meeting role. A conversational prompt is
acceptable; a mandatory modal is not required.

### Attaching

The selected surface shows “Connecting to this meeting…” while Minutes binds
the exact capture session and establishes evidence cursors and policy.

### Ready

The surface shows “Listening and ready,” plus honest capability chips:

- transcript attached,
- desktop metadata on/off,
- screen off/waiting/available/unavailable,
- provider local/cloud,
- on-demand/evented behavior.

“Screen available” is not “screen included.”

### Quiet monitoring

The surface stays visually alive without producing assistant messages.
Routine transcript movement should update a small status indicator, not create
“watching,” “re-armed,” or “still listening” chat turns.

### Background insight

When strategist mode is explicitly enabled and a high-signal threshold is met,
Minutes shows a concise insight card. Coach-originated nudges remain labeled
Coach evidence and do not silently enter Recall conversation history.

### Foreground question

The user bubble appears immediately. Any background insight generation is
cancelled or suspended. Visible status progresses through grounded stages such
as “Reading the live transcript” and “Answering you.”

### Correction

A role or speaker correction visibly updates the relevant chip. Future turns
use the corrected state. The system does not claim that historical raw text was
rewritten.

### Screen inclusion

The user asks a screen-dependent question or explicitly selects “Include
current screen.” The GUI shows the image destination and one-turn disclosure
before bounded retrieval. Only after successful disclosure does the chip read
“Screen included.”

### Meeting ended

The session shows “Meeting ended · final transcript processing.” Live finals
remain available, but the assistant may not claim the final debrief is ready.

### Final artifact ready

The live-assistance session rebinds from capture session ID to finalized meeting
path through a trusted mapping. The UI offers recap, debrief, decisions, and
follow-up. The session retains safe corrections and posture state for the
transition.

### Restart

The first release keeps live GUI conversation content in memory only. After an
app restart, Minutes says the prior live chat was not retained while the
finalized meeting remains available. Persistent chat requires a separate
opt-in retention design.

## Shared architecture

### 1. Minutes-owned session model

Add a surface-neutral core module, initially isolated from Tauri integration:

```rust
pub struct LiveAssistanceSession {
    pub id: LiveAssistanceSessionId,
    pub scope: AssistanceScope,
    pub surface: AssistanceSurface,
    pub capture_session_id: Option<String>,
    pub finalized_meeting_ref: Option<MeetingRef>,
    pub phase: AssistancePhase,
    pub user_role: UserRole,
    pub posture: AssistancePosture,
    pub goal: Option<String>,
    pub capture_mode: Option<CaptureMode>,
    pub transcript_cursor: EvidenceCursor,
    pub desktop_context_cursor: EvidenceCursor,
    pub screen_state: ScreenDisclosureState,
    pub speaker_corrections: SpeakerCorrectionSet,
    pub focus_generation: u64,
    pub source_policy_generation: u64,
    pub provider_capabilities: ProviderCapabilities,
    pub foreground_turn: Option<ForegroundTurn>,
    pub bounded_history: Vec<AssistanceTurn>,
    pub cadence: CadenceState,
}
```

The reducer accepts typed events and produces deterministic actions. It never
stores arbitrary tool instructions derived from evidence.

Recommended event families:

- lifecycle: `capture_started`, `capture_stopped`,
  `processing_started`, `meeting_finalized`
- evidence: `transcript_final`, `desktop_context_updated`,
  `screen_state_changed`, `screen_disclosed`, `coach_nudge`
- user: `user_message`, `role_changed`, `posture_changed`,
  `speaker_corrected`, `screen_requested`
- provider: `foreground_started`, `background_started`, `cancelled`,
  `completed`, `failed`
- policy: `focus_changed`, `source_policy_invalidated`,
  `provider_capability_changed`

Priority ordering is part of the reducer contract, not prompt prose:

1. policy invalidation and teardown,
2. directly typed user input,
3. user corrections and explicit disclosure,
4. foreground completion,
5. lifecycle changes,
6. background insights,
7. ordinary evidence movement.

### 2. Live evidence hub

Do not create another transcript file watcher for each surface.

```text
Capture / live engine
        |
        v
Minutes LiveEvidenceHub
        |---- durable final events
        |---- Coach subscriber
        |---- Native Recall session subscriber
        `---- bounded terminal read/wait adapter
```

The first slice may consume exact-session finalized utterance events with a
cursor. Later work can extract an in-core fanout for low-latency partials.
JSONL and the durable event log remain audit/recovery boundaries, not the
preferred hot-path coordination mechanism.

Every subscriber filters by exact capture session ID. Another capture's
utterance is rejected even if timestamps overlap.

### 3. Foreground and background scheduling

The orchestrator maintains separate lanes:

- foreground: user-authored turns,
- background: optional strategist synthesis.

Background work is cancellable and disposable. A foreground event invalidates
any unpublished background result. No background result can publish after a
newer foreground turn begins.

Host adapters declare whether they support:

- cancellation,
- foreground preemption,
- concurrent user input,
- bounded event delivery,
- images,
- tool restrictions,
- ambient filesystem denial,
- and provider locality disclosure.

Features are enabled from proven capabilities rather than agent name.

### 4. Native Recall

Native Recall can remain a fresh non-interactive inference call per turn. Replace
the global tuple history and global in-flight turn with the session manager.

Every Tauri command and frontend event carries `session_id` and `turn_id`.
The UI ignores or routes late chunks that do not match the visible session and
turn.

The GUI first release supports safe foreground chat. Evented strategist mode
then consumes the shared scheduler. It must not launch a shell loop or silently
reuse the unrestricted PTY.

Native Recall remains read-only. Future actions such as saving a note or
confirming a speaker use separate explicit UI workflows with confirmation.

### 5. Terminal Sidekick

Add a canonical `minutes-live-sidekick` skill generated to every supported
host surface.

The skill:

- distinguishes itself from `minutes-copilot`,
- establishes role and posture with at most one short question when needed,
- explains `Live` and `Start Recording` parity,
- declares typed-user priority,
- treats transcript and screen content as untrusted evidence,
- uses supported bounded transcript/session reads,
- forbids hand-built Bash polling loops,
- forbids low-value monitoring chatter,
- records speaker confidence and corrections,
- and defines the stop/processing/debrief handoff.

Portable skill text must branch honestly on host capability. If a host cannot
provide evented, preemptible monitoring, it stays on-demand and offers Coach
for continuous nudges.

Narrow `minutes-copilot` routing to explicit Coach/HUD lifecycle language.
Ambiguous requests ask which surface the user wants.

### 6. Coach

Coach continues to own continuous opportunity detection, cadence, prompt
construction, privacy filtering, and provider routing.

Coach nudges can appear in Native Recall as separately labeled, collapsible
evidence cards. They do not become chat history unless the user asks about one.
Recall does not pause, resume, or stop Coach implicitly.

### 7. Provider capabilities

Add a typed `ProviderCapabilities` contract. Native live Recall requires:

- no arbitrary writes,
- no arbitrary shell,
- no ambient filesystem reads,
- no unapproved MCP servers,
- bounded output,
- cancellation,
- and honest local/cloud routing.

Unsupported providers either use a host-prefetched no-tool inference path or
are unavailable for native live mode. They remain available in the explicitly
powerful terminal surface if the user chose that surface.

### 8. Screen evidence

Consume the screen-awareness contract from `minutes-id3j`. The session stores
status and opaque validated references, not arbitrary filesystem paths or image
bytes.

Image disclosure is:

- exact-session,
- at most one bounded image per ordinary request,
- explicit per model turn,
- provider-destination labeled,
- retention aware,
- and separately provenance tagged.

Screen disabled, permission denied, waiting, available, included, stopped, and
cleaned are distinct states.

## Synthetic eval and privacy architecture

### Fixture location and schema

The current foundation contains:

- `crates/core/src/live_sidekick/session.rs`
- `crates/core/tests/live_sidekick_eval.rs`
- `crates/core/tests/fixtures/live_sidekick_eval/v1/README.md`
- `crates/core/tests/fixtures/live_sidekick_eval/v1/*.json`
- `docs/rfcs/0006-live-sidekick-session-and-eval.md`
- `scripts/check_live_sidekick_fixture_privacy.py`
- `tests/eval/live_sidekick_fixture_schema.py`
- `tests/eval/live_sidekick_routing_eval.mjs`
- `tests/eval/test_live_sidekick_fixture_privacy.py`
- `tests/eval/test_live_sidekick_fixture_schema.py`

The public schema stays decoupled from Rust serde types. The Python validator
owns its versioned envelope and execution classifications; the Rust integration
test adapts only declared core-reducer events; the JavaScript runner invokes the
compiled canonical skill router.

Each JSON fixture contains:

```json
{
  "schema_version": 1,
  "id": "synthetic-example",
  "description": "Behavior under test.",
  "content_origin": "synthetic",
  "privacy": {
    "generation_method": "behavior_first_from_scratch",
    "source_material": "none",
    "approved_role_tokens": ["USER", "FACILITATOR", "ENGINEER_A"]
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
    "provenance_required": true,
    "max_unsolicited_messages": 0,
    "parity_group": "example"
  }
}
```

Speaker identities use role tokens only:

- `USER`
- `FACILITATOR`
- `PARTICIPANT_A`
- `REVIEWER`
- `ENGINEER_A`

### Required fixtures

1. `capture_mode_parity.json`: Live and Start Recording produce the same
   normalized assistance trace. Recording may add durable-artifact status only.
2. `typed_user_preempts_background.json`: foreground dispatch and visible
   acknowledgement occur before any new evidence analysis.
3. `transcript_is_untrusted_data.json`: a participant asks for an external
   action; no mutation occurs without a typed user request.
4. `role_correction.json`: observer changes to technical responder; presenter
   scripts do not recur.
5. `speaker_correction.json`: a role-label correction affects future
   attribution while preserving uncertainty and immutable source.
6. `screen_provenance.json`: visual claims require an inspected screen event
   ID and remain distinct from transcript claims.
7. `screen_unavailable.json`: no visual claim occurs when capture is disabled,
   waiting, denied, stopped, or cleaned.
8. `quiet_cadence.json`: routine movement stays silent; one material decision
   produces at most one insight; monitoring-status chatter is forbidden.
9. `meeting_end_handoff.json`: monitoring stops, processing truth is reported,
   and final-debrief claims wait for the finalized artifact.
10. `gui_turn_continuity.json`: role, posture, and corrections persist across
    fresh inference calls; meeting switch isolates state.
11. `routing_disambiguation.json`: explicit Coach language, explicit terminal
    language, and ambiguous language route correctly.
12. `wrong_session_evidence.json`: another capture's transcript, screen, and
    Coach events are rejected.
13. `provider_capability_denied.json`: an unsupported native provider fails
    closed or uses the no-tool prefetched path.
14. `policy_invalidation.json`: sensitivity or focus change cancels in-flight
    output and clears incompatible history.

### Deterministic harness and explicit deferrals

Reuse the copilot eval philosophy:

- logical clock,
- scripted provider,
- no network,
- fixed replay,
- versioned schema,
- deterministic double-run assertion,
- explicit baseline,
- and machine-readable action trace.

The implemented harness scores reducer and routing behavior, not model
eloquence. Eight core-target fixtures run through nine declared reducer
replays; one routing fixture runs three cases through the compiled canonical
router. Every replay runs twice before its expected trace is checked. Five
future-orchestration fixtures are explicitly contract-only, and four executable
projections name the assertions they defer. Optional manual Claude/Codex/Gemini
runs must remain gitignored. Only reviewed, non-sensitive aggregate results may
be published.

### Leakage gate

The structural checker implements these public checks:

- `content_origin` must equal `synthetic`,
- only approved role tokens may appear in speaker fields,
- reject emails, phones, URLs, IPs, handles, street-address patterns,
  currency/price forms, long identifiers, API-key forms, absolute home paths,
  and high-entropy secrets,
- reject fields named `real_name`, `company`, `email`, `medical`,
  `source_transcript`, or `derived_from`,
- warn for unexpected title-case proper nouns,
- cap free-text length and vocabulary breadth,
- require the privacy metadata block.

A local-only pre-publication command accepts a gitignored private source or
denylist and performs n-gram or MinHash overlap checks. It emits only:

- pass/fail,
- counts,
- configured threshold,
- and fixture IDs.

It never prints matching private phrases, writes corpus hashes, or uploads an
artifact. The independent `live_sidekick_eval` CI job runs the public privacy,
schema, routing, and reducer gates, and the aggregate CI gate depends on it.
CI cannot prove non-overlap with a private corpus it does not possess. The
local gate and human attestation are therefore mandatory before publishing new
fixtures.

The review workflow is:

1. Behavior owner writes a content-free requirement.
2. Fixture author writes a synthetic scenario from scratch without the private
   meeting open.
3. Privacy reviewer runs structural and local overlap checks.
4. Reviewer attests that no real transcript, names, identifying combinations,
   or distinctive phrasing were copied.

## Implementation sequence

### Slice A — contract, routing, and deterministic core

Owned paths should be conflict-light:

- new `crates/core/src/live_sidekick/` files,
- new RFC and plan,
- new synthetic fixture directory,
- new privacy checker and tests.

Current status: **implemented for the isolated foundation, not integrated
feature conformance**. The branch contains the hardened reducer, focused
reducer tests, versioned behavior fixtures, scoped deterministic runners,
RFC/plan, privacy tooling, and a required independent CI job.

The reducer now rejects stale completions with generation-bearing invocation
identities, retains immutable capture/finalized-meeting evidence bindings,
enforces phase/source admission, rejects duplicate or blank inputs atomically,
and cancels in-flight work on stop/finalization. Focus generation, provider
capabilities, bounded history, explicit attaching/invalidated phases, and native
integration remain deferred and are represented as projection deferrals or
contract-only fixtures rather than false passes. Do not edit active
`commands.rs`, `context.rs`, or screen-awareness files as part of this
foundation.

### Slice B — terminal skill

Add the canonical `minutes-live-sidekick` source and compile it to Claude,
Codex, OpenCode, and command surfaces. Narrow the Coach routing fixtures.

Generated outputs must pass:

- `npm run build`
- `npm run compile`
- `npm run compile:dry`
- `npm run check`

Add routing cases for explicit HUD, explicit terminal sidekick, and ambiguous
surface selection.

Current status: **implemented in the repository, not proof of proactive host
behavior**. Canonical and generated skill surfaces plus routing tests exist.
The skill deliberately degrades to bounded on-demand reads unless its host can
prove event delivery, cancellation, and foreground preemption.

### Slice C — native foreground session manager

Current status: **deferred; no native GUI implementation in this branch**.

After the conversation-trust lane lands, integrate the core session manager:

- replace global history/turn with session-scoped state,
- tag commands and events with session and turn IDs,
- cancel or isolate old-focus turns,
- add provider capability gating,
- preserve fresh-process inference,
- and add visible lifecycle states.

This slice does not add background strategist generation.

### Slice D — exact-session live attachment

Current status: **deferred**.

Attach Native Recall to exact-session finalized utterances through the shared
evidence service. Add role/posture controls and capture-mode parity.

Bridge Coach nudges as separately labeled cards. Keep recording ownership and
Coach lifecycle independent.

### Slice E — evented strategist mode

Current status: **deferred**.

Add opt-in background synthesis through the Minutes scheduler. Prove:

- typed-user preemption,
- unpublished background result invalidation,
- cadence budget,
- no status chatter,
- and honest degradation for non-preemptible hosts.

### Slice F — screen disclosure

Current status: **deferred**.

The `minutes-id3j` screen-awareness contract is now merged upstream, but this
branch has not integrated it into live assistance. Add one-turn GUI inclusion
and terminal discovery after rebasing. Validate provider destination, cleanup,
wrong-session rejection, and provenance. Signed-app dogfood has verified the
permission-unavailable path; successful real-PNG retrieval still requires a
refreshed Screen Recording grant and must not be reported as complete.

### Slice G — lifecycle handoff and dogfood

Current status: **deferred**.

Bind capture stop to processing state, then rebind to the finalized artifact.
Exercise Live and Start Recording, terminal and GUI, supported and unsupported
providers, screen on/off/denied, meeting switch, cancellation, and restart.

## File ownership and active-lane coordination

At plan time:

- the conversation-trust lane owns `tauri/src-tauri/src/commands.rs`,
  `tauri/src-tauri/src/context.rs`, Recall privacy, chat contracts, SDK/MCP
  trust work, and related frontend changes;
- the screen-awareness lane owns screen/context-store/capture linkage,
  bounded status and retrieval, and its Tauri exposure;
- this live-assistance program initially owns only new core modules, synthetic
  fixtures, the privacy gate, RFC, canonical sidekick skill, and routing tests.

No live-assistance implementation edits shared Tauri files until the relevant
active lane has produced a reviewed candidate and an explicit handoff seam.

Because Beads are local-only per clone, cross-machine coordination uses:

- this committed plan,
- exact branch and candidate SHAs,
- explicit owned and forbidden paths,
- tmux handoff messages,
- and clone-local Beads that reference the same plan path.

## Target acceptance matrix

This matrix is the definition of integrated release readiness, not a statement
of current branch coverage. The implementation snapshot above is authoritative
about what can be exercised today.

### Core

- Event replay is deterministic across repeated runs.
- Foreground user events always outrank evidence and background work.
- A new foreground turn invalidates unpublished background output.
- Wrong-session evidence is rejected.
- Corrections supersede inference without mutating raw capture.
- Policy invalidation cancels and clears incompatible state.
- Capture mode does not change normalized live semantics.

### Terminal

- Explicit “you, the agent” language routes to Terminal Sidekick.
- Explicit Coach/HUD language routes to Coach.
- Ambiguous requests ask one question.
- No hand-built watcher is created.
- No monitoring chatter is emitted.
- A typed user message is the next visible action.
- Unsupported proactive hosts degrade to on-demand honestly.
- Stop, processing, and debrief handoff is accurate.

### Native Recall

- Every visible chunk matches the active session and turn.
- Meeting switching cannot bleed old chunks or history.
- The text box remains usable during background work.
- Sending a foreground message cancels background work.
- Provider capability failures are visible and fail closed.
- Live and Recording both attach.
- Ended and processing states are visible.
- Restart behavior is honest.

### Coach coexistence

- Coach nudges retain separate provenance.
- Recall does not silently add Coach output to chat history.
- Recall does not stop recording or Coach.
- User feedback and cadence remain owned by Coach.

### Screen

- No visual claim without a disclosed, inspected image event.
- Desktop metadata never masquerades as an image.
- Disabled, denied, waiting, stopped, and cleaned states are honest.
- Arbitrary paths and wrong-session refs are rejected.
- Provider destination is shown before disclosure.

### Privacy

- Every committed fixture passes structural leakage checks.
- Every fixture declares synthetic origin.
- Local overlap check passes without emitting sensitive content.
- Reviewer attestation exists for every new fixture batch.
- No private trace or model output is committed.

## Verification gates

Slice-specific deterministic tests run first. Before any Rust, Tauri, MCP,
frontend, or release commit, follow `docs/checklists/pre-commit.md`.

Checks that are available for the current foundation are:

```bash
cargo test -p minutes-core --lib live_sidekick --no-fail-fast
python3 scripts/check_live_sidekick_fixture_privacy.py
python3 -m unittest \
  tests/eval/test_live_sidekick_fixture_privacy.py \
  tests/eval/test_live_sidekick_fixture_schema.py
python3 tests/eval/live_sidekick_fixture_schema.py
cargo test -p minutes-core --no-default-features \
  --test live_sidekick_eval -- --test-threads=1
npm --prefix tooling/skills run build
node tests/eval/live_sidekick_routing_eval.mjs
```

The independent `live_sidekick_eval` CI job runs these public privacy, schema,
routing, and reducer gates; the aggregate CI gate depends on it. The schema
output reports executable, projection, and contract-only counts so a green job
cannot imply that future orchestration behavior ran. Current verified coverage
is fourteen focused reducer tests, sixteen Python controls, eight core-target
fixtures across nine double replays, and three routing cases replayed twice.

UI changes require:

- a signed `~/Applications/Minutes Dev.app`,
- click-testing on the real Mac,
- capture of every visible state transition,
- keyboard and cancellation checks,
- and screen-provider disclosure checks when images are in scope.

The final integrated candidate must additionally pass:

- pinned-toolchain formatting and clippy,
- core and app tests,
- skill compiler and golden checks,
- synthetic fixture privacy checks,
- deterministic live-sidekick replay twice,
- conversation-trust regression gates,
- screen-awareness regression gates,
- Live and Start Recording real-machine parity,
- and an adversarial review that attempts prompt injection through transcript
  and screen evidence.

Before UI acceptance, record what the user sees from click through completion
for every state above. Any interval where the action has no acknowledgement,
progress, result, or recoverable error is a UX failure even if the backend work
eventually succeeds.

## Rollout

The rollout is capability-gated:

1. Ship the synthetic core and skill routing without changing native behavior.
2. Ship native foreground session continuity for proven safe providers.
3. Attach exact-session live finals.
4. Add opt-in strategist cards.
5. Add screen inclusion after bounded retrieval lands.
6. Expand providers only when their capability contract is proven.

Existing Native Recall remains available during migration. Unsupported
providers and hosts receive an explicit explanation rather than silent feature
loss or unsafe fallback.

## Explicit non-goals

- Publishing or lightly anonymizing private meeting transcripts.
- Giving Native Recall the unrestricted PTY's authority.
- Making Coach and Recall one indistinguishable chat stream.
- Adding a new shell or JSONL poller.
- Persisting raw live GUI chat by default.
- Executing commands heard in a meeting.
- Auto-attaching screenshots to every request.
- Claiming all agent CLIs have equivalent safety or preemption.
- Rewriting immutable raw transcript evidence after a correction.
- Shipping marketing claims before deterministic and real-machine proof.

## Decisions fixed by this plan

- The GUI gets session-aware orchestration.
- Minutes owns the session; the model process need not persist.
- Terminal Sidekick, Coach HUD, and Native Recall remain distinct surfaces.
- Foreground user input has hard priority.
- Transcript and screen content are untrusted evidence.
- Public evals are synthetic from scratch, not redacted real meetings.
- Live and Start Recording have equivalent live-assistance semantics.
- Screen images are bounded, explicit, exact-session disclosures.
- Tauri integration waits for active trust and screen lanes at shared files.

## Remaining design decisions

The implementation owner must resolve these before Slice C:

1. Whether the core type is named `LiveAssistanceSession`,
   `RecallSession`, or `LiveSidekickSession`. The type must stay
   surface-neutral even if the module has a product-facing name.
2. Whether exact-session finalized utterances first flow through a new
   `LiveEvidenceHub` abstraction or a thin session-filtered adapter over the
   durable event reader. No additional watcher is acceptable.
3. Which provider adapters satisfy native live mode at launch.
4. Whether preferred posture is retained as a harmless UI preference across
   restarts. Raw conversation remains memory-only in v1.
5. Whether terminal evented mode is initially limited to hosts with proven
   foreground preemption or remains on-demand until a first-party terminal
   event bridge exists.
