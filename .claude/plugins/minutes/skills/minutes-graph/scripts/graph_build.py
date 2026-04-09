#!/usr/bin/env python3
"""
graph_build.py — Build the cross-meeting entity graph index from real
meeting frontmatter, augmented with `minutes people --json` data.

Used by the /minutes-graph skill to deterministically build/refresh the
graph index without asking Claude to walk hundreds of files in-context.

Usage:
    graph_build.py [--meetings-dir DIR] [--output FILE] [--incremental]

Output: writes the JSON index to ~/.minutes/graph/index.json by default
        (or to --output PATH). Prints a one-line summary JSON to stdout.

Requires: Python 3.8+, stdlib only. The `minutes` CLI must be on PATH.

Frontmatter parsing strategy: this script does NOT use a full YAML parser.
It does line-based extraction of the specific fields graph cares about
(`date`, `attendees`, `tags`, `people`, `decisions[].topic`). This avoids
adding a PyYAML dependency and is robust to the slight schema variations
real meetings exhibit. Other frontmatter fields are left untouched.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

DATE_RE = re.compile(r"^date:\s*(.+?)\s*$")
TAGS_RE = re.compile(r"^tags:\s*\[(.*)\]\s*$")
ATTENDEES_RE = re.compile(r"^attendees:\s*\[(.*)\]\s*$")
PEOPLE_LINE_RE = re.compile(r"^people:\s*(.+)$")
WIKILINK_SLUG_RE = re.compile(r"\[([^\[\]]+)\]")
DECISIONS_HEADER_RE = re.compile(r"^decisions:\s*$")
DECISION_TOPIC_RE = re.compile(r"^\s+topic:\s*(.+?)\s*$")

GARBAGE_SLUG_RE = re.compile(r"^(unknown[-_]?speaker|speaker[-_]?\d*|unknown)$", re.IGNORECASE)


def parse_inline_array(value: str) -> list[str]:
    """Parse a YAML inline array body like 'a, b, c' into ['a', 'b', 'c'].
    Strips surrounding quotes if any."""
    items = []
    for raw in value.split(","):
        item = raw.strip().strip('"').strip("'")
        if item:
            items.append(item)
    return items


def extract_frontmatter_lines(content: str) -> list[str]:
    """Return the lines between the first two `---` markers, or [] if missing."""
    lines = content.splitlines()
    if not lines or lines[0].rstrip() != "---":
        return []
    for i in range(1, len(lines)):
        if lines[i].rstrip() == "---":
            return lines[1:i]
    return []


NULL_TOPIC_VALUES = {"null", "~", "none", ""}


def extract_meeting_entities(file_path: Path) -> dict:
    """Pull date, people slugs, attendee names, and topics out of one meeting file."""
    try:
        content = file_path.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        return {"filename": file_path.name, "error": str(exc)}

    fm = extract_frontmatter_lines(content)
    date = None
    attendees: list[str] = []
    tags: list[str] = []
    people_slugs: list[str] = []
    decision_topics: list[str] = []

    in_decisions = False
    for line in fm:
        if not in_decisions:
            m = DATE_RE.match(line)
            if m:
                date = m.group(1)
                continue
            m = TAGS_RE.match(line)
            if m:
                tags = parse_inline_array(m.group(1))
                continue
            m = ATTENDEES_RE.match(line)
            if m:
                attendees = parse_inline_array(m.group(1))
                continue
            m = PEOPLE_LINE_RE.match(line)
            if m:
                people_slugs.extend(WIKILINK_SLUG_RE.findall(m.group(1)))
                continue
            if DECISIONS_HEADER_RE.match(line):
                in_decisions = True
                continue
        else:
            # Inside the decisions block: look for `topic: X` lines under each entry.
            m = DECISION_TOPIC_RE.match(line)
            if m:
                topic = m.group(1).strip().strip('"').strip("'")
                if topic and topic.lower() not in NULL_TOPIC_VALUES:
                    decision_topics.append(topic.lower())
                continue
            # Top-level field starts with no leading space and ends decisions block.
            if line and not line[0].isspace() and not line.startswith("-"):
                in_decisions = False

    # Filter null-like tags too — same noise can appear in tags arrays.
    clean_tags = {t.lower() for t in tags if t.lower() not in NULL_TOPIC_VALUES}
    topics = sorted(clean_tags | set(decision_topics))

    deduped_slugs = list(dict.fromkeys(people_slugs))

    # Build positional slug→display_name mapping when lengths match.
    # Real meetings typically have `attendees: [Case W., Mat S.]` and
    # `people: [[case-wintermute], [mat]]` in the same order. If they don't
    # match in length, we can't safely associate them — leave names empty.
    slug_to_display: dict[str, str] = {}
    if len(attendees) == len(deduped_slugs):
        for slug, display in zip(deduped_slugs, attendees):
            slug_to_display[slug] = display

    return {
        "filename": file_path.name,
        "date": date,
        "people_slugs": deduped_slugs,
        "attendees": attendees,
        "slug_to_display": slug_to_display,
        "topics": topics,
    }


def fetch_minutes_people() -> list[dict]:
    """Run `minutes people --json` and parse it. Returns [] on any failure."""
    try:
        result = subprocess.run(
            ["minutes", "people", "--json"],
            capture_output=True,
            text=True,
            timeout=15,
        )
        if result.returncode != 0:
            return []
        return json.loads(result.stdout)
    except (FileNotFoundError, subprocess.SubprocessError, json.JSONDecodeError):
        return []


def is_garbage_slug(slug: str, self_slug: str = "") -> bool:
    if not slug:
        return True
    if GARBAGE_SLUG_RE.match(slug):
        return True
    return False


def empty_entity() -> dict:
    return {
        "name": "",
        "aliases": [],
        "meetings": [],
        "first_mention": None,
        "last_mention": None,
        "count": 0,
        "co_occurs_with": {"people": {}, "topics": {}},
    }


def update_first_last(entry: dict, date: str | None) -> None:
    if not date:
        return
    if entry["first_mention"] is None or date < entry["first_mention"]:
        entry["first_mention"] = date
    if entry["last_mention"] is None or date > entry["last_mention"]:
        entry["last_mention"] = date


def increment_co_occurrence(target: dict, axis: str, key: str) -> None:
    target[axis][key] = target[axis].get(key, 0) + 1


def build_index(meetings_dir: Path) -> dict:
    meeting_files = sorted(meetings_dir.glob("*.md"))
    parsed = [extract_meeting_entities(p) for p in meeting_files]

    people_index: dict[str, dict] = {}
    topics_index: dict[str, dict] = {}

    for meeting in parsed:
        if meeting.get("error"):
            continue
        date = meeting["date"]
        slugs = meeting["people_slugs"]
        topics = meeting["topics"]
        attendees = meeting["attendees"]

        # Walk people in this meeting
        for slug in slugs:
            entry = people_index.setdefault(slug, empty_entity())
            if meeting["filename"] not in entry["meetings"]:
                entry["meetings"].append(meeting["filename"])
                entry["count"] += 1
            update_first_last(entry, date)
            # Add THIS slug's positional display name as an alias (if we have one).
            display = meeting.get("slug_to_display", {}).get(slug)
            if display and display not in entry["aliases"]:
                entry["aliases"].append(display)

        # Walk topics in this meeting
        for topic in topics:
            entry = topics_index.setdefault(topic, empty_entity())
            if meeting["filename"] not in entry["meetings"]:
                entry["meetings"].append(meeting["filename"])
                entry["count"] += 1
            update_first_last(entry, date)

        # Co-occurrence: every pair of entities in this meeting
        for i, p1 in enumerate(slugs):
            for p2 in slugs[i + 1 :]:
                increment_co_occurrence(people_index[p1]["co_occurs_with"], "people", p2)
                increment_co_occurrence(people_index[p2]["co_occurs_with"], "people", p1)
            for t in topics:
                increment_co_occurrence(people_index[p1]["co_occurs_with"], "topics", t)
                increment_co_occurrence(topics_index[t]["co_occurs_with"], "people", p1)
        for i, t1 in enumerate(topics):
            for t2 in topics[i + 1 :]:
                increment_co_occurrence(topics_index[t1]["co_occurs_with"], "topics", t2)
                increment_co_occurrence(topics_index[t2]["co_occurs_with"], "topics", t1)

    # Augment people from `minutes people --json`
    minutes_people = fetch_minutes_people()
    self_slug = ""
    for mp in minutes_people:
        if mp.get("source") == "self-enrollment":
            self_slug = mp.get("slug", "")
            break

    for mp in minutes_people:
        slug = mp.get("slug")
        if not slug or slug not in people_index:
            continue
        entry = people_index[slug]
        # Augment fields the CLI knows about
        entry["top_topics"] = mp.get("top_topics", [])
        entry["open_commitments"] = mp.get("open_commitments", 0)
        entry["score"] = mp.get("score", 0)
        entry["losing_touch"] = mp.get("losing_touch", False)
        # Merge the CLI's name as an alias (it may be lowercased/garbled — that's fine,
        # it's just one more variant the user might type)
        cli_name = mp.get("name") or ""
        if cli_name and cli_name not in entry["aliases"]:
            entry["aliases"].append(cli_name)

    # Pick a canonical display name for each person. Prefer aliases that look
    # human: capital letter + space (e.g., "Mat S.", "Sarah Chen"). If none
    # qualify, prefer aliases with any uppercase letter (e.g., "Mat"). Final
    # fallback is the longest alias. Ties broken alphabetically for determinism.
    def display_name_score(alias: str) -> tuple[int, int, str]:
        looks_human = 1 if (any(c.isupper() for c in alias) and " " in alias) else 0
        has_upper = 1 if any(c.isupper() for c in alias) else 0
        return (-looks_human, -has_upper, -len(alias), alias)

    for entry in people_index.values():
        if entry["aliases"]:
            entry["name"] = sorted(entry["aliases"], key=display_name_score)[0]
        else:
            entry["name"] = ""  # let the consumer fall back to the slug

    # Filter garbage from people index (diarization noise)
    cleaned_people = {
        slug: entry
        for slug, entry in people_index.items()
        if not is_garbage_slug(slug, self_slug)
    }

    return {
        "version": 1,
        "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "built_at_unix": int(time.time()),
        "meeting_count": len(meeting_files),
        "people": cleaned_people,
        "topics": topics_index,
    }


def detect_meetings_dir() -> Path | None:
    """Ask `minutes paths` for the canonical output_dir."""
    try:
        result = subprocess.run(
            ["minutes", "paths"], capture_output=True, text=True, timeout=5
        )
    except (FileNotFoundError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    for line in result.stdout.splitlines():
        if line.startswith("output_dir:"):
            return Path(line.split(":", 1)[1].strip())
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--meetings-dir",
        type=Path,
        help="Meetings directory (defaults to `minutes paths` output_dir)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output index path (default: ~/.minutes/graph/index.json)",
    )
    parser.add_argument(
        "--incremental",
        action="store_true",
        help="If the index already exists and no meeting files are newer than its built_at, exit without rebuilding",
    )
    args = parser.parse_args()

    meetings_dir = args.meetings_dir or detect_meetings_dir()
    if not meetings_dir:
        print(
            json.dumps(
                {
                    "error": "could not determine meetings dir",
                    "hint": "pass --meetings-dir or run `minutes paths`",
                }
            ),
            file=sys.stderr,
        )
        return 1
    if not meetings_dir.is_dir():
        print(json.dumps({"error": f"not a directory: {meetings_dir}"}), file=sys.stderr)
        return 1

    output_path = args.output or (Path.home() / ".minutes" / "graph" / "index.json")
    output_path.parent.mkdir(parents=True, exist_ok=True)

    if args.incremental and output_path.exists():
        try:
            existing = json.loads(output_path.read_text())
            built_at_unix = existing.get("built_at_unix", 0)
            newest = max(
                (p.stat().st_mtime for p in meetings_dir.glob("*.md")), default=0
            )
            if newest <= built_at_unix:
                print(
                    json.dumps(
                        {
                            "status": "fresh",
                            "meeting_count": existing.get("meeting_count", 0),
                            "person_count": len(existing.get("people", {})),
                            "topic_count": len(existing.get("topics", {})),
                            "output": str(output_path),
                        }
                    )
                )
                return 0
        except (OSError, json.JSONDecodeError):
            pass  # fall through to full rebuild

    index = build_index(meetings_dir)

    # Atomic write: temp + rename
    temp = output_path.with_suffix(".tmp")
    temp.write_text(json.dumps(index, indent=2))
    temp.replace(output_path)

    print(
        json.dumps(
            {
                "status": "ok",
                "meeting_count": index["meeting_count"],
                "person_count": len(index["people"]),
                "topic_count": len(index["topics"]),
                "output": str(output_path),
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
