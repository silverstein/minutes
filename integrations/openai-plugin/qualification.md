# OpenAI distribution qualification — September 30, 2026

This is the historical September 30 prototype record. The full 25-skill
package and Silverbook installation are recorded separately in
[full-plugin-qualification-2026-10-01.json](full-plugin-qualification-2026-10-01.json).

Owner: Codex working for Mat. Beads epic: `minutes-rt6k`; prototype tasks
`minutes-rt6k.1` and `.2`; live acceptance task `.3`.

Owned checkout: `/home/mat/Sites/minutes-worktrees/openai-distribution`, branch
`feat/openai-distribution`, from `origin/main` at
`5fbe77b6d1f406286e8fa985deb65a9d6affdea0`. Purpose: the first bounded OSS
ChatGPT-plan/plugin distribution tranche. The checkout is retained for review
and live qualification. The dirty canonical checkout was preserved.

| Proof | September 30 evidence |
| --- | --- |
| OAuth source/protocol | Standalone prototype; 29 tests pass on Linux and silverbook/macOS. Test fixtures resolve macOS temporary-directory symlinks; production symlink rejection is unchanged. Tokens, transport responses and signing keys in tests are synthetic. Missing MIME headers and empty terminal output still require valid SSE and confirmed completion; finished assistant messages are released only after `response.completed`; the success page removes callback query parameters with a CSP-authorized script. |
| Skill source/compiler | Five canonical skills packaged; 36 compiler tests pass, with routing, resolver, ownership, generated-output and golden checks. |
| Codex discovery | CLI 0.159.1 lists `minutes@minutes` v0.1.0 as available using a local marketplace config override. |
| Codex installation | `plugin add` succeeds and `plugin list` shows installed/enabled in an isolated bubblewrap profile. The user's normal Codex profile was not enabled. |
| Published MCP runtime | `minutes-mcp@0.27.0` plus checksum-verified released Linux CLI v0.27.0 passes a real stdio sample-corpus check: five meetings, two pricing sources, current reversal, outside-corpus denial. No model calls or real meeting reads. |
| Existing Silvercloud profile | Initial MCP qualification fails closed on installed CLI v0.18.0. A v0.27.0 child using existing host state then reports unconfirmed legacy QMD cleanup. No host engine upgrade or QMD repair was performed. |
| Sample sandbox | Qualification overlays separate sample config/state and released CLI only inside the child filesystem. This is an isolated sample-install receipt, not proof the existing host profile's QMD issue is resolved. |
| Browser sign-in | Verified on silverbook for both user-selected accounts. The prototype validated signed ID tokens and granted ChatGPT-plan scope. The active second account matches the email Mat requested; the first registration is preserved. No credentials or callback URLs are included in receipts. |
| Live model catalog / restart | A separate CLI process loaded persisted credentials and listed five account-visible models. The account-specific catalog request succeeded. |
| Live inference | First account: three attempts exposed an absent Content-Type header and then `subscription_sharing_usage_limit_exceeded`; inference stopped for that account. User-selected second account: two sample/diagnostic calls exposed finished-message events with empty terminal output; after fixing the parser, the third request returned a completed GPT-6-Astra answer. Manual review confirms both exact dated citations, the decision reversal and an open follow-up not claimed complete. No automatic retries or model/provider/key fallback. The first quota receipt remains unchanged; its scope/reset is unknown. |
| Callback rendering | Synthetic browser check verified the CSP-authorized URL replacement; Mat's second-account success screenshot also shows live `/auth/connected` without query parameters. |
| Refresh / logout | Not yet live-qualified. The token's reported earliest refresh point is later than this initial sample test; credentials are preserved. No premature refresh, revocation, or local logout was performed. |
| Attention asset | Prepared sample illustration and 60-second walkthrough. Browser rendering, both source disclosures, source link destinations and mobile width checked with agent-browser. The illustration explicitly labels its answer as prepared. No public posting or attention result. |
| Native desktop / public directory | Neither is activated by this tranche. This MCP-backed package needs a remote HTTPS endpoint and acceptance for directory submission; the local package does not meet that gate. Skills-only submissions are a separate supported format. |

The separate browser-qualification copy is
`/Users/silverbook/Sites/minutes-openai-qualification.4FCLga`; its credentials
stay in the documented protected prototype store. It uses Node 22.23.2
and loopback port 18765. Every attempt expires after ten minutes; after expiry,
start `login --port 18765` again and open the printed local URL on silverbook.
Do not collect tokens or callback URLs as evidence. User authorization to the
agent is recorded; no further permission request is needed for the sample demo.

The first-account quota receipt remains
[live-qualification-2026-09-30.json](live-qualification-2026-09-30.json).
The independent second-account success is recorded in
[live-qualification-second-account-2026-09-30.json](live-qualification-second-account-2026-09-30.json),
with the [actual model answer](live-sample-answer-2026-09-30.md). Each account
made three intentional sample/diagnostic calls; these were not automatic retries.
OpenAI's quota-error recovery is to stop new plan-inference requests and inspect
[ChatGPT Settings → Usage](https://chatgpt.com/settings/usage). Do not infer a
plan-wide exhaustion or a reset time from this code. See the official
[error guidance](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery).

Live acceptance requires verified sign-in, catalog discovery, one completed
sample response with correct dated citations, successful logout/revocation, and
restart/refresh behavior. Account eligibility and quota errors remain failures;
no provider/key substitution establishes this proof. Native meeting access
requires separate parity with Minutes' existing restricted-meeting and capture
isolation rules before desktop integration.

Public attention should be measured as attributable repository visits, completed
installs, first sourced answers and repeat use. No reach or adoption claim is
supported by the technical receipts above.
