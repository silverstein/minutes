#!/usr/bin/env python3
"""
tag_apply.py — Atomic outcome tagging for a meeting's YAML frontmatter.

Used by the /minutes-tag skill to avoid Edit-tool fragility on unusual
frontmatter. Inserts or updates the `outcome:`, `outcome_note:` (optional),
and `tagged_at:` fields just before the closing `---` of the frontmatter,
preserving everything else exactly.

Usage:
    tag_apply.py <meeting_file.md> --outcome won [--note "Sarah committed to monthly"]

Output: writes the file back atomically (temp file + rename). Prints
        {"status": "ok", ...} JSON to stdout on success, or
        {"error": "..."} to stderr with non-zero exit on failure.

Requires: Python 3.8+, stdlib only. No YAML parser — does line-based edits
to keep formatting and ordering of existing fields stable.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import sys
from pathlib import Path

OUTCOME_FIELDS = ("outcome", "outcome_note", "tagged_at")


def find_frontmatter_bounds(lines: list[str]) -> tuple[int, int] | None:
    """Return (start_index_inclusive, end_index_exclusive) of frontmatter content
    lines (excluding the surrounding `---` markers). Returns None if no
    well-formed frontmatter block exists.
    """
    if not lines or lines[0].rstrip("\r\n") != "---":
        return None
    for i in range(1, len(lines)):
        if lines[i].rstrip("\r\n") == "---":
            return (1, i)
    return None


def is_outcome_field_line(line: str) -> bool:
    stripped = line.lstrip()
    if line[: len(line) - len(stripped)]:  # has leading whitespace → not top-level
        return False
    for field in OUTCOME_FIELDS:
        if stripped.startswith(f"{field}:"):
            return True
    return False


def yaml_quote(value: str) -> str:
    """Quote a value safely for YAML inline use. Uses JSON-style double quotes,
    which YAML accepts and which round-trip cleanly through any YAML parser.
    """
    return json.dumps(value, ensure_ascii=False)


def update_frontmatter(content: str, outcome: str, note: str | None) -> str:
    today = datetime.date.today().isoformat()

    # Build the canonical outcome lines we want to end up with.
    new_outcome_lines = [f"outcome: {outcome}\n"]
    if note:
        new_outcome_lines.append(f"outcome_note: {yaml_quote(note)}\n")
    new_outcome_lines.append(f"tagged_at: {today}\n")

    lines = content.splitlines(keepends=True)
    bounds = find_frontmatter_bounds(lines)

    if bounds is None:
        # No frontmatter at all — synthesize one.
        new_fm = ["---\n"] + new_outcome_lines + ["---\n"]
        if content and not content.startswith("\n"):
            new_fm.append("\n")
        return "".join(new_fm) + content

    start, end = bounds
    # Strip any pre-existing outcome field lines from the frontmatter body.
    cleaned_fm = [line for line in lines[start:end] if not is_outcome_field_line(line)]

    # Make sure the last cleaned line ends with a newline so our insertions sit cleanly.
    if cleaned_fm and not cleaned_fm[-1].endswith("\n"):
        cleaned_fm[-1] = cleaned_fm[-1] + "\n"

    new_lines = lines[:start] + cleaned_fm + new_outcome_lines + lines[end:]
    return "".join(new_lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("meeting_file", type=Path)
    parser.add_argument(
        "--outcome",
        required=True,
        help="Outcome tag (e.g. won, lost, stalled, great, noise, or a custom value)",
    )
    parser.add_argument(
        "--note",
        default=None,
        help="Optional one-line note about why",
    )
    args = parser.parse_args()

    if not args.meeting_file.exists():
        print(json.dumps({"error": f"file not found: {args.meeting_file}"}), file=sys.stderr)
        return 1
    if not args.meeting_file.is_file():
        print(json.dumps({"error": f"not a file: {args.meeting_file}"}), file=sys.stderr)
        return 1

    # Capture the original file mode BEFORE any writes so we can restore it
    # after the atomic replace. Without this, a meeting at 0600 (private) would
    # come back as 0644 (world-readable) because the temp file is created with
    # default perms — a real privacy regression for users who manually lock
    # down sensitive meetings. Caught by external code review.
    try:
        original_mode = args.meeting_file.stat().st_mode
    except OSError as exc:
        print(json.dumps({"error": f"stat failed: {exc}"}), file=sys.stderr)
        return 1

    try:
        content = args.meeting_file.read_text(encoding="utf-8")
    except Exception as exc:
        print(json.dumps({"error": f"read failed: {exc}"}), file=sys.stderr)
        return 1

    new_content = update_frontmatter(content, args.outcome, args.note)

    # Atomic write: write to a sibling temp file, copy the original's mode onto
    # it, then rename over the original. Setting the mode BEFORE the rename
    # means there's no window where the file exists with the wrong perms.
    temp = args.meeting_file.with_suffix(args.meeting_file.suffix + ".tmp")
    try:
        temp.write_text(new_content, encoding="utf-8")
        os.chmod(temp, original_mode)
        temp.replace(args.meeting_file)
    except Exception as exc:
        if temp.exists():
            try:
                temp.unlink()
            except OSError:
                pass
        print(json.dumps({"error": f"write failed: {exc}"}), file=sys.stderr)
        return 1

    print(
        json.dumps(
            {
                "status": "ok",
                "file": str(args.meeting_file),
                "outcome": args.outcome,
                "note": args.note,
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
