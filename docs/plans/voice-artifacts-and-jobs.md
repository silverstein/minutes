# Voice artifacts and jobs

These opt-in tools extend `voice_live.html_prototypes`; they do not grant new
browser, message-send or shell permissions. The local CLI must be rebuilt and
restarted to load new declarations. The production desktop bundle is unchanged.

## Decision boards

Ask for a Now/Next/Later decision board. `create_decision_board` renders it
directly; it does not call a coding agent. `read_decision_board` returns current
object IDs and revision; `edit_decision_board` applies one validated change.
Supported operations are add/edit/move/merge cards, add/rename/reorder columns,
and scoped undo. Pointer and voice share one store. Stale writes refuse without
changing data. Undo preserves unrelated columns and refuses same-column conflicts.

Select a card in the browser before saying "move this one to Next." The selection
expires after two minutes or an edit. Ambiguous references require clarification.
Saved revisions are private local files under `~/.minutes/decision-boards`.
Each board permits 64 cards, eight columns, 16 undo entries and 128 immutable
revisions. The preview is a session-owned loopback server with a capability token,
exact origin checks and no external resources. Reopen a saved board by its ID.
Closing the voice session closes its preview server; it does not erase the board.

## Reading lists

`create_reading_list` accepts exact source IDs from the current session's public
research. It saves static, escaped HTML under `~/.minutes/reading-lists` without
calling a coding agent. Opening failure leaves the saved file intact. The tool
does not certify source relevance, bibliography metadata or HTTP availability.
Research content must remain useful even if the document cannot be opened.

## Background work

`get_status` includes the latest 20 jobs in chronological order with correlated
IDs, elapsed time and state. Terminal timers are replaced by one delayed cue.
Host context updates use `turnComplete:false`, not an artificial user turn that
would interrupt current speech. The prompt requests an audible acknowledgment;
this is model behavior, not a guarantee of a spoken event on a fixed timer.

`cancel_job` cancels one returned ID. Queued jobs do not start. Supported isolated
coding-agent invocations poll cancellation, terminate their owned process group
on Unix, reap the child, and skip publication when cancellation is observed before
publication. Earlier effects are not undone. Windows currently guarantees direct
child termination, not the complete descendant process tree. Other already-running
operations may finish; their late result is withheld, not described as rolled back.
Provider-cancelled requests are not answered again; host-cancelled requests receive
a minimal cancellation receipt so the model is not left waiting for a result.

Steering is currently cancel, confirm stopped, then issue a revised task. It does
not edit a running agent's prompt. Status/cancel requests have their own lane;
research/reasoning/review, music and prototype generation do not block that lane.

## Local session review

`node tooling/voice-evals/review-session.mjs /absolute/path/to/session.md` reads one
explicitly selected local session, with no network or automatic scan. It emits
tool timing, error and repeated-cue counts, without transcript excerpts, arguments,
results, paths or call IDs. It is not a reasoning-quality score or anonymization
of the original log. Original logs remain private and may contain spoken content.

Both review and Jev qualification scripts have `--self-test`. The Jev script's
`--live` mode sends only built-in synthetic fixtures, never the selected session.
No recurring analysis or private provider export has been enabled.

## Acceptance boundaries

Unit tests cover shared revisions, stale selection, scoped undo, persistence,
local HTTP authentication/origin, escaping, cancellation and subprocess reaping.
Synthetic Live testing verifies board-tool routing and current-revision use.
Isolated browser checks cover desktop/mobile layout, selection, move and undo.
These do not substitute for a full hands-free rehearsal with native destination
readback. Stronger native field binding and that rehearsal remain separate gates.
Jev is evaluated, not enabled for the private corpus or live screen by this change.
