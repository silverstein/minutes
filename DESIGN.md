# Design System — Minutes

> Open-source conversation memory. Local, private, free forever.

## Product Context

- **What this is:** Local-first conversation memory layer for developers — captures meetings and voice memos via Whisper.cpp, makes them queryable via MCP tools for Claude, Codex, Gemini CLI
- **Who it's for:** Developers and power users who run AI assistants and care about privacy
- **Space:** Developer tools, AI tooling, local-first computing
- **Project type:** Two unified surfaces — marketing landing site (Next.js) + macOS menu bar app (Tauri)

## Aesthetic Direction

- **Direction:** Terminal With a Soul — Unix precision meets editorial humanity
- **Decoration level:** Minimal — the transcript output is the decoration
- **Mood:** The product should feel like it was built by someone who would never ship something embarrassing. Ink & Switch energy, not Notion energy. Archival permanence, not ephemeral SaaS.
- **Key design bet:** Light cream as the default (paper-like, archival) with warm-dark as the dark mode. Every other tool in this category defaults to dark — this is the deliberate departure.

## Typography

- **Display/Hero:** Instrument Serif — literary gravitas for a product about human conversations. The italic weight is used for emphasis within headings (e.g. "conversation *memory.*"). No other tool in this category uses a serif; this is intentional.
- **Body/UI:** Geist — Vercel's clean, technical sans-serif. Pairs naturally with Geist Mono. Used for all body text, labels, buttons, and navigation.
- **Transcript/code/mono:** Geist Mono — for all diarized output, timestamps, speaker labels, action items, YAML frontmatter, CLI examples, keyboard shortcuts.
- **Loading (landing site):** Google Fonts CDN for Instrument Serif + Geist + Geist Mono
- **Loading (Tauri app):** Bundled WOFF2 files in `tauri/src/fonts/` — offline-safe, no network dependency

**Scale (landing site):**

| Role | Font | Size | Weight |
|------|------|------|--------|
| Hero heading | Instrument Serif | 56-58px | 400 |
| Section heading | Instrument Serif | 28-32px | 400 |
| Feature title | Instrument Serif | 18-20px | 400 |
| Body | Geist | 15-17px | 400 |
| UI/labels | Geist | 13-14px | 400-600 |
| Mono labels | Geist Mono | 10-12px | 400-500 |
| Transcript | Geist Mono | 11-12px | 400 |

**Scale (Tauri app):**

| Role | Font | Size | Weight |
|------|------|------|--------|
| Meeting titles | Geist | 14px | 600 |
| Headings | Instrument Serif | 18-22px | 400 |
| Body | Geist | 13px | 400 |
| Timestamps/metadata | Geist Mono | 11-12px | 400 |
| Transcript output | Geist Mono | 11-12px | 400 |

## Color

Two-mode system. Light is the default on the landing site. The Tauri app respects system preference.

### Light Mode

```css
--bg:             #F8F4ED;   /* warm cream, paper-like */
--bg-elevated:    #EFEBE2;   /* card/panel lift */
--bg-hover:       #E8E2D8;   /* deeper surface, input backgrounds */
--border:         rgba(0,0,0,0.07);
--border-mid:     rgba(0,0,0,0.13);   /* more visible borders */
--text:           #1A1916;   /* near-black, warm undertone */
--text-secondary: #8C8880;   /* labels, metadata */
--text-tertiary:  #BDB9B0;   /* placeholders, timestamps */
--accent:         #C96B4E;   /* coral — CTAs, links, italic heading, speaker labels */
--accent-hover:   #B85A3E;   /* accent hover state */
--accent-soft:    rgba(201,107,78,0.10); /* accent background tint */
--red:            #C0392B;   /* recording indicator, errors, destructive */
--green:          #2E7D46;   /* success, complete */
```

### Dark Mode

```css
--bg:             #0D0D0B;   /* warm near-black, not pure #000 */
--bg-elevated:    #141412;   /* card/panel lift (Tauri: --bg-elevated) */
--bg-hover:       #1C1C1A;   /* elevated panels, Tauri primary surface */
--border:         rgba(255,255,255,0.06);
--border-mid:     #2A2924;   /* warm stone, more visible */
--text:           #E8E4DA;   /* cream off-white */
--text-secondary: #6B6760;   /* labels, metadata */
--text-tertiary:  #3D3B38;   /* placeholders, timestamps */
--accent:         #30D158;   /* green — CTAs, links, speaker labels */
--accent-hover:   #4ADE70;   /* accent hover state */
--accent-soft:    rgba(48,209,88,0.10);
--red:            #FF453A;   /* recording indicator, errors, destructive */
--red-bg:         rgba(255,69,58,0.12);
--green:          #30D158;   /* success (doubles as accent in dark mode) */
--live:           #30D158;   /* live transcript indicator */
--purple:         #BF5AF2;   /* AI/assistant state */
--orange:         #D4832A;   /* processing/pending state */
```

**Accent usage — restrained.** The accent should appear only on:
1. Italic or emphasized text in the hero heading
2. Speaker labels in transcript output
3. Feature/section label text (the small uppercase mono labels)
4. CTA button fill (primary action only)
5. Nav link hover state
6. Active/focus states on interactive elements

Never use the accent as a background for large areas or as decoration. It earns its attention by being rare.

**Recording red is separate.** The red (`--red`) is strictly semantic — recording state, errors, destructive actions. Never use it as a general accent.

## Spacing

- **Base unit:** 8px
- **Density:** Comfortable on landing, compact in the Tauri app

| Token | Value | Use |
|-------|-------|-----|
| 2xs | 4px | Icon gaps, inline spacing |
| xs | 8px | Between list items, small gaps |
| sm | 12-16px | Section padding, card interiors |
| md | 24px | Between sections, card-to-card |
| lg | 32-40px | Page sections |
| xl | 56-72px | Hero spacing, major section breaks |
| Page margin | 32px | Window/viewport edge to content |
| Max content width | 800px | Landing site |

## Layout

- **Approach:** Grid-disciplined — single column, centered, 800px max width on the landing site. The Tauri app uses a two-panel layout (list + detail) where applicable.
- **Composition first:** The landing page opens as a poster, not a document. Hero heading is the first thing that lands; the transcript card is the second. No above-the-fold feature grids.
- **Transcript as hero visual:** The actual diarized output (speaker labels, timestamps, action items) rendered in Geist Mono is the primary product demo. Competitors hide raw output. Minutes celebrates it.

## Border Radius

| Context | Value |
|---------|-------|
| Landing buttons | 5px — sharp, developer-coded |
| App buttons | 8px |
| Cards, panels | 8px (landing) / 10-12px (app) |
| Input fields | 6-8px |
| Pills, badges | 999px |
| Inline code | 3px |

Consistent rounding. Landing is slightly sharper than the app. Never fully squared, never fully bubbly.

## Motion

- **Approach:** Minimal-functional — only transitions that aid comprehension
- **Theme toggle:** 0.25s ease on background/color changes — the whole page transitions together
- **Hover states:** 0.15-0.2s ease
- **Panel open/close:** 0.2s ease-out
- **Recording pulse:** 1s ease-in-out, infinite — the only animation that loops

No bounces, no scroll-driven effects, no decorative entrance animations. Utility-grade motion.

**One sanctioned exception — product-honest reveals.** Motion that *demonstrates how the product actually behaves* is not decoration and is allowed, held to the same restraint as everything else. The landing hero's transcript card streams its lines in once on load, mirroring how Minutes streams real transcription — the same instinct as "show real progress (streamed partials), never spinners" under Interaction craft. Such reveals must: play once (never loop), finish under ~2.5s, use the standard 160–260ms ease per element, and be gated behind `prefers-reduced-motion: no-preference` so the reduce path renders the final state instantly. If a reveal doesn't teach the viewer something true about the product, it's decoration — cut it.

## Component Patterns

### Buttons

```
Primary:    bg: --accent, text: white (light) / #0D0D0B (dark), radius: 5px (landing) / 8px (app)
Ghost:      bg: transparent, text: --text-secondary, border: --border-mid, radius: same
Icon-only:  28×28px, hover: --bg-hover
```

### Transcript Cards / Meeting List Items

```
Container:  bg: --bg-elevated, border: 1px solid --border, radius: 8px
Header:     padding: 12px 18px, border-bottom: 1px solid --border
Body:       padding: 16-18px, font: Geist Mono 11-12px
Speaker:    color: --accent
Timestamp:  color: --text-tertiary
Action item: ☐ glyph, color: --accent
Divider:    1px solid --border between body and action items
```

### Feature Grid

```
Grid:        2-column, 1px gap, background: --border (creates border-like lines)
Cell:        bg: --bg, padding: 22-24px
Label:       Geist Mono 10px uppercase, color: --accent
Title:       Instrument Serif 18-20px, color: --text
Description: Geist 13px, color: --text-secondary
```

### Form Controls (app)

```
Background: --bg-hover
Border:     1px solid --border-mid
Radius:     6-8px
Padding:    6px 10px
Font:       Geist 13px
```

### Pills / Badges

```
Background: --accent-soft
Text:       --accent
Radius:     999px
Padding:    2px 8px
Font:       Geist Mono 10-11px
```

### Overlays / Panels (app)

```
Background:     --bg-hover
Border:         1px solid --border-mid
Backdrop-filter: blur(12px)
Radius:         12-14px
Shadow:         0 8px 32px rgba(0,0,0,0.3) — light / 0.5 — dark
```

## Recording States

```
IDLE        → tray icon: default
RECORDING   → tray icon: red dot, pulsing --red indicator, timer in Geist Mono
PROCESSING  → spinner with stage label in --text-secondary
COMPLETE    → --green check, file path in Geist Mono
ERROR       → --red indicator, error message, preserved capture path
```

Every state must be visible in every surface (tray, app, CLI output).

## Iconography

- No custom icon set. System emoji for status indicators.
- Recording: `●` (--red)
- Success: `✓` (--green)
- Error: `✗` (--red)
- Voice memo: `📱`
- Action item: `☐` / `☑`
- Ghost context: `👻`

## Accessibility

- Minimum contrast: WCAG AA (4.5:1 for body text, 3:1 for large text)
- Light mode: #1A1916 on #F8F4ED = ~16:1. Coral #C96B4E on #F8F4ED = ~3.8:1 (acceptable for large text/UI elements; avoid for small body copy)
- Dark mode: #E8E4DA on #0D0D0B = ~14:1. Green #30D158 on #0D0D0B = ~8.5:1
- All interactive elements keyboard-accessible
- Focus indicators: 2px solid --accent outline
- No information conveyed by color alone — always paired with text or icon

## Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-08 | Light cream (#F8F4ED) as landing default | Every tool in this category defaults to dark. Cream signals archival permanence and differentiates. |
| 2026-04-08 | Instrument Serif for headings | Only serif in the category — signals "conversations" not "code." Both voices in design consultation independently agreed. |
| 2026-04-08 | Coral accent in light (#C96B4E), green in dark (#30D158) | Warm/human for light mode, terminal/alive for dark mode. Accent is used only in 5 specific spots — never decoratively. |
| 2026-04-08 | Transcript output as hero visual | Real diarized output in Geist Mono replaces the existing DemoPlayer gif. The product's output is the aesthetic. |
| 2026-04-08 | Warm near-black (#0D0D0B) not pure black | Slightly warm undertone unifies dark mode with the cream light mode surfaces. |
| 2026-04-08 | No gradients, no glows, no illustration | Information density is the aesthetic. The only decoration is well-set type. |
| 2026-07-10 | Sanctioned "product-honest reveal" exception to the no-entrance-animation rule | Resolves the tension between "no entrance animations" (Motion) and "show streamed partials, never spinners" (Interaction craft). The hero transcript streams in once to mirror real transcription; reduce-motion renders it static. Reveals that don't demonstrate real product behavior remain banned. |

## Interaction craft

How the app feels, held to like the accent rule:

- **Feedback grants confidence, never demands attention.** Prove the system heard the user: live waveforms, quiet "saved 12:04" whispers, honest failure text. No modals for success, no toasts for routine.
- **Motion: 120-170ms, ease, physics-honest.** Panes slide because they go somewhere; done-states dissolve. Nothing animates for decoration. Always respect prefers-reduced-motion.
- **Spacing sits on the scale: 4/6/8/10/12/16/20px.** Off-scale values are bugs.
- **Status speaks in --text-meta mono.** Whisper, do not announce. Sounds, where they exist, are dry ticks, never melodies.
- **Speed is the first delight.** High-frequency actions must feel instant; show real progress (streamed partials), never spinners.
- **Warmth is budgeted like the accent color**: at most one human touch per surface, chosen deliberately.
- **UI changes ship with a human feel-test** in the installed dev app (see docs/checklists/pre-commit.md) covering timing and rhythm, not just correctness.
