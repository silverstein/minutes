# Minutes plugin for ChatGPT desktop

This local plugin packages all 25 canonical Minutes skills and connects the
existing 34-tool Minutes MCP server. It includes recording, live transcripts,
copilot, dictation, conversation search, meeting preparation, notes, summaries,
commitments, and follow-up. The generated package is at
`.agents/plugins/minutes`; its version is 0.2.0 and its MCP runtime is pinned to
`minutes-mcp@0.27.1`.

ChatGPT Work on desktop can load a local marketplace and run a stdio MCP server.
The local Minutes engine performs capture and audio processing. Installing the
plugin does not embed the Minutes desktop UI into ChatGPT, grant macOS
permissions, or make desktop capture available in a web browser. Hosted
ChatGPT Work, web/mobile access and public directory submission need a separate
remote connection and qualification.

## Install locally

Requirements: Node 22 or later, npm, a Codex CLI with plugin marketplace support,
and an installed Minutes CLI. The MCP server rejects engines older than 0.25.0.
The sample qualification uses released CLI 0.27.1; the Silverbook status check
uses its existing CLI 0.27.0. Full capture acceptance is still pending.

From a checkout containing this package:

```bash
node integrations/openai-plugin/install-local.mjs
```

If GUI or non-interactive PATH differs, supply executable paths:

```bash
/absolute/path/to/node integrations/openai-plugin/install-local.mjs \
  --codex /absolute/path/to/codex --npm /absolute/path/to/npm
```

The installer copies the package into a private owned installation directory,
installs the pinned MCP dependencies, and uses an absolute Node entrypoint.
It backs up the existing local Codex and personal marketplace configuration
before registration, registers `minutes@minutes`, and enables it. It does not
read OpenAI credentials, call a model, read meeting contents, upgrade Minutes,
or grant OS permissions. Runtime auto-setup is disabled. Keep the installation
directory: its absolute paths are used by the plugin. Failed installations are
preserved for inspection.

Restart ChatGPT desktop after installation. In its plugin directory, select the
Minutes marketplace and install or enable Minutes if the app prompts for it.
Start with: **“Check the Minutes connection and status.”** Confirm the result
comes from the connected Minutes tool before trying a real-library search or
recording. A successful CLI installation is separate from in-app acceptance.

## Capability requirements

| Workflow | Local requirements |
| --- | --- |
| Search, prep, recap, debrief, weekly review, commitments and relationship context | A configured Minutes library; existing restricted-meeting rules apply. |
| Recording, call capture, live transcripts and copilot | Local Minutes engine and appropriate microphone/system-audio permissions. Recording remains independent of optional consumers. |
| Dictation | Local engine and the OS permissions required for insertion. |
| Ingest and audio/video review | Local file access and the skill's processing dependencies; some helpers require Python or FFmpeg. |
| Speaker confirmation | Human confirmation through the existing Minutes flow; uncertain identities stay unmerged. |
| X1 closeout and meeting send | A connected X1 account and the canonical skill's explicit approval before external writes or sending. |
| Release notes and repository verification | A relevant local repository and shell access. |

The plugin does not upload a library to a hosted Minutes service. Tool results
used as model context are shared with the AI host under its settings. The
canonical meeting-access, confirmation and external-send rules remain in each
packaged skill.

## Qualification

```bash
cd integrations/openai-plugin
npm ci --ignore-scripts
npm run qualify
# On Linux, isolate a separately downloaded, checksum-verified CLI:
MINUTES_QUALIFICATION_BIN=/absolute/path/to/minutes-v0.27.1 npm run qualify
```

This uses five public synthetic meetings, an isolated Minutes configuration,
and the actual published MCP package. It verifies all 34 advertised tools
against the repository manifest, searches the two dated pricing sources,
checks the decision reversal, and denies an outside file. It makes no model
calls and reads no real meeting contents. The Linux sandbox does not repair or
replace the host's existing Minutes profile.

The [October 1 qualification record](full-plugin-qualification-2026-10-01.json)
distinguishes compiler parity, sample retrieval, Silverbook installation and
live MCP status from the pending ChatGPT UI and hardware tests. The
[September 30 record](qualification.md) covers the earlier five-skill prototype
and the separate ChatGPT-plan sign-in/sample-inference experiment.

Canonical skills remain under `tooling/skills/sources`. Regenerate and verify:

```bash
cd tooling/skills
npm run build
npm run compile
npm run ci
```

Official sources checked October 1, 2026:
[local plugin installation](https://developers.openai.com/plugins/build/plugins),
[ChatGPT Work MCP support](https://learn.chatgpt.com/docs/extend/mcp), and
[public submission requirements](https://developers.openai.com/plugins/deploy/submission).
