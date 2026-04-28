#!/usr/bin/env python3
"""Generate the Minutes macOS/Windows app icon source.

Design: warm-dark rounded square (`#0D0D0B`, the dark-mode `--bg`) with a
cream italic 'M' set in Instrument Serif (`#E8E4DA`, the dark-mode `--text`)
and a small recording dot in the upper-right corner using the dark-mode
`--red` (`#FF453A`). Per DESIGN.md the red is reserved for recording state,
so its use here is semantic — the icon literally tells you the app records.

Outputs `tauri/src-tauri/icons/app-icon.png` at 1024x1024. Run
`cargo tauri icon tauri/src-tauri/icons/app-icon.png` afterwards to
regenerate `icon.icns`, `icon.ico`, and the size-set PNGs from this source.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

REPO_ROOT = Path(__file__).resolve().parent.parent
ITALIC_TTF = REPO_ROOT / "tauri" / "src-tauri" / "dmg" / "fonts" / "InstrumentSerif-Italic.ttf"
OUTPUT_PATH = REPO_ROOT / "tauri" / "src-tauri" / "icons" / "app-icon.png"

# DESIGN.md palette
WARM_DARK = (13, 13, 11)        # #0D0D0B — dark-mode --bg
CREAM     = (232, 228, 218)     # #E8E4DA — dark-mode --text
RED       = (255, 69, 58)       # #FF453A — dark-mode --red, recording state

SIZE = 1024
RADIUS = 230  # macOS Big Sur+ icon corner radius (~22% of side)
M_FONT_SIZE = 720
DOT_DIAMETER = 84
DOT_CX = SIZE - 168
DOT_CY = 168


def main() -> None:
    if not ITALIC_TTF.exists():
        raise SystemExit(
            f"Missing italic font at {ITALIC_TTF}.\n"
            "Drop InstrumentSerif-Italic.ttf there before running."
        )

    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img, "RGBA")

    # Rounded square base
    draw.rounded_rectangle(
        (0, 0, SIZE - 1, SIZE - 1), radius=RADIUS, fill=(*WARM_DARK, 255)
    )

    # Recording dot first so the M's right serif can sit cleanly above it if
    # they ever meet at smaller renders. No halo — the V3 direction was a
    # crisp dot that survives 32px without bleeding.
    draw.ellipse(
        (DOT_CX - DOT_DIAMETER // 2, DOT_CY - DOT_DIAMETER // 2,
         DOT_CX + DOT_DIAMETER // 2, DOT_CY + DOT_DIAMETER // 2),
        fill=(*RED, 255),
    )

    # Centered italic "M" in Instrument Serif, anchor "mm" for true optical
    # centering of the glyph bounding box.
    font = ImageFont.truetype(str(ITALIC_TTF), size=M_FONT_SIZE)
    draw.text((SIZE // 2, SIZE // 2), "M",
              font=font, fill=(*CREAM, 255), anchor="mm")

    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    img.convert("RGBA").save(OUTPUT_PATH, format="PNG", optimize=True)
    print(f"Generated {OUTPUT_PATH} ({SIZE}x{SIZE})")
    print("Next: cargo tauri icon tauri/src-tauri/icons/app-icon.png")


if __name__ == "__main__":
    main()
