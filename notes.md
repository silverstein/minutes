## What's new

**macOS: an idle Minutes no longer slows down your whole Mac.** When Minutes sat idle, usually hidden in the menu bar, its status refresh repainted the translucent main window every couple of seconds even when nothing had changed. On macOS that makes the window server re-blend the entire frosted-glass surface each time, so over a few hours it pegged a CPU core and made scrolling and switching apps stutter across the whole system. Idle refreshes now skip redundant redraws, so a hidden, idle Minutes leaves your Mac alone. Windows and Linux were not affected.

**The AI assistant is now optional.** If you do not have an agent CLI installed (Claude Code by default), Minutes no longer auto-opens the assistant panel and then fails to launch it. It degrades quietly, the rest of the app keeps working, and the assistant shows a calm note that it is optional. Thanks to Mike (@mquinn614) for the fix.

## Install / update

The desktop app updates itself: open Minutes and it pulls v0.18.5 on next launch, or grab the DMG from the assets below.

- DMG: download from the release assets below
- CLI: `brew install silverstein/tap/minutes` or `cargo install minutes-cli`
- MCP: `npx minutes-mcp`
