# Recall tabs (issue #1349, design D)

Recall can keep four independent interactive agent processes running. Selecting
another meeting selects that meeting's existing tab or creates one; a new chat
always creates a separate tab. The live call is ordered first. Tab and meeting-row
status render from the same metadata snapshot.

## Workspace and migration

`~/.minutes/assistant/recall-tabs.json` holds tab order, active tab, meeting path,
Claude session ID, capture identity and exact processing job ID. Each tab owns
`tabs/<id>/`, including
`CLAUDE.md`, `AGENTS.md`, `CURRENT_MEETING.md`, `CURRENT_ARTIFACT.md`, skills and
focus notices. The parent instruction files are neutral so nested Claude sessions
cannot inherit another tab's live or stored meeting focus. Newly authored tab
artifacts also appear in Documents.

Closing a tab stops its process and removes its active metadata. Its workspace
remains because it can hold authored Documents files or custom skills/settings;
closing a conversation must not erase those files. Safe cleanup of unchanged
generated-only closed workspaces is a separate lifecycle follow-up.

The first load copies legacy singleton context into an `assistant` tab. Its
Claude conversation is located using the old project directory's most recent
transcript filename and modification time, without reading transcript contents or
moving user-owned Claude history. The saved desktop focus supplies the meeting
path. With `[assistant] agent = "claude"`, a saved Claude ID is passed through
`--resume`, and configured `agent_args` retain the user's choices. Claude searches
projects for an explicit ID. No unqualified `--continue` is shared across new tabs.
With a shell such as `agent = "zsh"`, restoration starts only that shell. The
in-panel **Resume previous conversation** action submits `claude --resume <id>`
only when clicked, preserving shell aliases. It refuses a nonempty/uncertain
input line, an active Claude session, or a terminal whose foreground process is
not the original shell. Unix foreground checks are required for this action;
other platforms can run the command manually. Nothing is typed on restore or tab
selection, and a Discuss/Prep starter is never pasted into an idle shell.

## Hooks and status

Managed Claude settings install SessionStart, UserPromptSubmit, PreToolUse,
PostToolUse, PermissionRequest, Notification, Stop, StopFailure and SessionEnd
command hooks. They invoke the installed app executable with `--recall-hook`
using Claude's direct argument-vector form: no shell, Python or Node dependency.
The app exits this helper path before GUI initialization. Both directly launched
Claude and Claude started manually inside a shell discover the hooks in the tab's
own `.claude/settings.json`. Existing custom settings/hooks are retained and
managed hooks are replaced idempotently; symlinked settings paths are refused.
Global Claude settings and trust decisions are untouched. User-specified CLI
`--settings` arguments are preserved, without duplicating managed hooks.

There is one git repository at the assistant root, with ordinary tab subfolders.
No per-tab git repository is needed for instruction discovery. This lets new tabs
inherit accepted parent trust rather than requesting it for every meeting.
`settings.json` is intentional: modern Claude reads project-local
`settings.local.json` at the repository root, but shared `settings.json` from the
starting directory. Native QA must verify this discovery and trust inheritance
with the installed Claude version; Minutes never preaccepts trust or adds bypass
flags. The first Recall workspace can still need the normal trust dialog.

Each hook writes only `tabs/<id>/.recall-state.json`, containing status and the
Claude session ID and whether Claude is active inside a shell. Prompt/tool work is working; permission/question/idle events
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

The optional observer reads existing capture status. After desktop capture is
preserved and queued, a separate Recall worker receives the exact producer and
processing job IDs; capture and Stop never wait for Recall locks or writes.
Only the matching live tab receives `.live-tab` and the
bounded live-transcript guidance. Prep and free-chat workspaces have no active
live section. Recording may precede finalized Markdown: a question in that live
tab can use its bounded reader guidance without reading unrelated meeting
history. A live tab is labelled Live call (or its existing capture title), with
no invented meeting path. A namespaced recording-owner/version or relay identity
is used when desktop context tracking is off; this does not enable desktop
context collection. On
completion the exact job output or capture's canonical Markdown link moves that
same tab and its per-tab focus into debrief, even after failed processing and
a later successful Retry, or when summarization changed the filename. A stopped
capture without a saved meeting remains a conversation rather than a debrief
of a nonexistent file.

Stored meeting/history materialization remains deferred until a real question
passes the running provider's authentication check. For shell tabs, only Claude's
UserPromptSubmit hook runs that gate: typing a shell launch command, restoring a
tab, or using an account slash command never materializes context. Removing
context marks it handled so a later prompt cannot silently add it back. Terminal input queues,
shadow lines, listeners, xterm buffers and context gates belong to their tab;
an in-flight question retains its originating tab across an asynchronous switch.
A deleted meeting restores as chat with a visible note and cleared stored focus.

## UI and qualification

Free chats have stable numbered names, including an upgrade of existing generic
Chat labels. The cap chooser uses the same name and current shortcut position,
refreshes its choices on metadata changes, excludes live tabs, and dismisses
when its pending live capture is admitted. Admission by capture ID is idempotent
if closing a tab races the observer; it never opens a guessed meeting path.
New Chat reuses a new, known-untouched shell tab in the current app session; any
human input, saved Claude session, live binding, draft or uncertain line excludes
it. Restored shells are not assumed to be empty.

Meeting labels use the app's existing metadata inventory, and the selected title
and prep/debrief phase are persisted with the tab. Restore and generic spawn
events keep that focus in the header. Resolving the label reads no new meeting
body and does not bypass deferred context materialization.

Each terminal tracks the grid last acknowledged by its own PTY. Activation,
reveal, zoom and container resizing fit xterm and synchronize that grid, even if
xterm was already fitted. Per-tab IPC ordering prevents an old resize from
overwriting a newer one, and equal acknowledged grids avoid unnecessary redraws.
Activation reveals even an idle terminal before fitting, without waiting for
child output. A visible fit repaints terminal rows after a notice changes height.
Tab shortcuts are suppressed at xterm's own keyboard boundary so they cannot
contaminate the human-input draft used by explicit resume.
The rendered regression switches three real xterms initially backed by 10-column
PTY doubles and checks their grids after each selection.

The native File Close action routes Cmd/Ctrl+W to Recall before the OS can hide
the window; the window close button keeps its existing behavior. Cmd+1–4,
Ctrl+Tab, Cmd+T and Cmd+W operate within Recall; Windows/Linux use Ctrl
for the Cmd shortcuts. Tab buttons also support arrows, Home and End. At the cap,
an in-panel chooser offers finished, non-live tabs. Closing work, a live tab, an
active Claude process or any tab with a saved conversation ID requires an
in-panel confirmation. Closing the active tab selects its neighbour.
Background output changes no panel visibility
or keyboard focus. Status uses tokens, capture blue only for live, red for errors,
and a static dotted working glyph under reduced motion. System notifications are
omitted; background status is visible in the strip and meeting list.

The real Unix PTY foreground regression shares the production identity predicate
and runs in a fresh integration-test process with a 20-second parent deadline.
It does not inherit the desktop unit suite's native runtimes or background threads.
Native QA still verifies shell operation inside the actual desktop process.

Linux Rust execution and browser contracts do not prove signed Mac behavior.
The PR remains unmerged for t17 to install its exact head in Minutes Dev and test
a live call plus two prep sessions, shortcuts, matching background status,
quit/relaunch conversation restoration and the live-to-debrief transition.

Protocol references: [Claude hook input and exec form](https://code.claude.com/docs/en/hooks),
[explicit cross-project session resume](https://code.claude.com/docs/en/cli-reference),
[settings discovery](https://code.claude.com/docs/en/settings),
[repository trust inheritance](https://code.claude.com/docs/en/permissions).
