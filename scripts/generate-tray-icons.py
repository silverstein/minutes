#!/usr/bin/env python3
"""Generate the Minutes macOS menubar tray icons.

Three states (per `tauri/src-tauri/src/main.rs:188-194`):

- `icon.png` — idle (template). Just the italic M on transparent. macOS tints
  this based on menubar appearance so a single solid color works for both
  dark and light menubars.
- `icon-recording.png` — full color. M + prominent red dot, ~25% canvas so
  it survives 44px rendering. The red is the recording semantic per
  DESIGN.md.
- `icon-live.png` — full color. M + prominent green dot in a different
  position so a glance distinguishes it from recording.

Renders at 88×88 (44pt @ 2x for retina menubar). Tauri sets these via
`tray.set_icon(...)` per state. For template icons (idle), `set_icon_as_template(true)`
makes macOS auto-tint.

Outputs to `tauri/src-tauri/icons/`. Run from repo root.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

REPO_ROOT = Path(__file__).resolve().parent.parent
ITALIC_TTF = REPO_ROOT / "tauri" / "src-tauri" / "dmg" / "fonts" / "InstrumentSerif-Italic.ttf"
ICONS_DIR = REPO_ROOT / "tauri" / "src-tauri" / "icons"

# DESIGN.md palette
CREAM = (232, 228, 218)  # #E8E4DA — used for non-template recording/live
BLACK = (0, 0, 0)        # template uses black silhouette; macOS auto-tints
RED = (255, 69, 58)      # #FF453A — recording state
GREEN = (48, 209, 88)    # #30D158 — live transcript distinguishing color

# 44pt menubar @ 2x retina = 88px. Apple Human Interface Guidelines.
SIZE = 88

# Glyph: italic M optically centered, leaving room for a state dot in the
# corner without crowding.
M_FONT_SIZE = 70

# Idle "brand dot" — small template-tinted dot upper-right. Re-establishes
# the brand identity (italic M alone is too generic; Monologue and others
# use the same letterform). Tiny enough that it doesn't dominate.
IDLE_DOT_DIAMETER = 8
IDLE_DOT_CX = SIZE - 12
IDLE_DOT_CY = 12

# Recording state: same upper-right position, but bigger and red. Reads
# as "the brand dot grew bright" — clear progression from idle.
RECORDING_DOT_DIAMETER = 20
RECORDING_DOT_CX = SIZE - 14
RECORDING_DOT_CY = 14

# Live transcript state: lower-right green dot. Position-difference (not
# just color) is the primary state cue so color-blind users and tinted
# menubars can still distinguish from recording at a glance. Color is
# the secondary cue.
LIVE_DOT_DIAMETER = 20
LIVE_DOT_CX = SIZE - 14
LIVE_DOT_CY = SIZE - 14


def render(
    font: ImageFont.FreeTypeFont,
    dot: tuple[tuple[int, int, int], int, int, int] | None,
    m_color: tuple[int, int, int],
) -> Image.Image:
    """Render one tray icon.

    Pass `dot=None` to skip drawing a dot.
    Otherwise `dot` is `(rgb_color, cx, cy, diameter)`.
    """
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img, "RGBA")

    # Draw the dot first so M's serif sits cleanly above it if they touch.
    if dot is not None:
        color, cx, cy, diameter = dot
        draw.ellipse(
            (
                cx - diameter // 2,
                cy - diameter // 2,
                cx + diameter // 2,
                cy + diameter // 2,
            ),
            fill=(*color, 255),
        )

    # Optically center the M. anchor='mm' uses the glyph bbox center.
    draw.text((SIZE // 2, SIZE // 2), "M", font=font, fill=(*m_color, 255), anchor="mm")
    return img


def main() -> None:
    if not ITALIC_TTF.exists():
        raise SystemExit(
            f"Missing italic font at {ITALIC_TTF}.\n"
            "Drop InstrumentSerif-Italic.ttf there before running."
        )

    ICONS_DIR.mkdir(parents=True, exist_ok=True)
    font = ImageFont.truetype(str(ITALIC_TTF), size=M_FONT_SIZE)

    # Idle: template-style. M + small "brand dot" upper-right, both black so
    # macOS auto-tints them together. Differentiates Minutes from other
    # italic-M tray icons (Monologue, etc) without dominating.
    idle = render(
        font,
        dot=(BLACK, IDLE_DOT_CX, IDLE_DOT_CY, IDLE_DOT_DIAMETER),
        m_color=BLACK,
    )
    idle_path = ICONS_DIR / "icon.png"
    idle.save(idle_path, format="PNG", optimize=True)
    print(f"Generated {idle_path} ({SIZE}x{SIZE}) — idle template, small brand dot upper-right")

    # Recording: cream M + RED dot upper-right (the brand dot grew bright).
    rec = render(
        font,
        dot=(RED, RECORDING_DOT_CX, RECORDING_DOT_CY, RECORDING_DOT_DIAMETER),
        m_color=CREAM,
    )
    rec_path = ICONS_DIR / "icon-recording.png"
    rec.save(rec_path, format="PNG", optimize=True)
    print(f"Generated {rec_path} ({SIZE}x{SIZE}) — recording, big red dot upper-right")

    # Live: cream M + GREEN dot lower-right. Position-difference (not just
    # color) is the primary state cue.
    live = render(
        font,
        dot=(GREEN, LIVE_DOT_CX, LIVE_DOT_CY, LIVE_DOT_DIAMETER),
        m_color=CREAM,
    )
    live_path = ICONS_DIR / "icon-live.png"
    live.save(live_path, format="PNG", optimize=True)
    print(f"Generated {live_path} ({SIZE}x{SIZE}) — live, big green dot lower-right")


if __name__ == "__main__":
    main()
