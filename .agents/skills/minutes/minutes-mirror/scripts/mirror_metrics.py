#!/usr/bin/env python3
"""
mirror_metrics.py — Deterministic behavioral metrics from a meeting transcript.

Used by the /minutes-mirror skill to avoid LLM token-counting errors.
The skill calls this script via Bash and consumes the JSON output.

Usage:
    mirror_metrics.py <meeting_file.md> --self "Mat,Mat S.,MAT_SILVERSTEIN"

Self labels are matched case-insensitively against the speaker label inside
[NAME 0:00] markers in the transcript section. Multiple labels can be passed
comma-separated for users who appear under different names across meetings.

Output: JSON to stdout with talk_ratio, fillers, hedging, monologue, etc.
Errors: JSON to stderr with non-zero exit code.

Requires: Python 3.8+, stdlib only. No external dependencies.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# Filler words counted in self speech.
FILLERS = (
    "um", "uh", "like", "you know", "basically", "literally", "kinda", "right?",
)

# Hedging language counted in self speech. "just" is intentionally excluded —
# its false-positive rate is too high ("just want to confirm" is not hedging).
HEDGES = (
    "maybe", "kind of", "sort of", "i think", "i guess", "possibly",
    "somewhat", "a little", "perhaps", "sorry to",
)

# Speaker turn markers. Real Minutes transcripts use two distinct formats and
# mirror needs to handle both:
#
#   1. Bracket form (from local whisper + diarization):
#        [Mat 0:00] Hello
#        [Mat S. 0:00] Hello          ← multi-word label
#        [SPEAKER_3 12:34] Hello
#        [Mat S.] Hello               ← no timestamp
#
#   2. Bold form (from imported/cleaned transcripts):
#        **Hiro Protagonist**: Hello
#        **Y.T.**: Hello
#        **Name** 12:34: Hello        ← optional timestamp before the colon
#
# Both capture groups are non-greedy so the optional timestamp can claim any
# trailing `\s+\d+:\d+`, leaving multi-word names intact in group 1.
BRACKET_SPEAKER_RE = re.compile(r"^\[(.+?)(?:\s+(\d+:\d+(?::\d+)?))?\]\s*(.*)$")
BOLD_SPEAKER_RE = re.compile(r"^\*\*(.+?)\*\*(?:\s+(\d+:\d+(?::\d+)?))?\s*:\s*(.*)$")
TRANSCRIPT_HEADER_RE = re.compile(r"^##\s*Transcript\s*$", re.MULTILINE | re.IGNORECASE)
NEXT_SECTION_RE = re.compile(r"^##\s+\S", re.MULTILINE)  # next top-level section after transcript


def parse_self_labels(arg: str) -> set[str]:
    return {label.strip().lower() for label in arg.split(",") if label.strip()}


def extract_transcript(content: str) -> str:
    """Return the text between `## Transcript` and the next top-level section.

    Without the end-bound, trailing sections like `## Action Items` or
    `## Decisions` get appended to the final speaker turn (via parse_turns'
    non-speaker-line continuation logic) and pollute talk-time/monologue
    metrics. Stop at the next `##` heading to keep the transcript clean.
    """
    start_match = TRANSCRIPT_HEADER_RE.search(content)
    if not start_match:
        return content
    body = content[start_match.end():]
    end_match = NEXT_SECTION_RE.search(body)
    if end_match:
        return body[: end_match.start()]
    return body


def parse_time_to_seconds(time_str: str | None) -> int | None:
    if not time_str:
        return None
    parts = time_str.split(":")
    try:
        if len(parts) == 2:
            return int(parts[0]) * 60 + int(parts[1])
        if len(parts) == 3:
            return int(parts[0]) * 3600 + int(parts[1]) * 60 + int(parts[2])
    except ValueError:
        return None
    return None


def match_speaker_line(line: str):
    """Try each speaker marker format in turn. Returns the first match or None.

    Bracket form is checked first because it's more common in locally-recorded
    Minutes transcripts. Bold form handles imported/cleaned transcripts.
    """
    return BRACKET_SPEAKER_RE.match(line) or BOLD_SPEAKER_RE.match(line)


def parse_turns(transcript: str, self_labels: set[str]) -> list[dict]:
    turns: list[dict] = []
    current: dict | None = None
    for raw in transcript.splitlines():
        line = raw.strip()
        match = match_speaker_line(line)
        if match:
            if current is not None:
                turns.append(current)
            speaker = match.group(1).strip()
            current = {
                "speaker": speaker,
                "time": match.group(2),
                "text": match.group(3) or "",
                "is_self": speaker.lower() in self_labels,
            }
        elif current is not None and line:
            current["text"] += " " + line
    if current is not None:
        turns.append(current)
    return turns


def count_pattern_hits(text: str, patterns: tuple[str, ...]) -> int:
    text_lower = text.lower()
    total = 0
    for pattern in patterns:
        if " " in pattern or pattern.endswith("?"):
            total += text_lower.count(pattern)
        else:
            total += len(re.findall(rf"\b{re.escape(pattern)}\b", text_lower))
    return total


def estimate_seconds_from_words(word_count: int) -> int:
    # ~150 wpm conversational speech.
    return round(word_count / 150.0 * 60)


def compute_metrics(turns: list[dict]) -> dict:
    total_words = 0
    self_words = 0
    other_words = 0
    self_filler = 0
    self_hedging = 0
    self_questions = 0
    speakers: set[str] = set()

    # Stretch tracking for monologue detection: collapse consecutive same-side turns.
    stretches: list[dict] = []
    current_stretch: dict | None = None

    for turn in turns:
        speakers.add(turn["speaker"])
        words = turn["text"].split()
        wc = len(words)
        total_words += wc

        if turn["is_self"]:
            self_words += wc
            self_filler += count_pattern_hits(turn["text"], FILLERS)
            self_hedging += count_pattern_hits(turn["text"], HEDGES)
            self_questions += turn["text"].count("?")
        else:
            other_words += wc

        side = "self" if turn["is_self"] else "other"
        if current_stretch is None or current_stretch["side"] != side:
            if current_stretch is not None:
                stretches.append(current_stretch)
            current_stretch = {
                "side": side,
                "word_count": wc,
                "first_words": " ".join(words[:8]),
                "start_time": turn.get("time"),
            }
        else:
            current_stretch["word_count"] += wc
    if current_stretch is not None:
        stretches.append(current_stretch)

    self_stretches = [s for s in stretches if s["side"] == "self"]
    other_stretches = [s for s in stretches if s["side"] == "other"]
    longest_monologue = max(self_stretches, key=lambda s: s["word_count"], default=None)
    longest_listen = max(other_stretches, key=lambda s: s["word_count"], default=None)

    # Duration: use last timestamped turn if available, else estimate from words.
    last_seconds = None
    for turn in reversed(turns):
        secs = parse_time_to_seconds(turn.get("time"))
        if secs is not None:
            last_seconds = secs
            break
    duration_minutes = (last_seconds / 60.0) if last_seconds else (total_words / 150.0)
    duration_minutes = max(duration_minutes, 0.1)  # avoid div-by-zero

    talk_ratio = (self_words / total_words) if total_words > 0 else 0.0

    def per_100(count: int) -> float:
        return round(count * 100 / self_words, 2) if self_words > 0 else 0.0

    def stretch_summary(s: dict | None) -> dict | None:
        if not s:
            return None
        return {
            "word_count": s["word_count"],
            "seconds_estimate": estimate_seconds_from_words(s["word_count"]),
            "first_words": s["first_words"],
            "start_time": s["start_time"],
        }

    return {
        "total_words": total_words,
        "self_words": self_words,
        "other_words": other_words,
        "talk_ratio": round(talk_ratio, 3),
        "self_turn_count": sum(1 for t in turns if t["is_self"]),
        "other_turn_count": sum(1 for t in turns if not t["is_self"]),
        "speakers": sorted(speakers),
        "filler_count": self_filler,
        "filler_per_100_words": per_100(self_filler),
        "hedging_count": self_hedging,
        "hedging_per_100_words": per_100(self_hedging),
        "question_count": self_questions,
        "questions_per_5min": round(self_questions * 5 / duration_minutes, 2),
        "duration_minutes": round(duration_minutes, 1),
        "longest_monologue": stretch_summary(longest_monologue),
        "longest_listen": stretch_summary(longest_listen),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("meeting_file", type=Path, help="Path to a meeting markdown file")
    parser.add_argument(
        "--self",
        required=True,
        dest="self_labels",
        help="Comma-separated speaker labels for the user (case-insensitive)",
    )
    args = parser.parse_args()

    if not args.meeting_file.exists():
        print(json.dumps({"error": f"file not found: {args.meeting_file}"}), file=sys.stderr)
        return 1

    self_labels = parse_self_labels(args.self_labels)
    if not self_labels:
        print(json.dumps({"error": "--self must contain at least one label"}), file=sys.stderr)
        return 1

    content = args.meeting_file.read_text(encoding="utf-8", errors="replace")
    transcript = extract_transcript(content)
    turns = parse_turns(transcript, self_labels)

    if not turns:
        print(
            json.dumps(
                {
                    "error": "no diarized speaker turns found",
                    "hint": "transcript may be a single block without [NAME 0:00] markers",
                }
            ),
            file=sys.stderr,
        )
        return 2

    self_matches = sum(1 for t in turns if t["is_self"])
    if self_matches == 0:
        print(
            json.dumps(
                {
                    "error": "no turns matched any self label",
                    "self_labels": sorted(self_labels),
                    "speakers_found": sorted({t["speaker"] for t in turns}),
                    "hint": "check that --self matches the speaker labels in the transcript",
                }
            ),
            file=sys.stderr,
        )
        return 3

    metrics = compute_metrics(turns)
    print(json.dumps(metrics, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
