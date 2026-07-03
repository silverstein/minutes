# Dictation UX Improvements Plan

## Problem Statement

The dictation engine is excellent (streaming whisper, VAD, PID locking, audio cues) but the user experience has a trust gap: users never see their words appearing, never get a preview of what was captured, and face unnecessary settings complexity.

Current rating: 6/10. Goal: 8+/10.

## Current State

### What works well
- **Engine**: Streaming whisper with VAD, proper conflict resolution, failed audio recovery
- **HUD overlay**: 320x60 dark pill, bottom-right, always-on-top, status dot + waveform + timer
- **Audio cues**: Procedurally generated, musically designed, 4 states
- **Dual activation**: Standard shortcut (Cmd+Shift+Space) + raw key hotkey (Caps Lock)
- **CLI**: Shows streaming partial text beautifully

### Key UX gaps
1. Overlay doesn't show partial text (backend emits `dictation:partial` but overlay ignores it)
2. No text preview on success — "Copied" then gone, user doesn't know what was captured
3. Two shortcut systems with 6+ controls is confusing
4. No dictation history in the main app
5. No visual silence countdown
6. Clipboard not implemented on Windows

## Proposed Changes

### 1. Text-primary overlay with streaming text (highest impact)

The overlay's primary content shifts from status labels to the user's actual words
once speech begins. Status/waveform/timer become secondary context.

**Overlay states and dimensions:**

```
LISTENING (320×48):
┌──────────────────────────────────────┐
│  [● blink]  Listening...             │
└──────────────────────────────────────┘
  Compact pill. Red dot pulses (1.6s ease-in-out).
  Label: 16px --font-sans, weight 650, --text.

DICTATING (320×88):
┌──────────────────────────────────────┐
│  [●]  Dictating       [waveform] 0:03│
│  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ │
│  the quick brown fox jumps over...   │
└──────────────────────────────────────┘
  Pill expands (0.2s ease-out) to accommodate text line.
  Status row: same as current (dot + label + waveform + timer).
  Divider: 1px rgba(255,255,255,0.06).
  Text row: 13px --font-sans, weight 400, --text-secondary.
  Max 2 lines, overflow: hidden, text-overflow: ellipsis.
  Text updates via `dictation:partial` events with soft opacity fade (0.15s).
  If no partial text yet, show "..." placeholder in --text-tertiary.

PROCESSING (320×88):
┌──────────────────────────────────────┐
│  [spinner]  Dictating          0:05  │
│  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ │
│  the quick brown fox jumps over...   │
└──────────────────────────────────────┘
  Seamless transition — label stays "Dictating", only indicator changes
  to spinner. Waveform hidden, timer frozen at last value. Text preserved.
  This makes processing feel like a natural beat, not a loading screen.

SUCCESS (320×88):
┌──────────────────────────────────────┐
│  [✓ green]  Copied to clipboard      │
│  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ │
│  the quick brown fox jumps over the  │
└──────────────────────────────────────┘
  Final text shown (replaces partial). Green checkmark.
  Auto-dismiss after 2.5s. Click anywhere to dismiss early.
  Audio cue: complete (existing).

ERROR (320×64):
┌──────────────────────────────────────┐
│  [✗ red]  Failed — audio saved       │
└──────────────────────────────────────┘
  No text row (nothing to show). Red border glow.
  Auto-dismiss after 4s. Audio cue: error (existing).
```

**Resize behavior (CSS-only, no Tauri window resize):**
- Overlay window created at 88px height (max size). Always stays 88px.
- Pill CSS animates between 48px and 88px height within the fixed window.
- When pill is 48px, upper 40px of window is transparent empty space.
- This eliminates Tauri `set_size()` + `set_position()` race conditions.
- Pill is anchored to the bottom of the window via `position: absolute; bottom: 0`.
- Transition: 0.2s ease-out on pill height via CSS.

**Text edge cases:**
- Empty partial: show "..." placeholder.
- Very long text (>2 lines): overflow hidden + text-overflow ellipsis on last line.
- Multi-utterance session: after each utterance succeeds, pill collapses to 48px
  (listening), then expands again on next speech. Each utterance is independent.
- Rapid partials (<100ms apart): debounce DOM updates to 150ms to prevent flicker.

### 2. Interaction state coverage

```
  STATE       | SIZE    | INDICATOR    | LABEL              | TEXT ROW  | AUDIO CUE
  ------------|---------|------------- |--------------------|-----------|----------
  LOADING     | 320×48  | spinner      | "Loading model..." | hidden    | none
  LISTENING   | 320×48  | ● red blink  | "Listening..."     | hidden    | start
  DICTATING   | 320×88  | ● red solid  | "Dictating"        | partials  | none
  PROCESSING  | 320×88  | spinner      | "Dictating" (kept) | last text | none
  SUCCESS     | 320×88  | ✓ green      | "Copied"           | final     | complete
  ERROR       | 320×64  | ✗ red        | "Failed — saved"   | hidden    | error
  CANCELLED   | dismiss | —            | —                  | —         | stop
  YIELDED     | dismiss | —            | —                  | —         | stop
```

**Loading state (new):** Overlay appears immediately on shortcut press with spinner +
"Loading model..." label. Transitions to Listening once whisper model is initialized.
This eliminates the 1-3s dead gap on first use where nothing is visible.

**Conflict states:**
- Recording already active → "Recording in progress" error toast (no overlay).
- Another dictation active → "Dictation already running" error toast (no overlay).
- Model missing → "Model not found — run setup" error, dismiss after 4s.

### User Journey — Emotional Arc

```
  STEP | USER DOES                | USER FEELS           | DESIGN SUPPORTS IT WITH
  -----|--------------------------|----------------------|---------------------------
  1    | Presses Cmd+Shift+Space  | "Did it work?"       | Instant overlay appearance (loading state)
  2    | Sees "Listening..." pill  | "OK, it heard me"    | Start audio cue + red pulsing dot
  3    | Starts speaking           | "Is it getting this?" | Words appear live in text row
  4    | Pauses mid-thought        | "Still there?"        | Dot still shows, text stays visible
  5    | Finishes speaking          | "What did it get?"   | Text stays put, subtle spinner swap
  6    | Sees "Copied" + text      | "Got it, looks right" | Green check + final text confirmation
  7    | Pill dismisses             | Back to work          | 2.5s auto-dismiss, unobtrusive
```

The critical insight: steps 3-5 are where the current UX fails. The user speaks
into a black box and only finds out what was captured when they paste. With
streaming text, every step from 3 onward provides continuous confirmation.

### 3. Simplify shortcut settings

**Current (6 controls, confusing):**
```
Standard shortcut:    [Cmd+Shift+Space ▼]  [Off]
Raw key hotkey:       [Rec: Caps Lock]     [Off]
```
Two toggles that can both be on/off independently. Users don't know which to use.

**Proposed (merged, 1 dropdown + 1 toggle):**
```
DICTATION settings:

  Shortcut              [Cmd+Shift+Space ▼]  [On]
  ┈ Standard shortcut system. Recommended.

  When "Hold key" option is selected:
  Shortcut              [Hold: Caps Lock ▼]  [Rec]  [On]
  ┈ Hold to dictate, tap to lock/unlock. Requires Input Monitoring.
```

**Dropdown options (unified list):**
```
  ─── Standard shortcuts ───
  Cmd+Shift+Space         (default)
  Cmd+Option+Space
  Cmd+Shift+D
  ─── Advanced (hold key) ───
  Hold: Caps Lock
  Hold: [Record new key]
```

**Behavior:**
- One dropdown, one toggle. Selecting any option updates the underlying system.
- Standard shortcuts use `tauri_plugin_global_shortcut`. Hold keys use `HotkeyMonitor`.
- [Rec] button appears only when an advanced "Hold:" option is selected.
- Conflict detection: if selected shortcut matches the quick-thought shortcut,
  show inline error "Already used by quick-thought capture" and disable the toggle.
- Persisted config: `dictation.shortcut` stores the value, `dictation.shortcut_enabled`
  stores on/off.
- **Backend dispatch (value-prefix):** Values starting with "Hold:" route to
  `HotkeyMonitor` (raw keycode monitoring). All other values route to
  `tauri_plugin_global_shortcut` (standard keyboard shortcuts).
- When switching types: deactivate the old system before activating the new one.

**Required tests:**
1. Value-prefix dispatch routes correctly ("Hold:57" → HotkeyMonitor, "CmdOrCtrl+Shift+Space" → global_shortcut)
2. Switching from standard to hold-key deactivates old system first
3. Conflict detection rejects shortcuts that match quick-thought capture
4. Windows `write_to_clipboard` uses `clip.exe` subprocess correctly

### 4. Dictation history in main app

Add "Dictation" as a filter option on the existing meetings list.

**Implementation:**
- Dictation files already have `type: dictation` in YAML frontmatter.
- Add "Dictation" to the type filter dropdown alongside "Meeting" and "Memo".
- Dictation items use the existing meeting list item component:
  - Title: first 8 words (already stored in frontmatter `title`).
  - Badge: "Dictation" pill using DESIGN.md pill pattern (accent bg, accent text).
  - Metadata: date + duration, using 12px --font-sans --text-secondary.
  - Click: expands to show full dictation text.
  - Action: copy-to-clipboard icon button on hover (existing icon-only button pattern).
- No new section, no new tab. Zero new components — filter + badge only.

### 5. Windows clipboard support
Implement `write_to_clipboard` for Windows using `clip.exe` (same pattern as macOS `pbcopy`).

### 6. Silence countdown indicator

Thin progress bar at the bottom of the pill, inside the border radius.

**Spec:**
- Height: 2px. Color: --accent at 30% opacity.
- Position: absolute bottom of pill, inside border-radius (clipped by overflow).
- Behavior: starts full-width when silence begins (after speech ends), shrinks
  right-to-left over `silence_timeout_ms`. When it reaches zero, session ends.
- Only visible during the "listening after speech" phase (has_spoken && !was_speaking).
- Not shown during initial listening (before any speech detected).
- Animation: CSS width transition, linear timing function matching the timeout duration.

### 7. Accessibility

**Overlay:**
- Pill element: `role="status"` + `aria-live="polite"` — screen readers announce
  state changes ("Listening", "Dictating", "Copied to clipboard").
- Text row: included in the live region, so transcribed text is announced.
- Esc key: already handled for cancel.
- No tab navigation needed (overlay is non-interactive except Esc).

**Settings:**
- All form controls already keyboard-accessible (native select, button).
- Merged shortcut dropdown: ensure [Rec] button is keyboard-focusable when visible.
- Toggle buttons: ensure `aria-pressed` state is set.

**Color contrast:**
- Text row (#f5f5f7 on rgba(28,28,34,0.96)) = ~15:1 contrast ratio. Passes AAA.
- Secondary text (rgba(238,236,231,0.62) on panel) = ~7:1. Passes AA.
- All indicator colors (red, green, blue) paired with text labels — no color-only info.

## Priority Order
1. Streaming text in overlay (transforms the experience)
2. Success state text preview (builds trust)
3. Windows clipboard (unblocks Windows users)
4. Simplify shortcut settings (reduces confusion)
5. Dictation history (adds review capability)
6. Silence countdown (polish)

## Design System Notes

**Overlay token exception:** The dictation overlay uses its own color tokens that
intentionally diverge from DESIGN.md. The floating transparent HUD needs different
treatment than in-app panels. Document in DESIGN.md under "Overlay Exceptions":
```css
/* Dictation overlay (floating HUD) */
--panel: rgba(28, 28, 34, 0.96);   /* vs --bg-elevated: #2c2c2e */
--panel-edge: rgba(255, 255, 255, 0.08);
--accent: #79a9ff;                  /* softer blue vs --accent: #0a84ff */
```

**Existing patterns reused:**
- Meeting list items → dictation history entries
- Pills/badges → dictation type label
- Form controls → settings dropdowns, toggles
- Animation timings → 0.2s ease-out for overlay transitions

## NOT in scope

- **LLM cleanup of dictation text** — the plan's `cleanup_engine` config exists but
  post-processing AI cleanup is a separate feature (different latency/privacy tradeoffs).
- **Dictation in non-Tauri contexts** — CLI already has streaming text. MCP server
  can start/stop dictation. This plan focuses on the desktop overlay only.
- **Multi-language switching** — dictation inherits the whisper language from config.
  Per-utterance language detection is a separate feature.
- **Dictation text editing** — the overlay is read-only. If the user wants to edit,
  they paste and fix in their target app. In-overlay editing would change the component class entirely.

## Technical Notes

- `dictation:partial` events already emitted by backend — overlay just needs to listen.
- `dictation:level` events also emitted — waveform already consumes them.
- Overlay window: create at fixed 88px height. Pill animates via CSS (no Tauri resize API).
  Update `show_dictation_overlay()` to use `inner_size(320, 88)` instead of `(320, 60)`.
- Main app meeting list already has filtering infrastructure — add dictation type.
- Windows clipboard: `clip.exe` accepts stdin, same pattern as `pbcopy`.
- Loading state: requires overlay to appear before `dictation::run()` starts.
  Current flow: `start_dictation_session()` calls `show_dictation_overlay()` then spawns
  the dictation thread. Overlay already appears first — just need to emit a "loading"
  event before the whisper model is initialized.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | — | — |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | — | — |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 1 | CLEAR | 2 issues, 0 critical gaps |
| Design Review | `/plan-design-review` | UI/UX gaps | 1 | CLEAR | score: 5/10 → 8/10, 6 decisions |

**UNRESOLVED:** 0
**VERDICT:** DESIGN + ENG CLEARED — ready to implement.
