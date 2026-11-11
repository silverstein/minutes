# Recall tabs (issue #1349, design D)

Recall can keep four independent interactive agent processes running. Selecting
another meeting selects that meeting's existing tab or creates one; a new chat
always creates a separate tab. The live call is ordered first. Tab and meeting-row
status render from the same metadata snapshot.

## Workspace and migration

`~/.minutes/assistant/recall-tabs.json` holds tab order, active tab, meeting path,
Claude session ID and capture session ID. Each tab owns `tabs/<id>/`, including
`CLAUDE.md`, `AGENTS.md`, `CURRENT_MEETING.md`, `CURRENT_ARTIFACT.md`, skills and
focus notices. The parent instruction files are neutral so nested Claude sessions
cannot inherit another tab's live or stored meeting focus. Newly authored tab
artifacts also appear in Documents.

The first load copies legacy singleton context into an `assistant` tab. Its
Claude conversation is located using the old project directory's most recent
transcript filename and modification time, without reading transcript contents or
moving user-owned Claude history. The saved desktop focus supplies the meeting
path. A saved Claude ID is passed through `--resume`; Claude searches projects
for an explicit ID. No unqualified `--continue` is shared across new tabs.

## Hooks and status

Managed Claude settings install SessionStart, UserPromptSubmit, PreToolUse,
PostToolUse, PermissionRequest, Notification, Stop, StopFailure and SessionEnd
command hooks. They invoke the installed app executable with `--recall-hook`
using Claude's direct argument-vector form: no shell, Python or Node dependency.
The app exits this helper path before GUI initialization. User settings are
merged into a private settings file; neither global settings nor the user's
`.claude` directory is edited, and settings values are not placed in argv.

Each hook writes only `tabs/<id>/.recall-state.json`, containing status and the
Claude session ID. Prompt/tool work is working; permission/question/idle events
are waiting; Stop is done; StopFailure is error. A dedicated optional worker
polls these tiny files every 750 ms and emits a metadata snapshot only on change.
No terminal screen parsing determines status. On relaunch stale process status
and live markers are cleared; conversation IDs survive.

The native hook consumes each per-tab focus notice once on UserPromptSubmit,
preserving #1308: opening a document, dropping context or moving into debrief
never types an unsolicited turn into the agent's input. Configured non-Claude
terminals retain their existing interactive behavior; Claude hook status and
conversation resume are specific to Claude Code.

## Live call and isolation

The optional observer reads existing capture status; capture and Stop do not
call it or wait for it. Only the matching live tab receives `.live-tab` and the
bounded live-transcript guidance. Prep and free-chat workspaces have no active
live section. Recording may precede finalized Markdown: a question in that live
tab can use its bounded reader guidance without reading unrelated meeting
history. On completion the capture's canonical Markdown link moves the same tab
into debrief, even when summarization changed the filename.

Stored meeting/history materialization remains deferred until a real question
passes the running provider's authentication check. Terminal input queues,
shadow lines, listeners, xterm buffers and context gates belong to their tab;
an in-flight question retains its originating tab across an asynchronous switch.
A deleted meeting restores as chat with a visible note and cleared stored focus.

## UI and qualification

Cmd+1–4, Ctrl+Tab, Cmd+T and Cmd+W operate within Recall; Windows/Linux use Ctrl
for the Cmd shortcuts. Tab buttons also support arrows, Home and End. At the cap,
an in-panel chooser offers finished, non-live tabs. Closing work or a live tab
requires an in-panel confirmation. Background output changes no panel visibility
or keyboard focus. Status uses tokens, capture blue only for live, red for errors,
and a static dotted working glyph under reduced motion. System notifications are
omitted; background status is visible in the strip and meeting list.

Linux Rust execution and browser contracts do not prove signed Mac behavior.
The PR remains unmerged for t17 to install its exact head in Minutes Dev and test
a live call plus two prep sessions, shortcuts, matching background status,
quit/relaunch conversation restoration and the live-to-debrief transition.

Protocol references: [Claude hook input and exec form](https://code.claude.com/docs/en/hooks),
[explicit cross-project session resume](https://code.claude.com/docs/en/cli-reference).
