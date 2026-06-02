# Minutes Eval v0.1 — Results

## Agent: `claude` &middot; run 20260420-101437

Fixture: `/Users/you/Sites/minutes/tests/eval/fixtures/meetings` &middot; 20 questions

### `q01` &middot; relationship_ownership

**Q:** Who currently owns the Northwind customer relationship?

**Expected:**
Priya Raghavan. Ownership was formally transferred from Mat to Priya at the customer success review on 2026-04-14. Mat is now escalation-only.

**Rubric:**
  - [ ] Names Priya as current owner
  - [ ] Notes the handoff happened at the 2026-04-14 CS review
  - [ ] Notes Mat stepped back to escalation-only (or equivalent)

**Response:**
> Priya owns the Northwind relationship. Ownership was formally transferred from Mat to Priya on 2026-04-14 (`2026-04-14-cs-review-priya.md`, decision: "Priya is the relationship owner for Northwind going forward. Mat steps back to escalation-only"), after Riley began contacting Priya directly.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly states "Priya owns the Northwind relationship."
RUBRIC_2: pass — The answer notes the transfer occurred on 2026-04-14 and cites the CS review file.
RUBRIC_3: pass — The answer quotes the decision that Mat "steps back to escalation-only."
VERDICT: PASS
```


### `q02` &middot; relationship_ownership

**Q:** Who at the company has spoken directly to Riley at Northwind?

**Expected:**
Mat spoke to Riley on 2026-03-04 (initial SSO complaint) and 2026-03-30 (status follow-up). Priya also spoke to Riley recently, per the 2026-04-14 CS review where Priya noted Riley had started calling her directly. Alex, Sam, Jordan, and Jamie are not documented as having spoken to Riley.

**Rubric:**
  - [ ] Names Mat and Priya
  - [ ] Cites at least one of the Mat-Riley meetings (2026-03-04 or 2026-03-30)
  - [ ] Does not falsely claim Alex, Sam, Jordan, or Jamie spoke to Riley

**Response:**
> **Mat and Priya** have spoken directly to Riley at Northwind.
> 
> - **Mat**: ran both 1:1 calls — the original customer escalation (`2026-03-04-northwind-customer-call.md`) and the SSO status follow-up (`2026-03-30-northwind-status-followup.md`).
> - **Priya**: per the CS review on `2026-04-14-cs-review-priya.md`, "Riley called Priya directly last week," and Mat handed her the Northwind relationship going forward. She's also the assignee to tell Riley about scoped reporting exports in `2026-04-17-product-prioritization.md`.
> 
> Other mentions of Riley (eng standup on 2026-04-07, voice memo 2026-04-10) are secondhand references, not direct conversations.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names both Mat and Priya as having spoken directly to Riley.
RUBRIC_2: pass — The answer cites both Mat-Riley meetings (2026-03-04 and 2026-03-30).
RUBRIC_3: pass — The answer does not claim Alex, Sam, Jordan, or Jamie spoke to Riley; it only references secondhand mentions in the eng standup and voice memo.
VERDICT: PASS
```


### `q03` &middot; relationship_ownership

**Q:** Who is the primary engineering point of contact for customer-facing shipping commitments?

**Expected:**
Alex Kim. Alex owns the SSO nested-groups feature (shipped 2026-04-03), team billing (ships 2026-04-15), and the scoped reporting-exports fix (ships 2026-05-02). Every customer-facing engineering commitment in the corpus routes through Alex.

**Rubric:**
  - [ ] Names Alex
  - [ ] Cites at least two of: SSO, team billing, reporting exports

**Response:**
> Based on the corpus, **Alex K.** is the primary engineering point of contact for customer-facing shipping commitments.
> 
> Evidence:
> - `2026-03-11-eng-standup.md`: Alex owns SSO nested-groups, the commitment Mat made to Riley at Northwind.
> - `2026-03-30-northwind-status-followup.md`: Alex assigned the remaining SSO fix tied directly to the Northwind customer deadline.
> - `2026-04-07-eng-standup.md`: Alex confirms SSO shipped to Northwind and takes the next customer-facing ship (team billing, due 2026-04-15).
> 
> Mat personally makes the commitments to customers, but Alex is the engineer who owns delivery.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names Alex K. as the primary engineering point of contact.
RUBRIC_2: pass — The answer cites both SSO (nested-groups) and team billing, satisfying the two-of-three requirement.
VERDICT: PASS
```


### `q04` &middot; active_priorities

**Q:** What is the current Q2 engineering focus?

**Expected:**
Team billing (ships 2026-04-15) and a scoped reporting-exports fix (ships 2026-05-02). Advanced analytics was explicitly cut at the 2026-04-17 prioritization meeting.

**Rubric:**
  - [ ] Names team billing
  - [ ] Names reporting exports
  - [ ] Mentions that advanced analytics was killed (or equivalent)

**Response:**
> Q2 engineering focus (locked 2026-04-17 in `2026-04-17-product-prioritization.md`):
> 
> 1. **Team billing** — primary engineering investment, ships April 15 (already in flight). Unlocks the consultant-agency segment.
> 2. **Scoped reporting exports** — date-ranged CSV only (no dashboard, no charts); Alex to ship by May 2.
> 3. **Advanced analytics killed** — removed from roadmap; zero at-risk accounts cited it.
> 
> The April 10 voice memo (`2026-04-10-voice-memo-q2-priorities.md`) also flagged retention for at-risk accounts as a third theme, but the April 17 meeting converted that into the scoped-exports decision rather than a standalone engineering track.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names "Team billing" as the primary engineering investment.
RUBRIC_2: pass — The answer explicitly names "Scoped reporting exports" with the May 2 ship date.
RUBRIC_3: pass — The answer explicitly states "Advanced analytics killed — removed from roadmap."
VERDICT: PASS
```


### `q05` &middot; active_priorities

**Q:** Is the company currently offering monthly billing to new customers?

**Expected:**
No. Monthly billing was launched 2026-02-28 for the consultant segment, and the experiment was explicitly reversed on 2026-03-25. The company is now annual-only across all segments. The four existing monthly customers were given a soft migration path rather than a forced switch.

**Rubric:**
  - [ ] Answers No
  - [ ] Cites the 2026-03-25 reversal
  - [ ] Notes the experiment supersedes the 2026-02-28 launch decision

**Response:**
> No. The monthly billing experiment was reversed on 2026-03-25 (`2026-03-25-pricing-reversal.md`) — decision reverts to **annual-only** across all segments, explicitly superseding the 2026-02-28 decision. Only four existing monthly customers remain (grandfathered with soft migration); no new monthly signups.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer begins with "No."
RUBRIC_2: pass — The answer explicitly cites the 2026-03-25 reversal with a source file reference.
RUBRIC_3: pass — The answer states the reversal "explicitly supersed[es] the 2026-02-28 decision."
VERDICT: PASS
```


### `q06` &middot; active_priorities

**Q:** What is Priya's current top priority?

**Expected:**
Retention of three at-risk accounts — Northwind, Seabreeze Labs, and Canton Research. Priya committed to personal outreach to all three by 2026-04-18 and to standing up a weekly at-risk review cadence with a first session on 2026-04-21.

**Rubric:**
  - [ ] Names retention or at-risk accounts
  - [ ] Names all three accounts (Northwind, Seabreeze, Canton)
  - [ ] Notes the weekly review cadence starting 2026-04-21

**Response:**
> Priya's current top priority is **owning the at-risk accounts / retention motion** — specifically Northwind, Seabreeze Labs, and Canton Research.
> 
> Per the `2026-04-14-cs-review-priya.md` meeting, Mat formally transferred relationship ownership of Northwind to her and tasked her with:
> - Personal outreach to all three at-risk accounts by **2026-04-18**
> - Standing up a **weekly at-risk review cadence**, first session **2026-04-21**
> 
> This is reinforced in the `2026-04-10` voice memo, where Mat names retention (Priya's triage of at-risk accounts) as one of the three Q2 priorities and explicitly asks Priya to bring him options. The `2026-04-17` prioritization meeting adds a near-term follow-up — tell Riley (Northwind) that scoped CSV exports ship by May 2 — but that's a subtask under the broader retention focus.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names "at-risk accounts / retention motion" as the top priority.
RUBRIC_2: pass — All three accounts (Northwind, Seabreeze Labs, Canton Research) are named.
RUBRIC_3: pass — The answer notes the weekly at-risk review cadence with first session on 2026-04-21.
VERDICT: PASS
```


### `q07` &middot; what_changed

**Q:** What feature got killed in the most recent product prioritization meeting?

**Expected:**
Advanced analytics. It was on the Q2 roadmap and was cut at the 2026-04-17 prioritization meeting. Priya confirmed zero at-risk accounts cited analytics as a concern.

**Rubric:**
  - [ ] Names advanced analytics as killed
  - [ ] Cites the 2026-04-17 meeting

**Response:**
> **Advanced analytics** was killed at the 2026-04-17 Product Prioritization meeting. It had been on the Q2 roadmap since Q4 but no customers were pulling for it — Priya confirmed zero at-risk accounts cited analytics as a pain point.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names "Advanced analytics" as what was killed.
RUBRIC_2: pass — The answer cites the 2026-04-17 Product Prioritization meeting.
VERDICT: PASS
```


### `q08` &middot; what_changed

**Q:** What changed about the Northwind relationship recently?

**Expected:**
Ownership transferred from Mat to Priya at the 2026-04-14 CS review. Mat is now escalation-only. The underlying driver is that Riley had started calling Priya directly.

**Rubric:**
  - [ ] Names the ownership transfer from Mat to Priya
  - [ ] Cites the 2026-04-14 CS review

**Response:**
> Recent changes to the Northwind relationship, in order:
> 
> - **2026-03-30** (`2026-03-30-northwind-status-followup.md`): Mat delivered a status call per his March 4 commitment. SSO nested-groups fix slipped from March 31 to April 3. Riley accepted, but flagged reporting exports (CSV-only, no date range) as a new pain point.
> - **2026-04-14** (`2026-04-14-cs-review-priya.md`): Two big shifts. (1) **SSO issue is resolved** — Riley confirmed. (2) **Relationship ownership formally transferred from Mat to Priya**; Mat is now escalation-only. Trigger: Riley called Priya directly the prior week, signaling he'd "moved past" Mat. Northwind is still on the at-risk list, now over reporting exports.
> 
> Net: SSO crisis closed, ownership handed off, new (smaller) gripe open on reporting.

**LLM pre-grade:**
```
RUBRIC_1: pass — The agent explicitly states "Relationship ownership formally transferred from Mat to Priya" with Mat now escalation-only.
RUBRIC_2: pass — The agent cites the 2026-04-14 CS review by date and filename.
VERDICT: PASS
```


### `q09` &middot; what_changed

**Q:** What's new about the reporting-exports situation?

**Expected:**
Reporting exports was explicitly off-roadmap as of 2026-03-30 when Riley raised it. At the 2026-04-17 product prioritization meeting it was given a scoped green-light: date-ranged CSV exports only (no dashboard, no visualizations), bounded to two weeks of engineering, shipping 2026-05-02.

**Rubric:**
  - [ ] Notes it was previously off-roadmap
  - [ ] Notes the 2026-04-17 scoped green-light
  - [ ] Includes the scope constraint (date-ranged CSV, no dashboard)
  - [ ] Includes the 2026-05-02 ship date

**Response:**
> The reporting-exports situation was decided at the 2026-04-17 product prioritization meeting.
> 
> **What's new:** It got a scoped green-light. The team approved a date-ranged CSV export — no dashboard, no visualizations — bounded to two weeks of engineering. Alex owns shipping by **2026-05-02** (10 working days from 2026-04-20), and Priya will tell Riley (Northwind) it ships by May 2.
> 
> **Arc leading up to it:**
> - 2026-03-30 — Riley at Northwind complained exports were "useless" (CSV only, no date range). Mat flagged internally, not on roadmap.
> - 2026-04-07 eng standup — Mat assigned himself to scope it for the April 17 prioritization meeting.
> - 2026-04-10 voice memo — Mat framed it as a scoping question: "two-week fix or two-month platform thing."
> - 2026-04-14 CS review — Priya noted reporting exports is now Northwind's primary pain point post-SSO; account ownership transferred Mat → Priya.
> - 2026-04-17 — scoped version approved, addresses Northwind plus the 2-3 other customers Priya/Jordan expect to ask.

**LLM pre-grade:**
```
RUBRIC_1: pass — The arc explicitly notes "not on roadmap" as of 2026-03-30.
RUBRIC_2: pass — Clearly states the 2026-04-17 prioritization meeting granted a scoped green-light.
RUBRIC_3: pass — Includes "date-ranged CSV export — no dashboard, no visualizations."
RUBRIC_4: pass — States Alex owns shipping by 2026-05-02.
VERDICT: PASS
```


### `q10` &middot; decision_history

**Q:** Has the pricing strategy changed in the last two months? If so, describe the sequence of decisions.

**Expected:**
Yes. Two decisions, in order. (1) 2026-02-28: launch monthly billing narrowly to the consultant segment, keep annual as default for enterprise. Threshold was 12 signups by end of March. (2) 2026-03-25: reverse the monthly billing experiment. Only four signups came in versus the 12-signup threshold; churn on the four was 50% versus the annual baseline. Company is now annual-only across all segments.

**Rubric:**
  - [ ] Identifies both decisions in order
  - [ ] Cites both dates (2026-02-28 and 2026-03-25)
  - [ ] Notes the second decision supersedes the first
  - [ ] Explains why the reversal happened (signup miss and/or churn)

**Response:**
> Yes — pricing strategy was tested and reversed within the two-month window.
> 
> **Sequence of decisions:**
> 
> 1. **2026-02-28** (`2026-02-28-pricing-strategy.md`) — Launched monthly billing as an experiment, gated to the next three consultant signups. Annual remained default for enterprise/SMB self-serve. Success bar: 12 monthly signups by end of March. Known risk: payback stretched from month 11 → 14–15.
> 
> 2. **2026-03-11** (`2026-03-11-eng-standup.md`) — Implementation in flight: Alex reviewing/merging the Stripe monthly-billing PR to unblock Jordan's landing page.
> 
> 3. **2026-03-25** (`2026-03-25-pricing-reversal.md`) — Reversed. Only 4 signups in 3 weeks (vs. 12 bar), and actual churn tracking at 6.4% vs. the 4% assumption (2 of 4 canceled before month two). Decision frontmatter explicitly `supersedes` the 2026-02-28 decision. Reverted to annual-only across all segments; existing 4 monthly customers offered soft migration (annual at one-time discount or ride monthly until renewal), not forced flip.
> 
> Net: back to annual-only, with a documented reversal rather than a silent rollback.

**LLM pre-grade:**
```
RUBRIC_1: pass — Identifies both decisions in the correct order (2026-02-28 launch, then 2026-03-25 reversal).
RUBRIC_2: pass — Cites both 2026-02-28 and 2026-03-25 explicitly.
RUBRIC_3: pass — States the reversal decision `supersedes` the prior one and reverts to annual-only.
RUBRIC_4: pass — Explains reversal via signup miss (4 vs. 12) and churn (2 of 4 canceled / 6.4% vs. 4%).
VERDICT: PASS
```


### `q11` &middot; decision_history

**Q:** What's the current state of the SSO nested-groups commitment?

**Expected:**
Shipped on 2026-04-03, three days after Riley's originally-promised date of 2026-03-31. Riley confirmed delivery per the 2026-04-07 eng standup. The commitment is closed.

**Rubric:**
  - [ ] Ship date: 2026-04-03 (or April 3)
  - [ ] Notes it shipped 3 days late vs the 2026-03-31 promise
  - [ ] Notes it is shipped (not still open)

**Response:**
> **Shipped and closed.** SSO nested-groups shipped 2026-04-03, three days past the original 2026-03-31 Northwind commitment:
> 
> - **2026-03-04** (Northwind call): Mat committed SSO-first onboarding fix by 2026-03-31 after Riley flagged nested Okta groups breaking their login.
> - **2026-03-11** (eng standup): Alex took it, targeted 2026-03-25 for one week of slack.
> - **2026-03-30** (Northwind follow-up): Slipped — 80% done, reset to 2026-04-03. Riley accepted, annoyed but not churning.
> - **2026-04-07** (eng standup): Shipped 2026-04-03, Riley confirmed. Commitment closed.
> - **2026-04-14** (CS review): Priya confirms SSO resolved; Northwind's new pain point is reporting exports.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer states it shipped 2026-04-03.
RUBRIC_2: pass — The answer explicitly notes it was three days past the original 2026-03-31 commitment.
RUBRIC_3: pass — The answer clearly states "Shipped and closed" and "Commitment closed."
VERDICT: PASS
```


### `q12` &middot; decision_history

**Q:** Are there any decisions in the corpus that directly contradict earlier decisions?

**Expected:**
Yes. The 2026-03-25 pricing-reversal decision explicitly supersedes the 2026-02-28 decision to launch monthly billing. The frontmatter on the 2026-03-25 meeting names this explicitly via a `supersedes` field.

**Rubric:**
  - [ ] Identifies the pricing decision pair
  - [ ] Cites both dates
  - [ ] Names the later as superseding the earlier

**Response:**
> Yes — one explicit contradiction, plus one schedule slip that effectively overrides an earlier commitment.
> 
> **Explicit reversal (pricing):**
> - `2026-02-28-pricing-strategy.md`: decided to **launch monthly billing** alongside annual for consultants.
> - `2026-03-25-pricing-reversal.md`: decided to **revert to annual-only**, with `supersedes: "2026-02-28 decision to launch monthly billing"` declared in the frontmatter. The transcript states it outright: "we are reversing the 2026-02-28 decision."
> 
> **Schedule slip (SSO nested-groups):**
> - `2026-03-11-eng-standup.md`: Alex to ship SSO nested-groups by **2026-03-25**.
> - `2026-03-30-northwind-status-followup.md`: new ship date **2026-04-03, three days late** — overrides the earlier date but isn't flagged with `supersedes:`, so it reads as a slip rather than a formal reversal.
> 
> No other decisions in the corpus contradict earlier ones. The 2026-04-17 kill of advanced analytics removes a *roadmap* item but not a prior recorded decision.

**LLM pre-grade:**
```
RUBRIC_1: pass — The agent identifies the pricing decision pair (2026-02-28 launch monthly billing vs 2026-03-25 reversal to annual-only).
RUBRIC_2: pass — Both dates (2026-02-28 and 2026-03-25) are cited explicitly.
RUBRIC_3: pass — The agent names the 2026-03-25 decision as superseding the 2026-02-28 decision, quoting the `supersedes` frontmatter field.
VERDICT: PASS
```


### `q13` &middot; decision_history

**Q:** What did Jamie ask for at the 2026-03-18 investor check-in, and has it been delivered?

**Expected:**
Jamie asked for a Q2 OKR doc with three objectives and three key results each, originally due first week of April, something he could show the board at the April meeting. It had not been delivered by the end of the corpus on 2026-04-17; Mat re-committed at that meeting to finalizing and sending it on 2026-04-18. Delivery is not confirmed in the corpus.

**Rubric:**
  - [ ] Names the Q2 OKR doc
  - [ ] Notes 3 objectives, 3 KRs each
  - [ ] States it has not been confirmed delivered in the corpus (or equivalent)

**Response:**
> Jamie asked for a **Q2 OKR doc** — three objectives with three measurable key results each — deliverable by the first week of April (due 2026-04-04) so he could bring it to the April board meeting.
> 
> **Not yet delivered as of 2026-04-20.** The original action item in `2026-03-18-investor-checkin-jamie.md` is still `status: open`. Mat drafted a raw version while walking on 2026-04-10 (`2026-04-10-voice-memo-q2-priorities.md`), then re-committed on 2026-04-17 to "finalize and send to Jamie tomorrow" (due 2026-04-18) after the product-prioritization meeting locked in scope. That follow-up action item is also still `status: open`, and no later meeting confirms delivery — so it's missed the original April-4 deadline and is at least two days past the revised April-18 target.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names the "Q2 OKR doc".
RUBRIC_2: pass — The answer states "three objectives with three measurable key results each".
RUBRIC_3: pass — The answer says "Not yet delivered as of 2026-04-20" and notes no later meeting confirms delivery.
VERDICT: PASS
```


### `q14` &middot; staleness

**Q:** What commitments are overdue or at risk of being overdue relative to the corpus end date of 2026-04-17?

**Expected:**
The Q2 OKR doc is the clearest overdue item — originally due first week of April per the 2026-03-18 investor check-in, not yet delivered. Jordan's commitment to email the four monthly-billing customers with migration-back-to-annual options was due 2026-04-01 and status is not confirmed in any later meeting. Team billing ships 2026-04-15 and may be close to on-time; corpus ends two days after.

**Rubric:**
  - [ ] Names the Q2 OKR doc
  - [ ] Either: names the Jordan monthly-customers email, OR explicitly notes only the OKR doc is clearly overdue

**Response:**
> ## Overdue / at-risk commitments relative to corpus end 2026-04-17
> 
> ### Overdue and still unresolved
> - **Mat — Q2 OKR doc to Jamie** (`2026-03-18-investor-checkin-jamie.md`, due **2026-04-04**): 13 days overdue. Reappears on 2026-04-17 as a fresh action due 2026-04-18, so it was missed and rescheduled.
> - **Alex — team-billing backend ship** (`2026-04-07-eng-standup.md`, due **2026-04-15**): 2 days past due; no subsequent meeting confirms it shipped.
> - **Sam — team-billing invoice UI handoff to Alex** (`2026-04-07-eng-standup.md`, due **2026-04-10**): 7 days past due; no handoff confirmation in later files. (Related earlier item: design due 2026-03-18 in the 2026-03-11 standup.)
> 
> ### Overdue with no explicit close-out in corpus
> From `2026-03-25-pricing-reversal.md` (monthly-billing wind-down):
> - Jordan — pull monthly billing from landing page (due 2026-03-27)
> - Alex — disable monthly Stripe price (due 2026-03-27)
> - Jordan — email four monthly customers about migration (due 2026-04-01)
> 
> No follow-up meeting confirms these closed, so status is ambiguous.
> 
> ### Overdue but resolved by later meetings (low risk)
> - Alex's SSO nested-groups chain (2026-03-11 due 2026-03-25 → slipped to 2026-04-03 in 2026-03-30 → 2026-04-14 notes "SSO issue resolved").
> - Mat's SSO-first onboarding fix (due 2026-03-31) — same resolution signal on 2026-04-14.
> - 2026-02-28 pricing items (superseded explicitly by the 2026-03-25 reversal).
> - Mat's Riley follow-up (due 2026-03-30) and reporting-exports scoping (due 2026-04-16) — both satisfied by the 2026-03-30 and 2026-04-17 meetings.
> 
> ### At risk (due on/just after 2026-04-17)
> From `2026-04-14-cs-review-priya.md` and `2026-04-17-product-prioritization.md`:
> - Priya — reach out to three at-risk accounts (due 2026-04-18)
> - Priya — tell Riley exports ships by May 2 (2026-04-18)
> - Sam — remove analytics mockups from roadmap page (2026-04-18)
> - Mat — finalize Q2 OKR doc and send to Jamie (2026-04-18) — this is the re-slip of the 2026-04-04 commitment, so doubly at risk.
> 
> **Biggest red flag:** Mat's OKR-to-Jamie promise has already slipped once; the 2026-04-18 version is the second bite and the corpus shows no draft landing before 2026-04-17.

**LLM pre-grade:**
```
RUBRIC_1: pass — The agent explicitly names the Q2 OKR doc as overdue (due 2026-04-04, 13 days overdue).
RUBRIC_2: pass — The agent names Jordan's commitment to email the four monthly customers about migration (due 2026-04-01).
VERDICT: PASS
```


### `q15` &middot; staleness

**Q:** What did Mat personally commit to Riley, and was it kept?

**Expected:**
Mat personally committed on 2026-03-04 to shipping the SSO nested-groups onboarding fix by end of Q1 (2026-03-31), and to following up the week of 2026-03-30 regardless of shipping status. Result: (a) the ship-by- 2026-03-31 commitment was NOT kept on that date; it slipped to 2026-04-03, three days late, but did ship. (b) The call-regardless commitment WAS kept: Mat called Riley on 2026-03-30 with an honest status update.

**Rubric:**
  - [ ] Names the SSO fix commitment
  - [ ] States the SSO commitment was NOT kept on the original date
  - [ ] Notes it did ship (late) on 2026-04-03
  - [ ] Credits Mat for keeping the secondary follow-up-regardless commitment

**Response:**
> ## Mat's personal commitments to Riley (Northwind, 2026-03-04)
> 
> 1. **Ship SSO-first onboarding fix by end of Q1 (2026-03-31)** — `promised_to: riley`
> 2. **Follow up with Riley the week of March 30**
> 
> ## Were they kept?
> 
> **Follow-up call:** Kept on time. Mat called on 2026-03-30 as promised (`2026-03-30-northwind-status-followup.md`).
> 
> **SSO fix:** Kept, but late. Mat used the follow-up call to deliver the miss honestly — 80% done, re-committed to 2026-04-03 (3 days past the original March 31 deadline). Per `2026-04-07-eng-standup.md:43`: *"SSO nested-groups shipped 2026-04-03. Riley confirmed receipt. That commitment is closed."*
> 
> Net: commitment delivered three days late, but the relationship survived because Mat proactively surfaced the slip on 2026-03-30 rather than letting Riley discover it.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names "Ship SSO-first onboarding fix by end of Q1 (2026-03-31)" as a commitment.
RUBRIC_2: pass — The answer states the SSO fix was "Kept, but late" and missed the original March 31 deadline by 3 days.
RUBRIC_3: pass — The answer explicitly notes it shipped on 2026-04-03.
RUBRIC_4: pass — The answer credits Mat for calling Riley on 2026-03-30 as promised ("Kept on time").
VERDICT: PASS
```


### `q16` &middot; staleness

**Q:** What open action items involve Alex as of the end of the corpus?

**Expected:**
Two. (1) Ship team billing backend (multi-seat Stripe subscriptions) by 2026-04-15. (2) Ship the scoped reporting-exports fix (date-ranged CSV) by 2026-05-02.

**Rubric:**
  - [ ] Names both: team billing and reporting exports
  - [ ] Includes both due dates

**Response:**
> By `status: open` in the frontmatter, Alex has seven open action items — but narrative evidence in later meetings closes most of them. Net view:
> 
> **Genuinely still open at end of corpus (2026-04-17):**
> - `2026-04-17-product-prioritization.md` — Ship scoped reporting-exports (date-ranged CSV), due **2026-05-02**.
> - `2026-04-07-eng-standup.md` — Team billing backend (multi-seat Stripe subs), due **2026-04-15**. Frontmatter still `open`; 2026-04-17 transcript has Alex saying "Confirmed" on track but no explicit completion, so technically unclosed.
> 
> **Marked `open` in frontmatter but closed by later meetings:**
> - SSO nested-groups (2026-03-11, due 2026-03-25) and the remaining SSO fix (2026-03-30, due 2026-04-03) — 2026-04-07 standup: "SSO is out. Shipped Friday… That commitment is closed."
> - Wire monthly price into Stripe (2026-02-28) and code-review the Stripe monthly PR (2026-03-11) — shipped, then reversed on 2026-03-25.
> - Disable the monthly Stripe price (2026-03-25, due 2026-03-27) — 2026-04-10 voice memo refers to "when we killed monthly billing" in past tense.
> 
> Note: no frontmatter item is ever flipped to `done` in this corpus — status tracking happens in narrative, not YAML.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly names both "team billing backend" and "reporting-exports" as the genuinely open items.
RUBRIC_2: pass — Both due dates are included: 2026-05-02 for reporting exports and 2026-04-15 for team billing.
VERDICT: PASS
```


### `q17` &middot; staleness

**Q:** Are there any open action items owed to Jamie?

**Expected:**
Yes, one. The Q2 OKR doc. Originally committed at the 2026-03-18 investor check-in with a first-week-of-April due date, re-committed on 2026-04-17 to ship 2026-04-18.

**Rubric:**
  - [ ] Names the Q2 OKR doc
  - [ ] Notes the original 2026-03-18 commitment and/or the 2026-04-17 recommit

**Response:**
> Yes, two open action items are owed to Jamie, both assigned to Mat:
> 
> - **2026-03-18-investor-checkin-jamie.md** — "Write Q2 OKR doc and share with Jamie", due 2026-04-04, status: open, `promised_to: jamie`. **Overdue** (today is 2026-04-20).
> - **2026-04-17-product-prioritization.md** — "Finalize Q2 OKR doc reflecting these decisions, send to Jamie", due 2026-04-18, status: open. Also past due by 2 days.
> 
> These appear to be the same underlying commitment — the April 17 item is a refreshed deadline on the original March 18 promise, which was missed.

**LLM pre-grade:**
```
RUBRIC_1: pass — The agent explicitly names the Q2 OKR doc in both action items.
RUBRIC_2: pass — The agent cites both the 2026-03-18 original commitment and the 2026-04-17 recommit.
VERDICT: PASS
```


### `q18` &middot; cross_meeting_inference

**Q:** Why is the reporting-exports project happening now?

**Expected:**
Three-meeting arc. Riley raised reporting exports as a new pain point at the 2026-03-30 Northwind status call. Priya reinforced it as a live retention driver at the 2026-04-14 CS review (Riley's attention had shifted from SSO to exports). Mat then brought a scoped proposal to the 2026-04-17 product prioritization meeting, where the team approved a narrow date-ranged-CSV fix to ship by 2026-05-02.

**Rubric:**
  - [ ] Traces the chain through all three meetings (2026-03-30, 2026-04-14, 2026-04-17)
  - [ ] Names Riley or Northwind as the origin
  - [ ] Notes Priya's role in flagging it as a retention risk

**Response:**
> Scoped reporting-exports is happening now because it's a retention fix for an at-risk customer, not a roadmap item.
> 
> - **2026-03-30** (Northwind follow-up): Once the SSO issue was resolved, Riley immediately flagged exports as the next blocker — "CSV only, no date range, everything or nothing."
> - **2026-04-14** (CS review with Priya): Northwind escalated to elevated-risk; Riley went around Mat directly to Priya. Reporting exports became Northwind's live pain point, with 2–3 other accounts expected to ask.
> - **2026-04-17** (product prioritization): Team killed advanced analytics and green-lit a tightly scoped reporting-exports fix — date-ranged CSV, no dashboard, two weeks of eng — specifically to address Northwind plus the accounts Priya/Jordan anticipate. Alex ships by 2026-05-02; Priya tells Riley.
> 
> So: customer-driven, churn-risk triggered, deliberately scoped small to fit inside the Q2 team-billing commitment.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer explicitly traces the chain through all three meetings (2026-03-30, 2026-04-14, 2026-04-17).
RUBRIC_2: pass — Riley and Northwind are named as the origin at the 2026-03-30 meeting.
RUBRIC_3: pass — Priya's role in the CS review escalation (elevated-risk, retention) is noted.
VERDICT: PASS
```


### `q19` &middot; cross_meeting_inference

**Q:** What is the overall arc of the Northwind relationship across the corpus?

**Expected:**
Four-phase arc. (1) 2026-03-04: hot and angry. Riley cited SSO friction and said the word churn twice. Mat personally committed to a 2026-03-31 fix. (2) 2026-03-30: partially resolved. SSO slipped 3 days. Mat called honestly. Riley annoyed but accepting. New pain point surfaced: reporting exports. (3) 2026-04-14: SSO delivered but reporting concerns remain. Relationship ownership transferred from Mat to Priya. (4) 2026-04-17: reporting-exports fix scoped for 2026-05-02. Account is still at-risk but with active remediation owned by Priya.

**Rubric:**
  - [ ] Identifies at least 3 distinct phases
  - [ ] Cites the SSO arc and the exports arc
  - [ ] Notes the relationship-ownership transfer
  - [ ] States current status is at-risk-with-active-remediation (or equivalent)

**Response:**
> ## Northwind relationship arc
> 
> **2026-03-04 — First contact / crisis** (`2026-03-04-northwind-customer-call.md`): Riley B. (Director of Ops) says "churn" twice over SSO nested-Okta-groups onboarding failures. Mat makes a personal Q1 commitment to ship by March 31.
> 
> **2026-03-11 — Internal response** (`2026-03-11-eng-standup.md`): Alex takes SSO nested-groups, internal target March 25 for a one-week cushion.
> 
> **2026-03-30 — Missed deadline, managed well** (`2026-03-30-northwind-status-followup.md`): Mat proactively calls per his promise. Ships April 3, three days late. Riley is annoyed but retained; raises a new complaint about reporting exports (CSV-only, no date range).
> 
> **2026-04-07 — SSO closed out** (`2026-04-07-eng-standup.md`): SSO shipped April 3, Riley confirmed. Mat queues reporting-exports for the April 17 prioritization meeting.
> 
> **2026-04-14 — Relationship handoff** (`2026-04-14-cs-review-priya.md`): Riley has started going directly to Priya. Mat formally transfers Northwind ownership to Priya; he drops to escalation-only. Northwind flagged as one of three at-risk accounts.
> 
> **2026-04-17 — Structural fix** (`2026-04-17-product-prioritization.md`): Scoped reporting-exports (date-ranged CSV, no dashboard) greenlit for May 2. Priya to tell Riley the next day.
> 
> **Overall shape**: acute SSO crisis → visibly managed slip → account saved but escalated to CS → relationship matures from founder-owned to CS-owned, with the second Northwind pain point (exports) absorbed into the Q2 roadmap.

**LLM pre-grade:**
```
RUBRIC_1: pass — The answer identifies six distinct phases (March 4, March 11, March 30, April 7, April 14, April 17), well exceeding the minimum of three.
RUBRIC_2: pass — The answer explicitly traces the SSO arc from crisis through ship date and the exports arc from new complaint through scoped May 2 fix.
RUBRIC_3: pass — The April 14 entry explicitly states "Mat formally transfers Northwind ownership to Priya."
RUBRIC_4: pass — The answer notes Northwind is "flagged as one of three at-risk accounts" with the exports fix "greenlit for May 2," equivalent to at-risk-with-active-remediation.
VERDICT: PASS
```


### `q20` &middot; cross_meeting_inference

**Q:** What is the company's hiring plan for Q2 2026?

**Expected:**
No net new hires. Per the 2026-03-18 investor check-in, Jamie explicitly endorsed a zero-new-hires Q2 on the grounds that the team needs to gel around what's already shipped, and Mat confirmed that plan. Reinforced in Mat's 2026-04-10 voice memo ("no new hires").

**Rubric:**
  - [ ] Answers zero/no new hires
  - [ ] Cites Jamie's input from 2026-03-18
  - [ ] Cites Mat's reinforcement in the 2026-04-10 voice memo

**Response:**
> Based on the corpus:
> 
> **Q2 2026 hiring plan: no net new hires.**
> 
> - **2026-03-18 investor-checkin-jamie**: "headcount plan for Q2 is the next ask. No net new hires proposed before we hit the current team's stride." Runway is "Fourteen months at current burn. Zero new hires baked in." Jamie's guidance: "Don't force hiring. Let the team gel on what's shipped before you add load."
> - **2026-04-10 voice-memo-q2-priorities**: Explicitly NOT doing: "no new hires, no platform rewrites, no analytics product bets."
> - **Open question (not a decision)**: A CS-ops hire is floated as one of three possible shapes for the retention problem, pending Priya's triage (2026-04-10). No commitment made, and the 2026-04-14 CS review with Priya contains no hiring discussion.
> - **2026-04-17 product-prioritization** finalizes the Q2 cut lines (kill analytics, keep team billing, scoped exports) but does not revisit hiring.
> 
> Plan of record: zero hires in Q2, with a possible CS-ops exception still gated on Priya's retention triage.

**LLM pre-grade:**
```
RUBRIC_1: pass — The agent explicitly states "Q2 2026 hiring plan: no net new hires."
RUBRIC_2: pass — The agent cites Jamie's 2026-03-18 investor check-in with direct quotes about no new hires and letting the team gel.
RUBRIC_3: pass — The agent cites the 2026-04-10 voice memo with the explicit "no new hires" reinforcement.
VERDICT: PASS
```


---

## Human sign-off

For each rubric bullet above, tick the box when satisfied.
A question passes only if every rubric bullet is ticked.

Redirect this output to `docs/eval/results-v0.1.md` once reviewed.
