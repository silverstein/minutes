# ChatGPT-plan qualification prototype

This standalone Unix/Node prototype qualifies OpenAI's OSS sign-in flow with
Minutes' public sample meetings. It does not activate a provider in Minutes or
change the desktop app. It requires Node 22 or later and a browser on the
computer running the loopback callback.

```bash
cd integrations/openai-plan
npm ci --ignore-scripts
node cli.mjs login
node cli.mjs status
node cli.mjs models
# Choose an exact slug returned for the signed-in account:
node cli.mjs demo --model <account-model-slug>
node cli.mjs logout
```

Open the local URL printed by `login` and choose **Continue with ChatGPT**. The
prototype requests identity, renewable access and ChatGPT-plan permission. The
user completes OpenAI's consent screen. An API key or client secret is not
required. Inference uses the user's ChatGPT allowance; inspect limits in
[ChatGPT usage settings](https://chatgpt.com/settings/usage). Model availability
and eligibility depend on the account and preview access.

Only two shipped synthetic records are sent: the February 28 monthly-billing
decision and March 25 reversal under `crates/mcp/fixtures/demo/`. The question
asks what was decided and whether it stuck, with both filenames and dates as
citations. `--question` accepts another question about those same records. It
cannot load the user's corpus or arbitrary files.

Credentials are stored in `~/.config/minutes/openai-plan-prototype/accounts.json`
with mode 0600 in a 0700 directory. This is deliberately separate from existing
Minutes settings and Codex credentials. Each verified account keeps its own
issued client ID and identity; the host ID survives restarts. Use `login
--account <issued-client-id>` to reauthorize a known registration and `use
<issued-client-id>` to select another signed-in account. `status` prints safe
metadata; do not print or attach the credential file.

The callback binds to `127.0.0.1`, validates state and PKCE, and verifies the
ID-token signature, issuer, audience, expiry, nonce and returning identity.
Granted plan scope gates inference. Refreshes serialize behind a credential
lock and save the replacement token atomically. An existing lock fails closed;
after a killed process, confirm no prototype process is running before manually
removing its `session.lock`. A normal cancellation releases it.

Model discovery comes from the account's catalog. Requests use `store:false`
and `stream:true`; an answer is shown only after `response.completed`. A late
quota failure or an interrupted stream is an error. There are no automatic
retries, provider substitutions, hosted tools, recording or transcription calls.
The live endpoint can omit `Content-Type`; the client still requires valid SSE
frames and explicit completion when that header is absent. An incompatible
declared MIME type is rejected. If the terminal output list is empty, the client
uses completed assistant messages from `response.output_item.done`, in output
order. Deltas alone cannot establish an answer; failed or incomplete responses
still discard all buffered output. Populated terminal output is authoritative.
After verified sign-in, the success page replaces
the callback URL with `/auth/connected`, removing authorization parameters from
the visible URL and that history entry.
Logout attempts renewable-session revocation and always clears local tokens;
the CLI reports explicitly if remote revocation was not confirmed.

For a remote computer, use a fixed callback port and forward it before opening
the browser, for example `login --port 18765` plus `ssh -L
18765:127.0.0.1:18765 <remote-host>`. Keep the callback hostname `127.0.0.1`.

```bash
npm test
```

The tests use synthetic tokens and transport responses. They verify protocol
boundaries, storage permissions, refresh rotation, cancellation and incomplete
streams. Passing them does not prove account eligibility or a live response.
Current evidence is in [qualification.md](../openai-plugin/qualification.md).
The September 30 live tests verified sign-in, persisted-session reuse and model
discovery. The first account returned a usage-limit error, preserved in its own
receipt. At the user's explicit choice, a second account completed the same
GPT-6-Astra sample after the parser fix. The answer cites both dated source files,
correctly identifies the reversal and does not claim an open task was completed.
The separate [second-account receipt](../openai-plugin/live-qualification-second-account-2026-09-30.json)
and [actual answer](../openai-plugin/live-sample-answer-2026-09-30.md) preserve that
success without relabeling the first account's quota failure. Both connections
remain stored. Near-expiry refresh and logout/revocation remain unqualified.

Sources checked September 30, 2026:

- [OSS ChatGPT-plan usage](https://developers.openai.com/siwc/token-sharing-open-source)
- [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
- [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
- [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)
