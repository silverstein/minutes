# Minutes v0.11.0: Command palette

## Press ⌘⇧K. Type the first few letters of what you want. Done.

v0.11.0 ships the command palette: a keyboard-first launcher for every Minutes feature, backed by a single typed registry the desktop app, MCP server, and CLI all consume from. It's the kind of thing you stop noticing after the first day, and then notice when it's not there.

### What you can do with it

Press `⌘⇧K` from anywhere on macOS. The palette opens centered on your active monitor. Type `rec` and Enter starts recording. Type `note` and Enter opens an input field; type your annotation and Enter again drops a timestamped note into the active session. Type `search pricing` and Enter and you're searching transcripts.

Twenty commands total in v0.11.0:

**Recording**: start, stop, add note (during recording), start live transcript, stop live transcript, read live transcript

**Dictation**: start, stop

**Navigation**: open latest meeting, open latest meeting from today, show upcoming meetings, open meetings folder, open memos folder, open assistant workspace

**Search**: search transcripts, research topic, find open action items, find recent decisions

**Meeting actions** (only when an assistant meeting is open): copy meeting markdown, rename current meeting

The palette knows what state you're in. Stop-recording only appears while you're recording. Start-dictation disappears mid-session. Rename-current-meeting only shows up when the assistant has a meeting selected. The list re-fetches automatically when state changes elsewhere — start a recording from the tray menu while the palette is open and you'll see it react in under a frame.

### Recents float to the top

The five most recently executed commands appear above the full list when the query is empty, with their original payload intact. Search "pricing" once, and tomorrow you can re-run the same search by pressing `⌘⇧K`, Enter — the recents store remembers the query, not just the command name. The store is forward-compatible: if you upgrade and a future Minutes adds a new command, the older client will preserve those entries on disk instead of eating them on next write.

If `~/.minutes/palette.json` ever gets corrupted, the palette quarantines it as `palette.json.broken` for forensics and starts fresh. The palette will always open.

### Rename current meeting refuses to corrupt your files

`Rename current meeting` is a fail-closed operation. It parses your meeting frontmatter as YAML, refuses to touch any file whose `title` field is anything other than a plain-string scalar (no folded blocks, no anchors, no aliases), edits exactly the title line, validates the result by re-parsing it, and rolls back if the post-write parse fails. Then it renames the file on disk to match the new slug. User-added sections in the body are preserved exactly as you wrote them.

If you've ever been bitten by a rename tool that quietly mangled your YAML, this one is the opposite of that.

### Enabled by default with a first-run heads-up

The palette shortcut defaults on for both fresh installs and upgrades. The first time v0.11.0 launches and registers `⌘⇧K`, you'll see a one-time macOS notification telling you the binding is live and pointing you at Settings to disable it if it conflicts with your IDE. Subsequent launches are silent.

`⌘⇧K` overlaps with Finder's "Connect to Server" (rare-use) and Firefox's Web Console. If you use either, or if you have a different `⌘⇧K` binding in another app, open the Settings overlay (gear icon in the main window), find the **Command Palette** section, and either:

- Toggle the shortcut off, or
- Pick a different binding from the dropdown — `⌘⇧K`, `⌘⇧O`, or `⌘⇧U`. None of these collide with the other Minutes shortcuts (dictation, live transcript, quick thought).

You can also edit `~/.config/minutes/config.toml`:

```toml
[palette]
shortcut_enabled = false   # or true
shortcut = "CmdOrCtrl+Shift+O"
```

The overlay itself shows "Minutes Palette" in the footer, so if you ever hit `⌘⇧K` and forgot what installed it, the answer is right there.

An earlier draft of this release defaulted the shortcut **off** for upgrades on the theory that opt-in beats hijacking. Dogfood feedback caught the problem with that design: opt-in via `config.toml` is invisible. Existing users would never discover the feature. The first-run notification + the visible Settings UI surface together replace the opt-in default with explicit consent that's actually findable.

### A note on `OpenLatestMeetingFromToday` recents

The recents store re-runs commands with the same input parameters, not the same output artifact. For most commands that's exactly what you want. For `OpenLatestMeetingFromToday` it means re-running the recent tomorrow opens whatever's "latest from today" tomorrow — i.e., the latest meeting from THAT day, not the meeting that was originally opened. This is intentional ("re-do my morning meeting check") but worth knowing.

### Under the hood: one registry, three codex passes, dual-reviewer final

The palette is built around a single typed command registry in `crates/core/src/palette.rs`. The desktop dispatcher matches on `ActionId` directly — no parallel mirror, no stringly-typed glue. Adding a new variant in core fails the Tauri build until a dispatch arm exists. The exhaustive match is the spec.

Every architectural decision survived three adversarial review rounds: pass 1 on the plan before any Rust got written (6 P1, 5 P2, 3 P3), pass 2 on the slice 2 implementation diff (2 P1, 3 P2, 3 P3), and a final dual-reviewer round with both codex and a fresh Claude reviewer running in parallel on the design fix (5 P1, 8 P2, 7 P3 combined). Every P1 was addressed. Findings logged in `PLAN.md.command-palette-slice-2` if you want the receipts.

### Tests

- 25 palette registry + recents tests in `minutes-core`
- 16 `markdown::rename_meeting` tests covering folded scalars, anchors, aliases, literal blocks, CRLF line endings, slug collisions, no-op renames, special-character escapes, post-write rollback path, and Unix file-mode preservation
- 22 Tauri palette tests including 11 `ActionResponse` envelope tests, 3 palette shortcut validation tests, the cross-collision smoke test, and the humanize-shortcut renderer
- 18 config tests including the upgrade migration

All green. `cargo fmt --all -- --check` clean. `cargo clippy --all --no-default-features -- -D warnings` clean.

### Install

Mac:
```bash
brew install silverstein/tap/minutes
```

Or download `Minutes_0.11.0_aarch64.dmg` below.

For Claude Desktop:
```bash
npx minutes-mcp@0.11.0
```

### What's next

The palette is the fourth command surface for Minutes (CLI, MCP, tray menu, palette). The registry is the foundation for unifying the other three. Slice 3 will start migrating CLI subcommand definitions to consume from the same registry, so a new feature lands in one place and reaches all three surfaces automatically.

Other things that didn't make this release and are on the followup list:
- Reprocess current meeting (needs a contract for output semantics — sibling artifact vs in-place vs versioned)
- A real `validate_recording_start` that catches mic permission, disk space, and PID failures synchronously instead of via deferred notifications
- Multi-meeting picker for "open latest meeting from today" when there's more than one
- The inline shortcut form (`> search pricing` parses as Search Transcripts with the query payload)
- Fully-typed inner DTOs for the `ActionResponse` passthrough variants (currently `serde_json::Value`)

Each of these is filed.

### Thanks

Two adversarial codex passes saved this release from a handful of subtle bugs. The plan-first discipline (write the plan, run codex on the plan, fix the plan, *then* write the code) is the most useful workflow change we've made this year.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
