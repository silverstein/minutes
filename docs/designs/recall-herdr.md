# Recall: Open in Herdr

`minutes-lrad.1` adds an optional local handoff at the desktop edge. Nothing in
minutes-core, capture, processing, or the portable palette enum calls Herdr.
`recall_herdr.rs` owns discovery, bounded CLI execution, transfer and recovery.
The executable/deadline boundary can be replaced by a fake `hc` in tests.
Recall's persisted tab gains only optional `herdr_terminal` and `handed_off_at`.
Old manifests deserialize both as null.

## Availability and configuration

The desktop reads this optional section from the existing Minutes config path
(including the existing config-path override):

```toml
[herdr]
enabled = "auto" # default; "off" disables discovery and new transfers
```

`auto` exposes the tab menu and palette command only when `hc` is on the augmented
assistant PATH (the same common install directories used by local PTYs, including
`~/.local/bin` and `/opt/homebrew/bin`) and `hc server status --json` returns
recognized server metadata.
This works with the minimal PATH supplied to Dock/Finder launches. The palette
renders ordinary commands first and appends the Herdr command asynchronously;
a hung discovery call cannot delay opening the list or executing ordinary
commands. Selecting Herdr performs one authoritative edge availability check.
No handoff happens automatically. Invalid configuration fails closed. A saved
external owner remains visible even when discovery fails or the switch is off;
Take back can still locate `hc` to reconcile that existing owner.
No machine, space, lane, credentials or provider settings are assumed. There is
no `--on` flag, and addresses containing `/` are rejected.

## Ownership and recovery

The menu and palette route to the same in-panel confirmation, initially focused
on Cancel. A Working tab refuses handoff with “Wait for this reply to finish,
then move it to Herdr”; the native backend checks again immediately before
writing its fence, so an idle-to-working transition during discovery cannot
silently terminate an answer. Opening the palette command does not itself move
a conversation.
A separate transition mutex serializes handoffs and returns a busy error rather
than holding the PTY manager lock across CLI calls. Normal tab closure refuses
to remove an externally owned tab; take it back first.

Before stopping the local process, Minutes writes a provisional local terminal
name and timestamp to the manifest. Every native spawn route checks this owner
before it creates or rewrites the workspace, and frontend restoration skips it.
This write-ahead fence also covers an app crash between launch and address save.
The tab's own PTY is removed, its foreground and original process groups are
stopped on Unix (including an agent launched inside a shell), and its child is
reaped before `hc run`. Windows uses a bounded process-tree termination first.
An unconfirmed local stop restores the original handle and does not call `hc`.
Every fallible operation after the fence is saved but before `hc run` is invoked
belongs to one recovery envelope: failure clears the persisted fence and starts
(or retains) the local tab. This includes metadata refresh and manager-lock
errors; rollback repairs the tab metadata before clearing a poisoned lock.
If persistence or local restart itself fails, that distinct recovery error is
shown instead of claiming success. Normal shutdown retains its existing asynchronous reaping behavior.

The launch uses the tab's own working directory and checks the same
`transcript_available` metadata as Recall. A matching nonempty Claude transcript
selects Claude with `--resume <id>`, including when the configured agent is a
shell and the user launched Claude inside it. Claude is resolved through the
shared binary resolver used by Recall's Claude tooling. Shell arguments (such
as `zsh -l`) are not passed to Claude. A configured Claude executable retains its
filtered `agent_args`, skill arguments and session-argument normalization. With
no resumable Claude transcript, the configured agent and its arguments are used;
a never-used Claude tab starts fresh. The binary is resolved before stopping
the local process, then transcript metadata and launch selection are refreshed
after the child exits so a final saved turn can be recovered. Normal shell
restoration after Take back retains its explicit Resume action.

`hc run --json --bg --no-parent --outlive-parent` receives the `minutes.tab` label
and, when there is a meeting, its filename stem as `minutes.meeting`. The default
name is `minutes-<tab-id>`. Long IDs use a bounded hash; if an earlier finished
terminal already has that name, a fresh bounded name avoids confusing an old
finished record with a delayed new launch. These are lookup names, not remote
addresses. The authoritative address from `run` is saved on success.

The server's launch argv contains `env -u CLAUDECODE -u
CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PID`, followed by
the local PTY's discovery environment and the agent's argument vector. This
clears server-inherited nested-Claude markers as well as clearing them on the
`hc` client. No shell joins or evaluates the agent arguments.

Every `hc` invocation has a five-second process deadline and runs on a blocking
worker, outside capture. The client is killed and reaped on timeout. Output is
read from anonymous files rather than blocking pipe readers; JSON parsing is
limited to 64 KiB per selected stream. Successful JSON comes from stdout. On a
nonzero exit with empty stdout, error JSON is read from stderr with the same
bound, matching hc 0.1.21. Failed-command JSON retains only `error.kind`, from
either stream; messages, hints and arbitrary stderr are never displayed. Invalid
or oversized errors remain ambiguous failures. No provider login or permissions
are changed.

A rejected launch is reconciled by name. Confirmed absence/finished state after
a definitive rejection clears the fence, restarts the local tab with its previous
theme and shows the error. A recovered live terminal remains the owner. An
unanswered, malformed or timed-out request is ambiguous: it may have reached the
server, so Minutes keeps the fence and shows the error instead of starting a
second writer. This is a deliberate safety refinement of the bead's unconditional
restart-on-failure wording. Take back requires a recognized finished record or
an explicit End terminal and take back action followed by confirmed exit.
Unknown state, failed kill and server failure do not prove exit. A recognized
`not_found` returns `RECALL_HERDR_MISSING_CONFIRM`, then asks: “Herdr has no record
of this terminal. If it isn't running anywhere, take the conversation back?”
Cancel retains the fence. Explicit Take conversation back is a distinct approval
from End terminal: it clears the owner/timestamp and restarts locally. The backend
rechecks before acting; if the terminal has become live, the missing-owner approval
does not authorize ending it. This gives an interrupted launch with no eventual
record a user-controlled exit without treating absence as automatic proof.
If `hc` has been uninstalled, Take back uses this same missing-owner confirmation
and clears the persisted fence only after explicit approval. Other discovery
errors still fail closed. If `hc` is found again when the user confirms, its
terminal is checked again; a live terminal still requires End confirmation.

The tab displays In Herdr, Copy address, Open terminal (copies an `hc attach`
command with an explanatory notice), and Take back. There is no interactive
attachment inside Minutes, remote transfer, live-context forwarding or terminal
state synchronization in this increment. Only explicit Take back polls lifetime.
An isolated Linux `hc` 0.1.21 probe on 2026-11-17 used a temporary `HC_SOCKET` and
XDG runtime/state/config directories: a `/bin/true` terminal remained `finished`
with the same address and exit code after `hc server restart`. The temporary
server was stopped, and the shared server PID was unchanged. That verifies record
retention for this version and restart path, not all versions or record deletion
(`hc rm`); the explicit missing-owner recovery remains necessary.
The existing metadata observer may still receive the tab's Claude hook updates.

## Observed hc contract

Verified against `hc` 0.1.21 on 2026-11-17 using command help, live server/get
readback and owned `/bin/true` and killable `/bin/sleep` probes, both removed
after inspection.
Only these fields are consumed; the server returns additional metadata.

| Command | Relevant JSON shape | Interpretation |
| --- | --- | --- |
| `hc server status --json` | `{"core":{"id":"..."},"version":"0.1.21",...}` | Successful exit and recognized metadata establish availability. |
| `hc run --json --bg ... -- <argv>` | `{"address":"t1d","state":"live",...}` | The local address is the handoff result. |
| `hc get t1d --json` (alive) | `{"address":"t1d","state":"live","exit_code":null,"status":{"state":"done"},...}` | `done` is agent status; this program is still alive. |
| `hc get t1d --json` (exited) | `{"address":"t1d","state":"finished","exit_code":0,"finished_ms":123,...}` | `state` is the lifetime discriminator; nonzero exit also ends ownership. |
| `hc get <missing> --json` | stderr: `{"error":{"kind":"not_found","message":"...","hint":"..."}}`, empty stdout, exit 3 | Absence is recognized separately from transport/parse errors. |
| `hc run --json ...` (invalid cwd or missing binary) | stderr: `{"error":{"kind":"spawn_failed",...}}`, empty stdout, exit 1 | A parsed rejection can be reconciled by terminal name. |
| `hc kill <missing> --json` | stderr: `{"error":{"kind":"not_found",...}}`, empty stdout, exit 3 | Failed kill does not establish exit. |
| `hc ls --json` | `{"hidden_finished":2,"terminals":[{"address":"t1d","state":"live",...}]}` | Observed for compatibility; not used by the adapter. |
| `hc kill <address> --json` | `{"address":"t1f","state":"finished","reason":"killed","signal":1,...}` | Only acknowledgement. A subsequent `get` must report `finished`. |

The error streams above were checked separately on Linux hc 0.1.21 using an
isolated server and temporary socket/state/config directories. The owned server
was stopped afterward and the shared server PID stayed unchanged. The Mac
reviewer independently observed the same missing-terminal stderr contract.

## Qualification boundary

Automated coverage uses isolated fake executables found through an isolated
PATH, with absent, down, invalid, slow, rejected and successful responses. It
checks literal argv preservation, server-side environment cleanup, live-vs-done
lifetime, kill confirmation, uncertain-launch fencing and a real Unix child reap.
Fake command rejections use stderr; one compatibility case also covers stdout.
Error tests check both 64 KiB limits, malformed errors, and stripping private
messages/hints. A shell tab with an active saved Claude transcript selects Claude
and the exact resume ID; absent, empty or mismatched transcripts keep the shell.
GUI-path lookup is tested with an hc-free process PATH and a fake executable
in an augmented directory. Missing-owner confirmation and pre-run failure tests
check persisted fence removal and local-restart dispatch, including uninstalled
hc and a live terminal found after reinstall; Working status is
refused. Palette tests keep discovery unresolved while listing and executing an
ordinary command, then check late append and one edge command per Herdr action.
The tab persistence test restores the ownership fields and accepts old manifests.
The rendered production controller tests cover visibility, Cancel focus, explicit
confirmation, address/attach copying, Take back and no local respawn on relaunch
or selection. The core guard allows only preexisting Herdr dictation-target
recognition and forbids new Herdr identifiers or adapter calls in core.

Signed Mac acceptance belongs to the reviewer: install the exact draft PR head
as Minutes Dev, hand off a previously used Claude tab, confirm a single session
writer and that the same transcript JSONL mtime advances after a new turn, quit
and relaunch Minutes, exercise live/refused and ended Take back, and repeat with
a configured shell. Confirm tokens, keyboard access, reduced motion and narrow
layout in the native webview. Linux tests and CI are not evidence for signing,
TCC, transcript persistence by the real Claude CLI, or native visual acceptance.
