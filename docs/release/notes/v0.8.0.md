# Minutes Plugin v0.8.0: Four new skills, a proactive brief hook, and bug fixes two review passes deep

## The lifecycle is done. Brief runs before your call, mirror tells you what you did, tag marks the outcome, graph lets you query everyone and everything across your history. All four ship with Python helper scripts that do the counting deterministically so an LLM doesn't have to guess.

Before v0.8.0 the plugin covered prep, record, debrief, weekly. After v0.8.0 it covers the full arc: `brief → prep → record → tag → debrief → mirror → weekly → graph`. Nothing got removed, nothing existing changed. The new skills plug into the same lifecycle files (`~/.minutes/preps/`, `~/.minutes/briefs/`) so debrief can pick them up after the call.

This release also borrows Garry Tan's individual-mode update-check pattern from gstack, so starting with v0.8.0 the plugin will tell you when the next release ships instead of leaving you to remember to check.

## The four new skills

**`/minutes-brief`** is the fast one. It auto-detects your next calendar event (Google Calendar MCP first, then `gog` CLI, then Apple Calendar via osascript), pulls person history, open commitments, recent decisions, and recent meetings in parallel, reads the last 1 or 2 meeting files for last-conversation tone, and produces a one-page brief: last conversation, recent hot topics, open commitments in both directions with overdue flagged, one-line vibe, and a concrete "Open with" sentence you can literally say at the top of the call. Designed so a SessionStart hook can fire it silently; also works as `/minutes-brief <name>` when you want it manually.

Brief is the fast version, prep is the interactive version. Brief asks zero questions and sets zero goals. Prep walks you through an AskUserQuestion flow to lock in what you actually want from the call. Both save to `~/.minutes/{briefs,preps}/` so debrief can find them after. Use brief when you have 60 seconds before the call, prep when you have 10 minutes and want to think hard.

**`/minutes-mirror`** is the self-coaching one. Two modes: single-meeting review (how did I do in the Sarah call) and 30-day pattern mode (what do my winning meetings have in common). Metrics it computes for you: talk-time ratio, filler word density, hedging density, longest monologue, longest stretch you listened, question rate, and per-meeting deltas from your baseline. When meetings are tagged via `/minutes-tag`, mirror correlates behavior with outcomes. One real output shape: "in meetings you tagged won, your average talk ratio was 38%. In lost, 67%."

The important bit is that **mirror doesn't ask Claude to count tokens in-context**. The bundled `mirror_metrics.py` script does the counting deterministically with regex and basic string ops, so a 6,000-word transcript gets the same answer every run. LLMs are bad at precise token counting. The script sidesteps that entirely. It handles both bracket-style (`[Mat 0:00]`) and bold-style (`**Hiro**:`) transcript formats, multi-word speaker labels like `[Mat S. 0:00]`, and bounds the transcript at the next `##` heading so post-transcript sections like `## Action Items` don't contaminate the metrics.

**`/minutes-tag`** is the 5-second one. It adds an `outcome:` field (`won`, `lost`, `stalled`, `great`, `noise`, or a custom value) to a meeting's YAML frontmatter so mirror can correlate behavior with results over time. The bundled `tag_apply.py` script does atomic YAML-safe frontmatter edits via tempfile + rename, **preserves the original file mode** (important for meetings you've `chmod 600`'d for privacy), and preserves every other frontmatter field byte-for-byte. Idempotent on re-tag. One-time lifetime nudge via `~/.minutes/tag-nudge-shown` marker, so you never get nagged twice.

Tag parses the outcome note from your original message ("tag that as a win because Sarah committed to monthly") instead of asking a second follow-up question. Speed is the whole feature. If tagging takes more than one prompt, tag has failed at its job.

**`/minutes-graph`** is the cross-meeting query layer. It queries people and topics as structured data with co-occurrence relationships precomputed, so questions like "who's been mentioned alongside Sarah", "every time we talked about pricing", and "first time the term X appears in my history" are single map lookups instead of text searches. Graph defers to `minutes people`, `minutes person`, and `minutes insights` when the CLI already answers the question. It only does the work the CLI can't do.

The bundled `graph_build.py` walks real meeting frontmatter (`attendees`, `people` slugs, `tags`, `decisions[].topic`), augments with `minutes people --json` output, filters speaker-diarization noise (`unknown-speaker`, `speaker-3`, etc.), and picks canonical display names via a "looks human" heuristic (capital letter plus space wins over lowercase-slug form). Incremental rebuilds complete in under a second. First build on ~50 meetings completes in under 5 seconds.

## Hook upgrade: proactive brief + plugin update check

`hooks/session-reminder.mjs` picks up two additions.

**Calendar context** now uses a three-way decision tree. If `osascript` plus Apple Calendar are available and find a meeting in the next 60 minutes, the hook injects a precise "run /minutes-brief or /minutes-prep" recommendation. If `osascript` succeeds with no meetings, the hook injects **zero calendar context**. That's how we earn back the right to fire on every startup, after commit `0b8adea` removed the earlier too-verbose version. If `osascript` fails (non-Mac, Calendar.app not running, permission denied, timeout), the hook falls back to a one-sentence hint telling Claude to check via the Google Calendar MCP if that's available.

**Update check** is the other addition. It's ported from `garrytan/gstack`'s `bin/gstack-update-check` (211 lines in gstack, about 200 lines in our hook). On every session start, subject to a 60 min / 12 hr cache and a per-version escalating-backoff snooze, it fetches the canonical `.claude-plugin/plugin.json` from `raw.githubusercontent.com/silverstein/minutes/main/`, compares to the local version, and when a newer version is available injects a notice with the full three-step upgrade sequence. Respects `[updates] check = false` in `~/.config/minutes/config.toml` for users who don't want the check.

One honest caveat: the recursive bootstrap problem applies. Users on v0.7.0 don't have this code yet, so they won't be auto-notified of v0.8.0. From v0.8.0 forward, the loop runs on its own.

## Dead `plugin/` tree deleted

`plugin/` at the repo root had been frozen for two weeks. It was orphaned when commit `270839d` (Mar 24) switched `marketplace.json` from `./plugin` to `./.claude/plugins/minutes`. Every feature commit after that date went into `.claude/plugins/minutes/` exclusively. This release deletes the dead tree (20 files), cleans up `.mcpbignore`'s stale `plugin/hooks/test/` entry, and adds `plugin/` to `.gitignore` as a tombstone so future sessions can't accidentally recreate the drift.

I ran into this drift myself during the release work, which is how it got caught. I wrote four new skills into the wrong tree before the marketplace pointer revealed the truth.

## Docs harmonization

All 18 skills now use the hyphenated form (`# /minutes-brief`) in their H1 headers to match README.md and the actual slash-command surface. Eight cross-reference lines inside skill bodies (debrief to prep, prep to debrief, weekly to prep, lint to debrief and search, recap to search) also switched from the old space form. `docs/designs/interactive-skills-ecosystem.md` picked up a status note marking it as a March 2026 historical snapshot with a pointer to README.md as the current source of truth. `docs/SKILL-TEMPLATE-INTERACTIVE.md` got updated so future skill authors using it as a template see the right form.

## Bug fixes from two rounds of external review

**Round 1** (caught by the first Codex pass, before I thought I was done):

- `tag_apply.py` was dropping file permissions from 0600 to 0644 on the atomic temp-write-and-rename. Meetings you'd locked down for privacy were silently becoming world-readable after tagging. The fix captures `st_mode` before writing and restores it before the `replace()` call.
- `mirror_metrics.py` speaker regex was `[^\s\]]+`, which stops at the first whitespace. Multi-word labels like `[Mat S. 0:00]` became speaker "Mat" with "S." leaking into the text. Fixed by switching to non-greedy `(.+?)` so the optional timestamp group claims the trailing `\s+\d+:\d+`.
- `session-reminder.mjs` opt-out check was `includes("enabled = false") && includes("[reminders]")`. It false-positived on configs like `[audio]\nenabled = false\n[reminders]\nenabled = true`. The fix is a scoped regex `/\[reminders\][^\[]*\benabled\s*=\s*false\b/` that requires `enabled = false` to appear inside the `[reminders]` block before any subsequent `[section]` header.

**Round 2** (caught by the second Codex pass, after I thought the first round was done):

- `mirror_metrics.py` only recognized bracket-style `[NAME 0:00]` speaker turns. Real Minutes meetings also use bold-style `**Name**: text` (imported or cleaned transcripts). Running mirror on a bold-style transcript returned `{"error": "no diarized speaker turns found"}`. Now tries both formats via a `match_speaker_line()` helper.
- `mirror_metrics.py` read from `## Transcript` to EOF and appended non-speaker lines to the current turn, which meant `## Action Items` and `## Decisions` sections after the transcript got concatenated onto the last speaker's text. Tested with a tiny fixture: Mat's actual 15 words were being counted as 31 because post-transcript sections glued onto the final turn. `extract_transcript()` now bounds the body at the next `## ` heading.
- `/minutes-brief` assumed `minutes person` writes clean JSON to stdout. Verified on this machine: it writes **both** the WARN tracing lines **and** the JSON profile to stdout (not stderr), and the human-readable "Profile for Mat: ..." text goes to stderr. Totally inverted. The skill now uses `minutes person "$name" 2>/dev/null | sed -n '/^{/,$p'` to strip the WARN prefix before parsing. Added a "CLI stream-handling notes" block to the skill documenting the per-command stdout contracts honestly instead of pretending they're uniform.
- `/minutes-graph`'s doc overstated that the `entities:` block doesn't exist in real meeting frontmatter. Verified: some meetings **do** have it (e.g. `2026-04-08-codex-native-call-attribution-repro-8-mat.md` has `entities: { people: [...], projects: [...] }`), but the schema is inconsistent across the corpus. Updated the doc to explain that `graph_build.py` intentionally uses the narrower, more-consistent set of fields (`attendees`, `people`, `tags`, `decisions[].topic`) and that modifying the script to also parse `entities:` is the right path if someone wants that data.

**Round 3** (caught after I thought v0.8.0 was ready to push):

Commit `b82e891` shipped the update-check feature but recommended `/plugin update minutes` alone as the upgrade command. That's a silent no-op in practice because of a Claude Code marketplace quirk I hadn't understood yet. Commit `0f0254e` corrected it to the full three-step sequence. See the "Upgrade incantation" section below for the full story and the commands that actually work.

## Who should care

**Anyone running the Minutes Claude Code plugin**, all 18 skills are in this release. If you currently use `/minutes-prep`, `/minutes-debrief`, or `/minutes-weekly`, you should update. The new brief, mirror, tag, and graph skills plug directly into the same lifecycle and don't change anything about how the existing four work.

**Anyone with a Minutes install on a Mac**, the SessionStart hook now does a calendar pre-check via `osascript` and recommends `/minutes-brief` when a meeting is in the next 60 min. If you don't want this, `[reminders] enabled = false` in `~/.config/minutes/config.toml` still turns the whole thing off.

**Anyone who wants the plugin to tell them when new versions ship**, the update-check hook is new in v0.8.0 and will notify you of v0.9.0 and beyond. Opt out via `[updates] check = false` if you'd rather check manually.

**Nobody on the MCP-server, CLI, or desktop app side.** This release is plugin-only. The Rust binary, the npm MCP server, the Tauri desktop app, and the Homebrew tap are all unchanged.

## CLI / MCP / desktop impact

**CLI:** no changes. The four new skills call existing `minutes` CLI commands (`person`, `commitments`, `insights`, `search`, `people`, `paths`, `voices`, `get`, `list`). All of these exist in v0.11.0 and earlier, so there's no minimum CLI version required beyond what v0.11.0 already shipped. If you're on v0.11.0 of the binary, the plugin upgrade gives you everything immediately. No binary update needed.

**MCP:** no changes. The MCP server wasn't touched. Both `crates/sdk` and `crates/mcp` are unchanged. `npx minutes-mcp@0.11.0` still delivers the same 26 tools, 7 resources, and interactive dashboard. No npm republish is needed for this release.

**Desktop:** no changes. The Tauri app is unchanged. No DMG rebuild, no notarization, no auto-updater `latest.json` update, no Homebrew tap bump. `brew install --cask silverstein/tap/minutes` still serves the existing Minutes.app.

This is a **plugin-only release**. It's distributed by `git push origin main` plus a GitHub Release tag (`plugin-v0.8.0`) that sits outside the `v*` namespace so it doesn't trigger the binary release workflows.

### New dependency: Python 3.8+ for the bundled helper scripts

Mirror, tag, and graph each ship a helper script under `skills/<name>/scripts/<name>.py` that the skill invokes via `python3`. macOS ships Python 3 system-wide, so the majority of users don't have to do anything. Linux users with a bare install and no Python 3 will see the scripts fail, and should install Python 3.8 or newer via their package manager. No third-party packages are required. The scripts are stdlib-only. No PyYAML, no requests, no numpy.

## Upgrade incantation

If you're currently on v0.7.0, this is what you run. Copy-paste the full sequence. Skipping steps 1 or 3 leaves you on a half-upgraded install.

```bash
/plugin marketplace update minutes      # git-pulls the local marketplace mirror so Claude Code knows v0.8.0 exists
/plugin update minutes@minutes          # installs the refreshed version into the plugin cache
# Then restart Claude Code so the new skills, hooks, and helper scripts load into your session
```

Why the two-step dance? Claude Code's marketplace mechanism caches each marketplace as a local git clone at `~/.claude/plugins/marketplaces/<name>/`. Running `/plugin update` on its own consults the cached `marketplace.json` for the version number. If that mirror is stuck at an old commit (which it will be, because git pulls only happen when you explicitly ask), Claude Code reports "already at latest" and does nothing. `/plugin marketplace update` is the git pull that unsticks the mirror. Once the mirror is fresh, `/plugin update` sees the new version and installs it.

This is not user error. It's how the marketplace works today. Future Claude Code releases may surface an auto-pull option, but until then, always use the two-step form in release notes.

**Nuke-and-pave alternative** (use this if you're on a very old version and the staged upgrade gets confused):

```bash
/plugin uninstall minutes
/plugin marketplace update minutes
/plugin install minutes@minutes
# Restart Claude Code
```

After the restart, sanity-check with:

```bash
ls ~/.claude/plugins/cache/minutes/minutes/0.8.0/minutes/.claude-plugin/plugin.json && \
  grep '"version"' ~/.claude/plugins/cache/minutes/minutes/0.8.0/minutes/.claude-plugin/plugin.json
```

You should see `"version": "0.8.0"`. If instead you see a different number or the file doesn't exist, the upgrade didn't land. Open an issue with `~/.claude/plugins/marketplaces/minutes/` and `~/.claude/plugins/cache/minutes/` directory listings attached and we'll debug.

## Breaking changes or migration notes

None. This is purely additive from a user's perspective. Nothing was removed, no existing skill's behavior changed, no frontmatter schema was tightened, no CLI command was deprecated.

One soft "migration" worth calling out: the outcome-tagging frontmatter fields (`outcome:`, `outcome_note:`, `tagged_at:`) are new. They're optional and only appear when you run `/minutes-tag` on a meeting. Existing meetings without these fields continue to work unchanged. Mirror's correlation analysis simply skips untagged meetings. There's no batch-tag migration and none is recommended. Tag meetings as you close them going forward, and the dataset builds up naturally.

One architectural note for future plugin contributors: this release introduces bundled Python helper scripts as a first-class pattern (`skills/<name>/scripts/<name>.py`). The pre-existing `minutes-verify/scripts/verify-setup.sh` had this pattern already, but it was an outlier. Now four of the 18 skills ship scripts. If you're writing a new skill that needs deterministic computation (counting, parsing, YAML editing, atomic writes), following the same pattern is the right move. LLM-driven approximation fails at scale. Scripts give you repeatable tests.

## Known issues

- Users on v0.7.0 of the plugin are not auto-notified of v0.8.0. The update-check hook is new in v0.8.0, so pre-0.8.0 users don't have it. This is the recursive bootstrap problem every auto-updater has. After upgrading manually once, users will be auto-notified of v0.9.0 and beyond.
- The calendar pre-check in the hook is macOS-optimized. The fetch itself uses `curl` via `execFileSync` which exists everywhere, so that part is cross-platform. But the calendar pre-check uses `osascript` which is macOS-only. On Linux it falls back to the Google Calendar MCP hint. No functional regression for Linux users, just one less precision path.
- `mirror_metrics.py` longest-monologue and longest-listen stretches are approximations on long meetings. For transcripts under 5,000 words, counts are precise. For longer transcripts, the script still gives you the right answer for talk-time ratio and filler counts, but the stretch-length numbers are bounded by how well the input transcript represents the real call (whisper.cpp's turn boundaries matter here). When in doubt, the skill surfaces a "≈" prefix and a warning.
- The `entities:` block in meeting frontmatter is inconsistent. Some meetings have it with a rich schema (`people`, `projects`), some don't have it at all. `graph_build.py` intentionally skips it and uses the more-consistent `people` slug + `tags` + `decisions[].topic` fields instead. If you want `entities:`-sourced data in your graph, modify `graph_build.py` to parse it and be ready to handle variant schemas across meetings.
- `minutes person` has an inverted stdout contract. It writes WARN tracing lines AND the JSON profile to stdout, with the human-readable summary on stderr. `/minutes-brief` handles this via `sed` extraction, but if you're building other tooling on top of `minutes person`, expect to strip the WARN prefix before JSON parsing. This is a CLI bug that should be fixed in a future Rust release.

## Install

```bash
# First-time install (new users)
/plugin marketplace add silverstein/minutes
/plugin install minutes@minutes
# Then restart Claude Code

# Upgrading from v0.7.0 (see the upgrade incantation section above for why)
/plugin marketplace update minutes
/plugin update minutes@minutes
# Then restart Claude Code
```

The plugin requires the `minutes` CLI binary (v0.11.0 or newer) to be on PATH. Install the binary separately via `brew install silverstein/tap/minutes` or `cargo install minutes-cli`. The plugin also requires Python 3.8+ on PATH for the bundled helper scripts. macOS ships Python 3 system-wide. Linux users may need `apt install python3` or equivalent.

## Credit

- **gstack / Garry Tan.** The update-check pattern in the SessionStart hook is ported from `bin/gstack-update-check` in `github.com/garrytan/gstack`. The escalating snooze (24h, 48h, 7d), the version-reset-on-new-release behavior, the per-state cache TTLs, the scoped-regex opt-out, all directly stolen from gstack's design, adapted to the Claude Code marketplace reality. Zero new ideas in the update check. gstack got it right and I matched it.
- **Codex.** Three rounds of adversarial code review caught six real bugs in code I had self-scored 10/10 on, including three that would have shipped broken: the `/minutes-brief` stdout-parsing bug, the `mirror_metrics.py` transcript-overrun bug, and the correct-upgrade-command bug in this very release note's predecessor commit. Self-review has a ceiling. Independent review is cheap insurance.

## Full changelog

The 7 commits that constitute this plugin release (newest first):

```
df1e6ac docs: add upgrade block to README plugin-install section
098d970 docs: plugin v0.8.0 release notes + checklist + CLAUDE.md for marketplace cache quirk
9b6c8b7 fix(hook): correct update-check to recommend the full three-step refresh
e450371 feat(hook): add plugin update check to SessionStart hook
b86ac54 docs: harmonize skill command naming to hyphenated form across 18 skills
b0c2d18 feat(plugin): add brief, mirror, tag, graph skills (v0.8.0)
dc3ccef refactor: delete dead plugin/ tree orphaned since marketplace switch
```

PR #100 (`feat/design-system`) also landed on main during this release window, but it's Tauri desktop app + landing site work, not plugin work. It'll ship under whatever future binary release tag Mat cuts next.

Diff for plugin-only scope: `git log dc3ccef..df1e6ac` from the repo root.
