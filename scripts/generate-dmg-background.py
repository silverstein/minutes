#!/usr/bin/env python3
"""Generate the DMG installer background for Minutes.

Design: cream paper (--bg #F8F4ED) with a subtle vertical luminance lift,
"Minutes" set in Instrument Serif near-black, a coral chevron arrow at ~50%
opacity between the title and the icon row, and a tracked uppercase mono
subtitle below the icons pulled from the landing page footer.

Output: tauri/src-tauri/dmg-background.png at 1320x800 (2x of the 660x400
Tauri DMG window) so Retina Macs render it crisp.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


REPO_ROOT = Path(__file__).resolve().parent.parent
TITLE_FONT_PATH = REPO_ROOT / "tauri" / "src-tauri" / "dmg" / "fonts" / "InstrumentSerif-Regular.ttf"
OUTPUT_PATH = REPO_ROOT / "tauri" / "src-tauri" / "dmg-background.png"

# 2x the DMG window (660x400 in tauri.conf.json bundle.macOS.dmg.windowSize).
OUTPUT_SIZE = (1320, 800)

# Palette pulled from DESIGN.md (light mode).
CREAM = (248, 244, 237)        # #F8F4ED — --bg
CREAM_LIFT = (252, 249, 244)   # gentle highlight at vertical center
NEAR_BLACK = (26, 25, 22)      # #1A1916 — --text
TEXT_SECONDARY = (140, 136, 128)  # #8C8880 — --text-secondary
CORAL = (201, 107, 78)         # #C96B4E — --accent

TITLE = "Minutes"
SUBTITLE = "LOCAL · OPEN SOURCE · FREE FOREVER"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate the Minutes DMG background.")
    parser.add_argument("--verbose", action="store_true", help="Print debug output.")
    return parser.parse_args()


def load_mono_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    """Prefer SF Mono, fall back to SF/Helvetica. SF Mono is on every Mac and
    pairs cleanly with Instrument Serif at small tracked sizes."""
    for candidate in (
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ):
        if Path(candidate).exists():
            return ImageFont.truetype(candidate, size=size)
    return ImageFont.load_default()


def paint_background(img: Image.Image) -> None:
    """Paint cream with a subtle vertical luminance lift toward the icon row.

    No visible vignette — just a gentle shift that nudges the eye to the
    center where the app icon and Applications symlink sit.
    """
    width, height = img.size
    pixels = img.load()
    for y in range(height):
        # Bell curve peaking at vertical center of the icon row (slightly above
        # geometric center to balance the title's optical weight).
        t = y / height
        brightness = max(0.0, 1.0 - ((t - 0.55) / 0.55) ** 2)
        for x in range(width):
            hx = abs(x - width // 2) / (width // 2)
            h_fade = max(0.0, 1.0 - hx ** 1.8)
            blend = brightness * h_fade * 0.65
            r = int(CREAM[0] + (CREAM_LIFT[0] - CREAM[0]) * blend)
            g = int(CREAM[1] + (CREAM_LIFT[1] - CREAM[1]) * blend)
            b = int(CREAM[2] + (CREAM_LIFT[2] - CREAM[2]) * blend)
            pixels[x, y] = (r, g, b, 255)


def draw_arrow(draw: ImageDraw.ImageDraw, cy: int, width: int) -> None:
    """Lucide-style move-right chevron: horizontal stem + open chevron head.

    Sits between the app icon (left) and the Applications symlink (right).
    Rendered in coral at 50% opacity so it points without shouting.
    """
    arrow_color = (*CORAL, int(255 * 0.50))

    cx = width // 2
    half_w = 60
    chevron_size = 18

    draw.line((cx - half_w, cy, cx + half_w, cy), fill=arrow_color, width=3)
    draw.line(
        (cx + half_w - chevron_size, cy - chevron_size, cx + half_w, cy),
        fill=arrow_color, width=3,
    )
    draw.line(
        (cx + half_w - chevron_size, cy + chevron_size, cx + half_w, cy),
        fill=arrow_color, width=3,
    )


def main() -> None:
    args = parse_args()

    if not TITLE_FONT_PATH.exists():
        raise SystemExit(
            f"Missing title font at {TITLE_FONT_PATH}.\n"
            "Drop InstrumentSerif-Regular.ttf there before running."
        )

    canvas = Image.new("RGBA", OUTPUT_SIZE, (*CREAM, 255))
    paint_background(canvas)

    draw = ImageDraw.Draw(canvas, "RGBA")

    title_font = ImageFont.truetype(str(TITLE_FONT_PATH), size=110)
    subtitle_font = load_mono_font(22)

    # --- Title: Instrument Serif, near-black, optical center at y=140 (2x) ---
    title_bbox = draw.textbbox((0, 0), TITLE, font=title_font)
    title_w = title_bbox[2] - title_bbox[0]
    title_h = title_bbox[3] - title_bbox[1]
    title_y_offset = title_bbox[1]
    title_cy = 140
    title_y = title_cy - title_h // 2 - title_y_offset
    title_x = (OUTPUT_SIZE[0] - title_w) // 2
    draw.text((title_x, title_y), TITLE, fill=(*NEAR_BLACK, 255), font=title_font)

    # --- Subtitle: tracked uppercase mono, mirrors title's top padding ---
    tracked_subtitle = "  ".join(SUBTITLE.split())
    # Re-join with single-space inside each word, double-space between words,
    # then expand letter spacing across the whole string.
    tracked_subtitle = " ".join(c if c != " " else " " for c in SUBTITLE)
    # Light letter-tracking: insert a hair-space-equivalent (regular space)
    # between each character. Mono fonts handle this cleanly.
    tracked_subtitle = " ".join(SUBTITLE)

    subtitle_bbox = draw.textbbox((0, 0), tracked_subtitle, font=subtitle_font)
    subtitle_w = subtitle_bbox[2] - subtitle_bbox[0]
    subtitle_h = subtitle_bbox[3] - subtitle_bbox[1]
    subtitle_y_offset = subtitle_bbox[1]
    subtitle_cy = OUTPUT_SIZE[1] - 80  # 80px from bottom (2x = 40 logical)
    subtitle_y = subtitle_cy - subtitle_h // 2 - subtitle_y_offset

    subtitle_layer = Image.new("RGBA", OUTPUT_SIZE, (0, 0, 0, 0))
    subtitle_draw = ImageDraw.Draw(subtitle_layer)
    subtitle_draw.text(
        ((OUTPUT_SIZE[0] - subtitle_w) // 2, subtitle_y),
        tracked_subtitle,
        fill=(*TEXT_SECONDARY, 255),
        font=subtitle_font,
    )
    canvas.alpha_composite(subtitle_layer)

    # --- Arrow: vertically centered on the icon row (Tauri default y=200 logical = 400 in 2x) ---
    arrow_cy = 400
    draw_arrow(draw, arrow_cy, OUTPUT_SIZE[0])

    if args.verbose:
        print(f"  Title @ y={title_y} (h={title_h})")
        print(f"  Arrow @ y={arrow_cy}")
        print(f"  Subtitle @ y={subtitle_y} (h={subtitle_h})")

    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    canvas.convert("RGB").save(OUTPUT_PATH, format="PNG", dpi=(144, 144), optimize=True)

    print(f"Generated {OUTPUT_PATH} ({OUTPUT_SIZE[0]}x{OUTPUT_SIZE[1]} @144 DPI)")


if __name__ == "__main__":
    main()
