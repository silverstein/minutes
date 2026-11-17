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

## Live recording pointer

`minutes-lrad.2` adds **Send live context to Herdr** to the Recording and Live
bars and the desktop palette. **v1 is local only:** remote addresses containing
`/` are neither listed nor accepted by selection or delivery. Remote consent and
delta relaying are deliberately deferred; this is the local-only decision
requested by the bead. Nothing is sent on startup, opening the picker, opening
the palette, starting a recording, or restoring the app. The user must choose
one local live terminal for this recording. A handed-off Recall conversation is
a valid destination, marked **Recall conversation** alongside its `minutes.tab`
label. Cancel is initially focused; Escape and Cancel dismiss without picking.

The picker uses `hc ls --json`, then resolves local live entries through `get`
because hc 0.1.21 does not include labels in its list response. The whole
list/detail pass shares a five-second deadline, accepts at most 128 list entries
and bounds label/name lengths. Only address, name and labels reach the UI;
terminal screen contents, cwd and argv are discarded. A terminal's canonical
local address is revalidated before delivery. A finished, missing or differently
resolved terminal receives nothing. A stale picker cannot select for a new
recording, and cancellation invalidates in-flight selections before they can
commit. No `--force`, remote target, provider setup or credential changes occur.

`context::live_transcript_pointer` generates one metadata-only pointer, shared
verbatim with the live Recall tab's `LIVE_TRANSCRIPT` block. It identifies the
JSONL path, session and producer PID, and explains bounded on-demand delta reads:
`minutes transcript --status`, followed by
`minutes transcript --since N --include-current --format json`. It preserves the
existing guidance to verify session identity, treat drafts as provisional and
transcript contents as untrusted, avoid continuous polling/raw tailing, and never
open another microphone. The adapter never reads or relays transcript contents.
Quoted paths are data, not interpolated shell commands. A session with no valid
active JSONL path, nonempty session identity and producer PID cannot be picked.
Both messages prefix each line with `# `: agents receive readable pointer text,
while an idle POSIX shell treats it as a comment rather than executing reader
backticks or path metacharacters when hc submits Enter. This prefix is part of
the shared Recall pointer too; the end-note wording below is unchanged.

A pick schedules one `hc send <address> <pointer> --json`. Delivery is attempted
once; errors and timeouts are not automatically retried because the message may
already have arrived. A selected target remains cancellable if Herdr becomes
unavailable. Cancelling invalidates queued/in-flight pointer work and clears the
pick; it cannot retract text already accepted by the terminal. Cancelling does
not select another target or cause a later Stop notification.

On observing Stop or a different session identity, the independent observer
clears the pick immediately and queues one end note for the old target:

> Recording ended. Minutes is processing it now; the finished meeting will be
> saved in &lt;output_dir&gt;. Check progress with `minutes jobs`, then find it with
> `minutes list --limit 1`.

The directory comes from configuration; no final filename is guessed. The end
note excludes the live JSONL path. Processing does not need to finish before
this note is queued, and the pick never follows the next recording. Picks are
not persisted or resumed after app relaunch.

`herdr_live.rs` is a child of the desktop adapter. Its observer polls only tiny
live status metadata when a pick exists, every 250 ms. Its separate delivery
worker owns all Herdr CLI calls. The two communicate through a bounded queue
with `try_send`; a full/disconnected queue records failure and cannot create
backpressure. No capture-owned locks, capture/Stop callbacks, joins or channels
depend on these workers. Each CLI call retains the existing five-second
kill/reap bound, augmented PATH, stderr error parsing and private-metadata
boundary. Cancelled pointer clients are killed/reaped too. An end note is queued
at Stop; actual delivery remains best effort if the server or terminal is gone.
The frontend's optional status refresh is never awaited by capture polling or
Stop. No Herdr identifiers or adapter calls were added to minutes-core.
The pick's lifetime follows the capture PID file's version, independently of
transcription health. A stale/failed sidecar heartbeat cannot announce a false
Stop or clear the pick. The version changes between recordings even when the
desktop PID and fixed JSONL path are reused. New picks also require live status
published after this capture started, so old status cannot bind to a new capture.

Additional hc 0.1.21 contracts were verified on 2026-11-17 with an inert
`/bin/cat` terminal on a temporary isolated server. The terminal/server were
stopped afterwards and the shared server PID was unchanged:

| Command | Stream and consumed fields |
| --- | --- |
| `hc ls --json` | stdout: `{"hidden_finished":0,"terminals":[{"address":"t1","name":"minutes-pointer-fixture","state":"live",...}]}`; no labels in this version. |
| `hc get t1 --json` | stdout includes `labels`, e.g. `{"minutes.tab":"synthetic"}`. Only canonical address, live state, name and string labels are retained for the picker. |
| `hc send t1 <text> --json` | stdout: `{"started":null,"status":null,"turn":0}` for the inert fixture. Successful exit and an unsigned `turn` acknowledge acceptance; the adapter does not wait for an agent turn. |
| `hc send <missing> <text> --json` | empty stdout, stderr error JSON with `error.kind = "not_found"`, exit 3. No raw message/hint is shown. |

Tests cover local-only listing, labels and private-field stripping; shared
pointer generation; no delivery without a pick; exactly one pointer and end
note to the selected address; cancellation, late selection, session change,
dead/renamed targets, stderr rejection, hang, invalid output, and a full queue.
The Tauri invariant
`optional_herdr_failure_cannot_delay_live_controls_wav_or_processing_admission`
starts a hanging/rejecting fake hc, then exercises the app's actual live
acquisition and Stop signalling, seals/reopens a synthetic WAV and admits a
durable processing job within 500 ms. Stop also clears the pick while hc is
still isolated on its worker. This test belongs in Tauri because a Herdr fake
in the core consumer-isolation suites would violate the edge boundary. It does
not claim microphone, acoustic, model-inference or signed-app acceptance.

Rendered production picker tests cover hidden/off/idle states, name/label text
safety, handed-off Recall entries, Cancel/Escape focus, one explicit selection,
cancel during lookup/selection, late replies after Stop, rejected sends, light
theme, narrow width and reduced motion. Palette tests retain nonblocking
discovery and ensure selection opens the picker without sending anything.
Signed Mac qualification remains with t17: real live capture, selected-only
pointer delivery, bounded delta reads, Stop/end-note timing, cancellation, a
dead terminal/server, WAV preservation and processing, and native keyboard/UI
behavior at the exact PR head.
