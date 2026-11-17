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

`auto` exposes the tab menu and palette command only when `hc` is on the desktop
process's PATH and `hc server status --json` returns recognized server metadata.
No handoff happens automatically. Invalid configuration fails closed. A saved
external owner remains visible even when discovery fails or the switch is off;
Take back can still locate `hc` to reconcile that existing owner.
No machine, space, lane, credentials or provider settings are assumed. There is
no `--on` flag, and addresses containing `/` are rejected.

## Ownership and recovery

The menu and palette route to the same in-panel confirmation, initially focused
on Cancel. Opening the palette command does not itself move a conversation.
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
Normal shutdown retains its existing asynchronous reaping behavior.

The launch uses the configured assistant agent and filtered `agent_args`, the
same skill arguments and Claude session-argument normalization as a local tab,
and the tab's own working directory. Only a matching nonempty transcript earns
`--resume`; a never-used Claude tab starts fresh. Transcript metadata is refreshed
after the local child exits so a final saved turn can be recovered. Configured
shells remain shells, following existing Recall semantics; their Resume action
continues to be explicit after Take back.

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
limited to 64 KiB. Arbitrary stderr is not displayed to avoid exposing private
context. No provider login or permissions are changed.

A rejected launch is reconciled by name. Confirmed absence/finished state after
a definitive rejection clears the fence, restarts the local tab with its previous
theme and shows the error. A recovered live terminal remains the owner. An
unanswered, malformed or timed-out request is ambiguous: it may have reached the
server, so Minutes keeps the fence and shows the error instead of starting a
second writer. This is a deliberate safety refinement of the bead's unconditional
restart-on-failure wording. Take back requires a recognized finished record or
an explicit End terminal and take back action followed by confirmed exit.
`not_found`, unknown state, failed kill and server failure do not prove exit.
An interrupted launch with no eventual terminal record needs operator diagnosis;
Minutes never guesses that it is safe to start another session writer.

The tab displays In Herdr, Copy address, Open terminal (copies an `hc attach`
command with an explanatory notice), and Take back. There is no interactive
attachment inside Minutes, remote transfer, live-context forwarding or terminal
state synchronization in this increment. Only explicit Take back polls lifetime.
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
| `hc get <missing> --json` | `{"error":{"kind":"not_found","message":"...","hint":"..."}}` with exit 3 | Absence is recognized separately from transport/parse errors. |
| `hc ls --json` | `{"hidden_finished":2,"terminals":[{"address":"t1d","state":"live",...}]}` | Observed for compatibility; not used by the adapter. |
| `hc kill <address> --json` | `{"address":"t1f","state":"finished","reason":"killed","signal":1,...}` | Only acknowledgement. A subsequent `get` must report `finished`. |

## Qualification boundary

Automated coverage uses isolated fake executables found through an isolated
PATH, with absent, down, invalid, slow, rejected and successful responses. It
checks literal argv preservation, server-side environment cleanup, live-vs-done
lifetime, kill confirmation, uncertain-launch fencing and a real Unix child reap.
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
