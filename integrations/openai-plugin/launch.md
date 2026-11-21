# Try Minutes in ChatGPT desktop

This free, open-source local preview connects the full Minutes skill set and
MCP tools to ChatGPT Work on your computer. It is the draft in
[PR #1069](https://github.com/silverstein/minutes/pull/1069), separate from a public
directory listing. Native capture and insertion have their own acceptance gates.

## Current testing gate

The first October 7 test confirmed ChatGPT discovery and `get_status`, but
sourced retrieval failed with Minutes 0.28.0 because its parser rejected QMD's
complete one-line empty-registry response. Minutes 0.28.1 includes that fix.
Plugin preview 0.2.5 pins MCP 0.28.1. The subsequent
[sourced-answer test](chatgpt-sourced-answer-qualification-2026-10-07.json)
passed in actual ChatGPT Work with the published engine: both dated meetings
were cited, their decision reversal was preserved, and unfinished work was
not reported as complete. The normal library connection was restored afterward.
The historical [failure record](chatgpt-conversation-qualification-2026-10-07.json)
is retained. Capture, live and optional workflows still need separate testing.

## Install and test the connection

Use Node 22+, npm, a Codex CLI with plugin support, and Minutes 0.28.1 or later. The released
Mac desktop package includes a CLI:

```bash
brew install --cask silverstein/tap/minutes
```

Use a fresh directory to preserve existing checkouts:

```bash
git clone --branch feat/openai-distribution --single-branch https://github.com/silverstein/minutes.git minutes-chatgpt-preview
cd minutes-chatgpt-preview
node integrations/openai-plugin/install-local.mjs --sample
```

The sample install contains five public synthetic meetings and its own private
configuration. All sample state selectors point to this installation; unrelated marketplace
registrations are preserved. It uses a
pinned MCP runtime and disables automatic engine/model setup. If a previous
installation used a custom parent, supply that same `--parent`; see the
[installer guide](README.md).

Reload ChatGPT desktop when no work is active, enable Minutes in the local
marketplace if prompted, and start a new Work conversation. First ask:

> Check the Minutes connection using its get_status tool. Report whether a
> recording is in progress. Do not read meetings, change configuration,
> run setup or start recording.

Verify an actual tool call. Then ask:

> Using Minutes, what changed about pricing between February and March?
> Cite both meetings and distinguish the decision from unfinished work.

The February sample proposes a narrow monthly-billing experiment. The March
sample reverses it to annual-only billing. The answer should cite both dated
sources and keep unfinished follow-up work unfinished. If tools are unavailable,
report the failure; do not substitute a result from this guide or prior context.

This plugin uses the existing ChatGPT session. Normal host usage limits apply.
The separate [ChatGPT-plan OAuth experiment](../openai-plan/README.md) is not
required to install or use the plugin.

## Connect your normal library

After the sample works, run the same installer without `--sample`, retaining any
custom `--parent`. The owned sample installation stays available for inspection
and rollback. Reload the plugin and start a fresh conversation.

```bash
node integrations/openai-plugin/install-local.mjs
```

Ask about a relevant conversation, prepare for a meeting using prior sources,
or draft a follow-up. Records stay local; requested tool results become context
for ChatGPT under its settings. Restricted-meeting policies still apply. Verify
retrieval separately from microphone capture, live consumers and dictation.

## Demonstration and feedback

Show the February proposal, the March reversal and the actual ChatGPT tool calls
and sourced answer. Keep the synthetic-data disclosure visible. Use a fresh
successful run; label historical captures by date and disclose shortened waits.
Keep account details, credentials and unrelated windows out of the recording.

With the engine patch passing the real ChatGPT retrieval test, invite a few
willing testers with this one try-it link. Track installation
completion, time to first sourced answer, blocking errors and return use in the
existing Beads lane. Views and stars measure attention; repeat useful
conversations are evidence of adoption. No recorded clip, public post or measured
adoption is claimed by this guide.

## Public directory route

The full local package stays available while public distribution is assessed.
The ordinary MCP submission route requires stable public HTTPS; local file and
hardware workflows may need product-specific review. Secure MCP Tunnel supports
private testing, explicitly not public submission or distribution. Bundled skills
do not replace the required Minutes connection.

Sources checked October 7, 2026:
[local packages](https://developers.openai.com/plugins/build/plugins),
[local MCP submission](https://developers.openai.com/plugins/guides/submit-claude-plugin),
[public submission](https://developers.openai.com/plugins/deploy/submission),
[private tunnel boundary](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels).
