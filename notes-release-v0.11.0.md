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

### Existing users opt in. Fresh installs see it on first run.

If you're upgrading from v0.10.x, the palette shortcut is **disabled** by default. The first time v0.11.0 loads your existing config, it adds a `[palette]` section with `shortcut_enabled = false` and persists it. This is on purpose: `⌘⇧K` is `Delete Line` in VS Code and `Push...` in JetBrains. We don't want to silently steal a chord you already use.

To enable it:

```toml
# ~/.config/minutes/config.toml
[palette]
shortcut_enabled = true
shortcut = "CmdOrCtrl+Shift+K"
```

Restart the desktop app. The shortcut registers immediately. You can rebind to anything Carbon's `RegisterEventHotKey` accepts.

Fresh installs default to enabled, so new users discover the feature on first run without hunting.

### Under the hood: one registry, four codex passes

The palette is built around a single typed command registry in `crates/core/src/palette.rs`. The desktop dispatcher matches on `ActionId` directly — no parallel mirror, no stringly-typed glue. Adding a new variant in core fails the Tauri build until a dispatch arm exists. The exhaustive match is the spec.

This is the same registry pattern slice 1 (`v0.11.0` was developed across two slices, both in this release) introduced in private without a UI. Slice 2 is what makes it visible.

Every architectural decision survived two adversarial codex passes — one on the plan before any Rust got written, one on the diff before this PR opened. Six P1 findings, five P2, three P3 from the plan review alone, all addressed. Findings logged in `PLAN.md.command-palette-slice-2` if you're curious.

### Tests

- 25 palette registry + recents tests in `minutes-core`
- 12 `markdown::rename_meeting` tests covering folded scalars, anchors, literal blocks, slug collisions, no-op renames, special-character escapes, and parse-after-write rollback
- 20 Tauri palette tests including 11 `ActionResponse` envelope tests (one per variant)
- 5 config migration tests covering the upgrade path

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
