# Minutes plugin for ChatGPT desktop

This local plugin packages all 25 canonical Minutes skills and connects the
existing 34-tool Minutes MCP server. It includes recording, live transcripts,
copilot, dictation, conversation search, meeting preparation, notes, summaries,
commitments, and follow-up. The generated package is at
`.agents/plugins/minutes`; its version is 0.2.3 and its MCP runtime is pinned to
`minutes-mcp@0.28.0`.

ChatGPT Work on desktop can load a local marketplace and run a stdio MCP server.
The local Minutes engine performs capture and audio processing. Dictation
currently uses the CLI; delegation to the polished native desktop dictation
surface remains a separate integration gate. Installing the
plugin does not embed the Minutes desktop UI into ChatGPT, grant macOS
permissions, or make desktop capture available in a web browser. Hosted
ChatGPT Work, web/mobile access and public directory submission need a separate
remote connection and qualification.

## Install locally

Requirements: Node 22 or later, npm, a Codex CLI with plugin marketplace support,
and an installed Minutes CLI. The MCP server rejects engines older than 0.25.0.
Use the released Minutes 0.28.0 engine for current qualification. Historical
receipts used older engines; they do not establish current ChatGPT conversation
or hardware acceptance.

From a checkout containing this package:

```bash
node integrations/openai-plugin/install-local.mjs
```

For the first retrieval test, use the isolated public sample library:

```bash
node integrations/openai-plugin/install-local.mjs --sample
```

This copies five synthetic meetings into the private installation and points
only this plugin at its own library and configuration. It preserves the user's
normal Minutes profile. Ask ChatGPT: **“Using Minutes, what changed about pricing
between February and March? Cite both meetings and distinguish the decision from
unfinished work.”** An outside-library request must remain denied.

After the sample test, run the installer again without `--sample` to connect the
normal configured library. Use the same `--parent` as the original installation
when it was installed under a custom parent. The owned upgrade preserves the
previous directory. Start a fresh ChatGPT conversation after reloading the plugin
so the sample and normal-library tool contexts are not confused.

The plugin uses the existing ChatGPT session. The separate
[ChatGPT-plan OAuth experiment](../openai-plan/README.md) is not a prerequisite.

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

The portable source package contains `plugin.json` and `mcp.json`. ChatGPT
26.928.31416 with bundled Codex 0.159.2 displays its skills but ignores the
portable MCP component. For desktop installation, the installer selects the
generated `.codex-plugin/plugin.json` compatibility entrypoint and preserves
the portable manifest as `portable-plugin.json` outside the installed plugin
root. Both layouts use the same MCP file. The source package remains portable.

An upgrade replaces only the registration of a previous private Minutes
installation created by this installer under the same parent. It retains the
old directory and restores its registration if the new registration fails.
An unrelated or linked marketplace is preserved and the installer stops with
an explanation. Other marketplaces and active sessions are preserved.

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

First check discovery through the actual desktop runtime:

```bash
node integrations/openai-plugin/qualify-host.mjs --codex /absolute/path/to/codex
```

On macOS, use ChatGPT's bundled executable when qualifying ChatGPT itself:
`/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex`.
This check requires exactly one `minutes@minutes` server with all 34 canonical
tools. It makes no model calls and invokes no meeting or capture tools. A
direct MCP handshake can pass while the desktop runtime ignores the server;
this check catches that failure. In-app execution still needs a separate
ChatGPT conversation test after the plugin has reloaded.

```bash
cd integrations/openai-plugin
npm ci --ignore-scripts
npm run qualify
# On Linux, isolate a separately downloaded, checksum-verified CLI:
MINUTES_QUALIFICATION_BIN=/absolute/path/to/minutes-v0.28.0 npm run qualify
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
[October 3 discovery record](desktop-discovery-qualification-2026-10-03.json)
records the failed in-app test, isolated manifest comparison, repair and
successful desktop runtime discovery. The
[September 30 record](qualification.md) covers the earlier five-skill prototype
and the separate ChatGPT-plan sign-in/sample-inference experiment.

Canonical skills remain under `tooling/skills/sources`. Regenerate and verify:

```bash
cd tooling/skills
npm run build
npm run compile
npm run ci
```

Official sources checked October 7, 2026:
[local plugin installation](https://developers.openai.com/plugins/build/plugins),
[ChatGPT Work MCP support](https://learn.chatgpt.com/docs/extend/mcp), and
[public submission requirements](https://developers.openai.com/plugins/deploy/submission).
