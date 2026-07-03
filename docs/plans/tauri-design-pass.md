# Plan: Tauri App Design System Alignment

> Bring the Minutes macOS Tauri app to the same quality bar as the landing page.

**Branch:** `feat/unified-design-system`
**Status:** Planning
**Approach:** Three phases — foundation cleanup, layout/hierarchy, polish. Phase 1 is CSS-only with zero layout restructuring.

---

## Context

The landing site now follows DESIGN.md. The Tauri app uses the correct token names and palette but has several direct violations (gradients, glows, wrong font weights) and flat typography hierarchy. The app reads as "competent dark-mode engineering CSS" rather than "intentionally designed editorial product."

Recent work already completed:
- note.html and meeting-prompt.html got a refinement pass
- Fonts bundled in tauri/src/fonts/
- Token names aligned to DESIGN.md across all HTML files

## Audit Summary

**Surfaces ranked by quality (best to worst):**
1. dictation-overlay.html — nearly done, minor weight cleanup
2. note.html — passable, minor gradient question
3. meeting-prompt.html — passable, same gradient question
4. terminal.html — functionally fine, wrong terminal font
5. index.html — weakest surface, needs structural work across phases

**Top 5 violations:**
1. `.btn-primary` in index.html uses a green-to-blue gradient + glow shadow
2. Instrument Serif rendered at weight 700 in multiple places (should be 400)
3. Terminal uses SF Mono/Menlo instead of Geist Mono
4. About card has a blue-tinted gradient background
5. Brand mark is a blue-gold CSS gradient icon inconsistent with product aesthetic

---

## Phase 1: Foundation Cleanup

**Goal:** Fix every direct DESIGN.md violation. CSS value changes only — no layout restructuring, no JS changes, no new components.

**Impact:** The app will look immediately more cohesive. Eliminates the "two different products" feeling.

### 1.1 Fix button gradient and glow

**File:** `tauri/src/index.html`
**What:** `.btn-primary` has `background: linear-gradient(135deg, var(--accent) 0%, #3d94c2 100%)` and `box-shadow: 0 0 12px rgba(48, 209, 88, 0.15)`. This introduces blue and glow.
**Change to:**
```css
.btn-primary {
  background: var(--accent);
  color: var(--bg);
  box-shadow: inset 0 -1px 0 rgba(0, 0, 0, 0.14);
}
.btn-primary:hover {
  background: var(--accent-hover);
  box-shadow: inset 0 -1px 0 rgba(0, 0, 0, 0.14);
}
.btn-primary:active {
  transform: scale(0.98);
}
```
**Why:** DESIGN.md explicitly says no gradients, no blue accent. The note.html and meeting-prompt.html already use the correct solid accent button style.

### 1.2 Fix Instrument Serif font weights

**File:** `tauri/src/index.html`
**What:** Multiple elements use `font-weight: 700` on Instrument Serif. The font is designed for weight 400 — 700 makes it look like a system serif fallback.
**Elements to fix:**
- `.brand-label` (line ~403): 700 → 400
- `.detail-title` (line ~1157): 700 → 400
- `.about-copy h2` (line ~1461): change to 400
- `.readiness-title` (line ~1607): 700 → 400
- `.recovery-title` (line ~1729): 700 → 400
- `.processing-title` (line ~778): already uses `--font-display` but is 600 — change to 400

**Why:** DESIGN.md specifies Instrument Serif at weight 400 for all heading roles. The landing page uses 400 everywhere.

### 1.3 Fix terminal font family

**File:** `tauri/src/terminal.html`
**What:** xterm.js Terminal constructor uses `fontFamily: "'SF Mono', Menlo, monospace"` (line ~189).
**Change to:** `fontFamily: "'Geist Mono', 'SF Mono', Menlo, monospace"`
**Risk:** May affect xterm.js character width calculations. Verify `fitAddon.fit()` still works correctly after change.
**Why:** DESIGN.md mandates Geist Mono for all mono/transcript/terminal contexts. The xterm theme file already uses the correct palette — the font is the only gap.

### 1.4 Fix about card background

**File:** `tauri/src/index.html`
**What:** `.about-card` (line ~1436) uses `background: linear-gradient(180deg, rgba(22, 32, 50, 0.96), rgba(16, 24, 38, 0.96))` — a blue-tinted gradient that creates a different color temperature.
**Change to:** `background: var(--bg-elevated)` with `border: 1px solid var(--border-mid)`
**Why:** No gradients rule. The blue tint is completely off-palette and makes the about card look like it belongs to a different product.

### 1.5 Simplify brand mark

**File:** `tauri/src/index.html`
**What:** `.brand-mark` (lines ~366-394) is a CSS-painted icon with blue-gold radial gradients and white pseudo-element bars. It's the most visually complex element in the app and communicates nothing about Minutes.
**Option A:** Replace with a simple Instrument Serif "M" in `--text` on a `--bg-elevated` circle/rounded-square.
**Option B:** Remove the icon entirely, keep only the "Minutes" wordmark (matching landing page approach).
**Decision needed:** Which option? Option B is simpler and more consistent with the landing page.
**Why:** The gradient icon contradicts the "no gradients, no decorative elements" rule and is the visual loudest thing in the header.

### 1.6 Fix dictation overlay font weight

**File:** `tauri/src/dictation-overlay.html`
**What:** `.label` uses `font-weight: 650` (line ~166). DESIGN.md defines Geist at 400, 500, or 600.
**Change to:** `font-weight: 600`
**Why:** 650 is not a defined weight in the type scale.

### 1.7 Fix device picker focus state

**File:** `tauri/src/index.html`
**What:** `.device-picker:focus` (line ~1914) uses `border-color: var(--blue)`. There is no `--blue` token.
**Change to:** `border-color: var(--accent)`
**Why:** The focus state references a nonexistent token. Should use accent like all other focus states.

---

## Phase 2: Layout & Hierarchy

**Goal:** Make the app feel art-directed. Requires layout changes but no new components.

**Depends on:** Phase 1 complete.

### 2.1 Redesign footer action bar

**Current problem:** 4-column grid (`1.3fr 0.55fr 1fr 0.7fr`) with device picker crammed inline. At 480px, everything is too tight and the primary action doesn't dominate.

**Proposed layout:**
- Record button: full width or dominant width, solid accent
- Secondary row or smaller buttons for Live, Quick Thought, Recall
- Device picker: move to a popover accessible from the Record button area, or to the settings/about panel

**Open question:** Is "Quick Thought" a first-class footer action? If it maps to dictation, consider unifying with the dictation header chip.

### 2.2 Fix meeting list date group hierarchy

**Current:** Date groups use Instrument Serif + `--orange`. This gives administrative metadata the editorial font while meeting titles get the UI font — hierarchy is backwards.
**Proposed:** Date groups → Geist Mono 10px uppercase `--text-tertiary`. Let meeting titles be the loudest element in the list.

### 2.3 Fix detail overlay section titles

**Current:** Section titles (ACTION ITEMS, SUMMARY) are uppercase Geist bold in `--text-tertiary` — too faint to scan.
**Proposed:** Geist Mono 10px uppercase `--accent` — matching landing page feature labels.

### 2.4 Improve empty states

**Current:** "⏳ Loading..." with emoji.
**Proposed:** Instrument Serif heading + one line of Geist body text. No emoji. Example: "No meetings yet" / "Record your first conversation to get started."

---

## Phase 3: Polish & State Refinement

**Goal:** Close remaining gaps in interaction design, spacing, and perceived quality.

**Depends on:** Phase 2 complete.

### 3.1 Recall panel visual integration
- More padding around embedded terminal
- Match header style to main header
- Subtle border-radius on terminal container

### 3.2 Hover/focus/selection states audit
- Consistent 0.15-0.2s ease transitions on all interactive elements
- Remove `!important` overrides and inline `style=""` attributes
- Ensure `:focus-visible` accent ring appears on all buttons

### 3.3 Recording state polish
- Consider lower opacity on audio viz bars (currently 0.6, may be too aggressive)
- Verify recording → processing → complete transition feels smooth

### 3.4 Window size review
- Current: 480x640 with min 380x480
- Proposed: evaluate 520x700 default — recording bar + audio viz + meeting list + footer gets tight at current size

### 3.5 Inline style and !important cleanup
- `.btn-icon` uses `!important` for border and background
- Footer has inline `style="display: flex; gap: 4px; align-items: center;"` on a wrapper div
- Clean these up for maintainability

---

## Open Questions (resolve before Phase 2)

1. **Light mode in the app?** All CSS hardcodes dark values. Should Phase 1 add `@media (prefers-color-scheme: light)` overrides? DESIGN.md says app respects system preference.
2. **Brand mark direction?** Wordmark only (Option B) or minimal icon (Option A)?
3. **Quick Thought placement?** Footer, tray menu, or header?
4. **Recall panel future?** Always xterm, or could it show structured meeting data?
5. **Atmospheric gradients in note/meeting-prompt?** Keep or remove?
6. **Window size?** Should default be taller?

---

## Files Affected

### Phase 1 (CSS-only)
- `tauri/src/index.html` — buttons, font weights, about card, brand mark, device picker focus
- `tauri/src/terminal.html` — xterm font family
- `tauri/src/dictation-overlay.html` — label font weight

### Phase 2
- `tauri/src/index.html` — footer layout, meeting list hierarchy, detail overlay, empty states

### Phase 3
- `tauri/src/index.html` — Recall panel, hover states, inline style cleanup
- `tauri/src-tauri/src/main.rs` — window size defaults (if changed)
- `tauri/src/vendor/xterm/minutes-theme.js` — no changes expected

### Not affected
- `tauri/src/note.html` — already passable
- `tauri/src/meeting-prompt.html` — already passable
- `site/` — already on design system
- `crates/` — no UI changes
