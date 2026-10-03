# Minutes OpenAI preview: one demo, one try-it path

Prepared September 30, 2026. Owner: Mat for recording and publication; Codex for
source, instructions and evidence. Tracking: `minutes-rt6k.4`. This is a draft
launch kit. No post, directory submission, merge, deployment or recorded clip is
claimed by this document.

The first audience is developers and heavy AI users curious about OpenAI's new
ChatGPT-plan sign-in support. Mat's stated goal is attention and distribution for
free, open-source Minutes. We have one successful sample response, not evidence
of adoption. The question to test is whether people can reproduce it and then
want conversation memory in their usual AI workflow.

## The story

A team approved monthly billing. A later conversation reversed it. Ask what
happened, and Minutes' sample prototype returns the current decision with both
dated sources. The hook is: **Minutes remembers when the decision changed.**

The proven path is a standalone, local Node prototype using the user's ChatGPT
allowance and two synthetic meetings. It does not require an API key. The Codex
plugin is a separate local installation connecting to Minutes' MCP server; it
does not acquire inference credentials from the prototype. Avoid presenting the
two as an integrated desktop flow.

## Record a 60–75 second clip

Use the actual prototype on silverbook. Keep the synthetic-record disclosure
visible. Do one full rehearsal before calling the clip ready. The saved result
is a fallback, labeled as a September 30 captured result; do not animate it as a
new response. There is no recording in this kit yet.

| Time | Show | Say |
| --- | --- | --- |
| 0–8 seconds | The completed answer's first sentence, large enough to read | “We approved monthly billing. Then we changed our minds. Can our AI keep track?” |
| 8–20 seconds | February sample and then March sample; highlight the two decision statements | “These are sample meetings. February says to test monthly for three consultants. March explicitly reverses it.” |
| 20–45 seconds | Run the actual sample command and show the completed answer with both exact filenames and dates | “I connected this Minutes prototype with my ChatGPT plan and asked what happened. It finds the reversal and shows where it came from.” |
| 45–58 seconds | The open follow-up task in the March source and the answer's caveat | “It also keeps the decision separate from the work. Removing the pricing page was still an open task.” |
| 58–75 seconds | Preview instructions and repository | “Minutes is free and open source. This is an early local preview. Try the two sample meetings and tell me where setup gets stuck.” |

Start with the already captured answer as an explicitly labeled result, then show
a fresh command if the account is available. A fresh model request consumes plan
allowance. If it fails, retain the failure and use the labeled capture; do not
silently retry, switch accounts or models, or fabricate typing. If waiting is
edited out of the clip, disclose that waiting was shortened.

The actual command on the already connected silverbook copy is:

```bash
cd /Users/silverbook/Sites/minutes-openai-qualification.4FCLga/integrations/openai-plan
/Users/silverbook/.local/bin/node cli.mjs demo --model gpt-6-astra
```

Use large terminal text and a clean window. Keep account email, registration IDs,
authorization URLs, tokens and unrelated windows out of the recording. The two
sources are under `crates/mcp/fixtures/demo/`. The captured answer is
[live-sample-answer-2026-09-30.md](live-sample-answer-2026-09-30.md).

## Give viewers one sample-only try-it link

During the pilot, link directly to this file on the preview branch:

https://github.com/silverstein/minutes/blob/feat/openai-distribution/integrations/openai-plugin/launch.md

This is source installation for macOS/Linux builders with Node 22 or later, not
a one-click desktop release. The branch is the draft in
[PR #1069](https://github.com/silverstein/minutes/pull/1069), separate from main.
Use a fresh directory so an existing Minutes checkout is preserved:

```bash
git clone --branch feat/openai-distribution --single-branch https://github.com/silverstein/minutes.git minutes-openai-preview
cd minutes-openai-preview/integrations/openai-plan
npm ci --ignore-scripts
node cli.mjs login
```

Open the printed loopback URL on that same computer, choose Continue with
ChatGPT, and complete consent. The prototype keeps its credentials separately
from Minutes and Codex. Then inspect the account's available models:

```bash
node cli.mjs models
```

Choose an exact slug from that output. Replace `MODEL_FROM_YOUR_CATALOG` below
with that slug; `gpt-6-astra` worked on the qualified account but availability is
account-specific:

```bash
node cli.mjs demo --model MODEL_FROM_YOUR_CATALOG
```

Expect a sourced answer identifying the February experiment and March reversal.
Only the two public synthetic records are sent to OpenAI. They use your ChatGPT
allowance; API credits are separate. If a usage-limit error appears, stop new
inference requests and inspect ChatGPT usage settings. Report the failure rather
than treating another account's result as the same test. Preview eligibility,
limits and model choices vary by account.

When finished, use `node cli.mjs logout`. It clears the local prototype tokens
and attempts remote revocation, reporting whether that was confirmed. Remote
revocation and near-expiry refresh have not yet been live-qualified; do not
present this as a broadly released native connection. See
[prototype details](../openai-plan/README.md) and
[qualification evidence](qualification.md).

After someone completes the sample and wants to use their own local Minutes
library, give them the separate [Codex plugin instructions](README.md). That
path additionally requires Codex plugin support and a compatible Minutes CLI.
Retrieved meeting text becomes context for the AI host under its settings.

## Draft the first post

Pair this text with the recorded clip and the single preview link above:

> A decision in February. The opposite decision in March.
>
> I connected a Minutes prototype to my ChatGPT plan and asked what happened.
> It found the reversal, cited both meetings, and kept an unfinished task
> separate from the decision.
>
> Minutes is free and open source. This early local preview uses two synthetic
> meetings and needs no API key. I'd love a few people to try setup and tell me
> where it gets stuck.
>
> Try the sample: [preview instructions](https://github.com/silverstein/minutes/blob/feat/openai-distribution/integrations/openai-plugin/launch.md)

Suggested first placements: Mat's own X account for the clip, and a GitHub
Discussion in Minutes for the reproducible instructions and setup feedback.
These are proposed placements, not posts already sent. Answer questions in the
thread; keep the first invitation focused on reproducing this one result.

## Run a small seven-day adoption test

Start with five to ten willing testers. Record observations in the existing
Beads lane, without names, transcripts or credentials: people who attempted
setup, people who completed a cited answer, time to first answer, blocking
errors, people who wanted to connect their own Minutes library, and people who
used it again. Ask each tester: “Did setup work, did the sources support the
answer, and what would you use this for next?”

Suggested decision rule, chosen for this pilot rather than an industry
benchmark: if at least three independent testers reproduce the answer and at
least two want to use their own library, continue improving onboarding and
qualify desktop integration. If attention is high but completions are low, fix
the observed setup failures before expanding the audience. If people complete
it but see no use, change the use case before investing in a hosted service.
Views and stars measure attention; repeat use is the stronger distribution
signal. None of these outcomes has been measured yet.

## Directory distribution is a later route

OpenAI supports skills-only plugins as well as plugins with MCP connections.
Our current package includes a local stdio MCP connection. Directory submission
for that connection requires hosted, reviewable MCP setup and verification;
it is a separate engineering and publication step. A skills-only package could
be assessed separately, but must be useful on its own, and the current submission
flow cannot add MCP to an existing skills-only plugin. Do not remove a needed
connection just to obtain a listing.

Sources checked September 30, 2026:
[ChatGPT-plan usage](https://developers.openai.com/siwc/token-sharing-open-source)
and [plugin submission](https://developers.openai.com/plugins/deploy/submission).
