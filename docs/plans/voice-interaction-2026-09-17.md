# Voice Interaction Program

Approved by Mat on September 17, 2026 UTC (September 16 on his Mac).
This is an implementation design and acceptance contract, not a claim that the
proposed capabilities already exist. Beads owns live issue status.

## Outcome

Demonstrate a future-of-work thesis: shared context becomes editable work while
the conversation continues. This is not a Minutes fundraising pitch. The first
complete experience is a voice-editable decision board with a verified handoff
into a normal writing app or browser draft. Preserve the user's existing work.

## Execution Order

Epic: `minutes-vvf1.1`, under `minutes-vvf1`. Execute one phase at a time.
The local issue store is on Silvercloud in `/home/mat/Sites/minutes`. The Mac's
Beads database is currently unavailable; do not reset it or invent a remote.
Builds and native tests run on Mat's daily-driver Mac, not Silvercloud.

| Order | Bead | Deliverable | Acceptance |
| --- | --- | --- | --- |
| 1 | minutes-vvf1.1.1 | Interaction repair, Kore, evidence and diagnostics | Visible-thread request uses one frame; receipt/image order agrees; critique gets grounded interpretation. Current claims and reading lists research first. Unknown failures stay unknown. Local dates remain local across UTC midnight. Synthetic Live and generation probes, then local CLI rebuild. |
| 2 | minutes-vvf1.1.2 | Existing text-transfer qualification | Native browser selection replacement and caret insertion read back exactly; clipboard restored; no submit; changed focus refuses. Keep PR1019 draft until this is proven. |
| 3 | minutes-vvf1.1.3 | Shared object targeting | Stable object and document identities, revision and expiry; visible intended target; conversational clarification on ambiguity. Stale references, same text in another field, and untrusted instructions cannot authorize a write. |
| 4 | minutes-vvf1.1.4 | Conversational decision board | Structured state shared by voice and pointer; move, rename, merge, add column and reorder without regenerating HTML. Preserve manual edits, selection and history. Scoped undo refuses conflicts rather than overwriting later edits. |
| 5 | minutes-vvf1.1.5 | Background work with truthful lifecycle | Reuse independent lanes. Correlated job IDs, brief acknowledgment, bounded waiting cues, inspectable failures, real cancellation of supported subprocesses. Distinguish cancellation requested from stopped and irreversible effects from reversible work. |
| 6 | minutes-vvf1.1.6 | Bounded app work and rehearsal | Draft into one explicitly named real destination; exact readback; show changes; never Send/Submit implicitly. Full rehearsal combines references, board changes, independent work, stop, and draft handoff. |

## Existing Boundaries

Reuse the voice session, work capsules, prototype store, local approvals and
guarded text-transfer adapter. The generic generated HTML preview has an opaque
origin, no network, and no persistent storage. Do not weaken that sandbox to add
board persistence. A first-party board should own validated structured state;
generated markup never becomes trusted command authority.

The prior text-transfer branch only proves some fixture cases. An accepted AX
write is not proof of delivery. Capture identity must bind later edits, not just
match a selected string. Verify before extending the app surface.

## Waiting Is Part of the Interaction

Every long job has an acknowledgment, running state, completion or failure, and
a recovery path. Show a quiet activity indicator rather than repeated terminal
lines. One delayed spoken update must not interrupt the user or repeat on every
timer tick. Optional low-key sounds or requested hold music can accompany work;
do not generate or autoplay a song for every task. Avoid fictitious percentages
and unsupported time estimates. An animation is feedback, not a substitute for
bounded work or cancellation.

For research artifacts, obtain and preserve useful source material before
formatting it. A compact reading list should eventually use a deterministic
renderer rather than a full coding-agent round trip. A failed presentation must
leave the researched answer available. Do not rerun an identical expensive
failure automatically, switch providers silently, or widen permissions.

## Evidence And Failure Semantics

Separate audience measurements, publishing trends and normative ideals. Specify
the population and denominator before percentages. Research before analysis;
extended thinking cannot conjure missing evidence. Label historical hypotheticals
as speculation. Correct an earlier unsupported premise before making an artifact
from it.

Capture failure stage, tool/call identity, duration and a stable failure category
without logging raw screen or clipboard contents. A timeout does not establish
a permission prompt, content refusal or outage. Verify authentication in the
actual Terminal launch environment: remote SSH and native Terminal can differ.

## Review Loop

Existing configurable session logging (enabled by default) writes private local Markdown transcripts, tool
arguments (with text-transfer payload redaction) and timings. Spoken private
content can still appear in transcripts. Do not claim logs are anonymized.

Add structured outcome correlation, then review selected test sessions for
unsupported claims, redundant approvals, duplicated tools, missing audible
acknowledgments, long waits, errors, poor recoveries and timezone mistakes.
Turn each reproducible failure into a synthetic regression case. Keep real
message content out of fixtures and repository commits. A scheduled analysis
needs an explicit scope, retention period and cloud-sharing decision; no
automatic transcript export or schedule is authorized by this design.
The review work is tracked as `minutes-vvf1.1.7`.

The supplied assessment combines older 26-tool sessions and later 35-tool
sessions. Bind every regression receipt to its actual build; do not treat all
historical symptoms as present in one version. Its claims that the artifact
error proved a permission prompt are not supported. The older timeout text
offered that hypothesis without evidence. The local-date failure likewise
shows no get_status call before the model asserted UTC as authoritative.

Additional regression cases from that assessment belong in the same review
suite: price versus value and median versus premium market outcomes; attendee
versus mention retrieval; preserving date and identity corrections; fictional
character interpretation versus diagnosis; displaying an exact file path
instead of treating a spoken-style preference as a capability prohibition;
capability answers tied to the current registry; and real cancellation/status
receipts rather than guessed task state.

Search grounding alone does not validate bibliography metadata. During the
generation probe, references existed but some author/link details were wrong.
Do not call this a vetted bibliography or ship it as the final demo artifact.
The simple reading-list renderer needs primary-source metadata validation and
usable links, not a dump of grounding redirects. Keep generation success,
research quality and rendered usability as separate acceptance gates.

## Release Evidence

Track source SHA, test results, local binary and real-user acceptance separately.
Keep focused PRs. Preserve unrelated changes and the production Minutes app.
Kore is a local configuration selection; keep Morris's warm, dry personality.
Do not restart the user's microphone session automatically. Explain when a
restart is required to load changed configuration and code.
