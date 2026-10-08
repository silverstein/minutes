# A 60-second sourced memory demo

**Audience:** people using AI assistants who want conversations to become
durable, useful memory. **Asset:** a screen recording using public synthetic
meetings. **Honest hook:** “Minutes remembers when the decision changed.”

| Time | Screen / narration |
| --- | --- |
| 0–10 s | Show the February 28 sample meeting: launch monthly billing for the next three consultant signups. “We made a decision in February.” |
| 10–20 s | Show the March 25 follow-up: annual-only, explicitly reversing the earlier decision. “Then we changed our minds.” |
| 20–40 s | Ask: “What did we decide about monthly billing, and did that decision stick?” Wait for the completed answer. Show both dated citations. |
| 40–50 s | Open each source. Show the exact statements that support the answer. |
| 50–60 s | “Free, open-source conversation memory. Local records. Ready for the AI you use.” Show the repo and local plugin install commands. |

The evidence supports this answer: monthly billing was approved as a narrow
experiment on February 28. On March 25 it was reversed to annual-only after
four signups fell below the threshold of twelve and churn looked worse. The
February decision did not remain current. This is an expected answer for review,
not itself a live model result. The separately preserved
[actual September 30 model answer](live-sample-answer-2026-09-30.md) now confirms
this behavior on the user-selected second account. It has been manually checked
against both source records; refresh and logout remain separate qualification gates.

If recording with the ChatGPT-plan prototype, show the consent screen and model
choice only after live qualification. Keep tokens, callback query strings,
account identifiers and unrelated meetings out of the recording. A prepared
illustration must retain its “Illustrated sample answer” label.

The source records are owned by this repository and marked `minutes_demo:true`:
[February decision](../../crates/mcp/fixtures/demo/2026-02-28-pricing-strategy.md)
and [March reversal](../../crates/mcp/fixtures/demo/2026-03-25-pricing-reversal.md).

Judge the exploration by completed installs, first successful sourced answers,
repeat use, and attributable repository visits/stars. Views measure attention;
they do not establish adoption. Do not claim ChatGPT directory publication,
native desktop OAuth support, or successful live sign-in from a unit-test receipt.
