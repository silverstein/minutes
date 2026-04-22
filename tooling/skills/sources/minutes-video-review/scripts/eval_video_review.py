#!/usr/bin/env python3
"""
Simple evaluation harness for Minutes Video Review bundles.

Usage:
  python3 eval_video_review.py --bundle-dir /path/to/bundle
  python3 eval_video_review.py --source https://go.screenpal.com/watch/... --focus "..."
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


EXPECTED_SCREENPAL_ISSUES = [
    "pending introductions are not appearing",
    "duplicate participant entries",
    "confuse recipients",
]


def run(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, capture_output=True, text=True, check=True)


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def score_bundle(bundle_dir: Path, scenario: str) -> tuple[int, list[str]]:
    analysis = load_json(bundle_dir / "analysis.json")
    metadata = load_json(bundle_dir / "metadata.json")
    analysis_body = analysis.get("analysis", {})
    problems = [str(x).lower() for x in analysis_body.get("problem_signals", [])]
    evidence = analysis_body.get("evidence", [])
    actions = analysis_body.get("recommended_next_actions", [])

    score = 0
    notes: list[str] = []

    if metadata.get("transcript_method") == "minutes-parakeet":
        score += 2
        notes.append("uses Minutes Parakeet transcription")
    else:
        notes.append(f"transcript method was {metadata.get('transcript_method')!r}")

    primary_signal = analysis_body.get("primary_signal")
    if primary_signal == "bug":
        score += 2
        notes.append("classifies as bug")
    elif primary_signal == "mixed":
        score += 1
        notes.append("classifies as mixed")
    else:
        notes.append(f"primary signal was {primary_signal!r}")

    if scenario == "bug":
        matched_issue_count = 0
        for expected in EXPECTED_SCREENPAL_ISSUES:
            if any(expected in problem for problem in problems):
                matched_issue_count += 1
        score += matched_issue_count * 2
        notes.append(f"matched {matched_issue_count}/3 expected issue themes")
    else:
        content_type = str(analysis_body.get("content_type", "unknown"))
        review_mode = str(analysis_body.get("review_mode", "unknown"))
        transcript_quality = str(metadata.get("transcript_quality", {}).get("quality", "unknown"))
        contact_sheet_artifact = metadata.get("contact_sheet_artifact")
        if scenario == "demo" and content_type in {"product-demo", "tutorial", "walkthrough"}:
            score += 3
            notes.append(f"classifies as {content_type}")
        elif scenario == "culture" and content_type == "culture-update":
            score += 3
            notes.append("classifies as culture-update")
        else:
            notes.append(f"content type was {content_type!r}")

        if transcript_quality == "low" and review_mode == "frame-first":
            score += 2
            notes.append("uses frame-first review mode for low-quality transcript")
        elif transcript_quality in {"medium", "high"} and review_mode == "transcript-first":
            score += 2
            notes.append("uses transcript-first review mode for usable transcript")
        else:
            notes.append(
                f"transcript quality/review mode was {transcript_quality!r}/{review_mode!r}"
            )

        if contact_sheet_artifact and Path(contact_sheet_artifact).exists():
            score += 1
            notes.append("includes contact sheet artifact")
        else:
            notes.append("missing contact sheet artifact")

    if evidence:
        score += 2
        notes.append(f"captures {len(evidence)} evidence items")
    else:
        notes.append("captures no evidence items")

    if actions:
        score += 2
        notes.append(f"captures {len(actions)} recommended actions")
    else:
        notes.append("captures no recommended actions")

    return min(score, 10), notes


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate a Minutes Video Review bundle.")
    parser.add_argument("--bundle-dir", default=None, help="Existing bundle directory to score")
    parser.add_argument("--source", default=None, help="Optional source to run before scoring")
    parser.add_argument("--focus", default="video review eval", help="Focus string when running from source")
    parser.add_argument("--out-dir", default="/tmp/minutes-video-review-eval", help="Output root for generated bundles")
    parser.add_argument(
        "--scenario",
        choices=["bug", "demo", "culture"],
        default="bug",
        help="Expected video scenario for scoring",
    )
    args = parser.parse_args()

    bundle_dir: Path | None = Path(args.bundle_dir).resolve() if args.bundle_dir else None

    if args.source:
        script_path = Path(__file__).with_name("video_review.py")
        result = run(
            [
                "python3",
                str(script_path),
                args.source,
                "--out-dir",
                args.out_dir,
                "--focus",
                args.focus,
            ]
        )
        payload = json.loads(result.stdout)
        bundle_dir = Path(payload["bundle_dir"]).resolve()

    if bundle_dir is None:
        print("Error: pass --bundle-dir or --source", file=sys.stderr)
        return 1

    score, notes = score_bundle(bundle_dir, args.scenario)
    payload = {
        "bundle_dir": str(bundle_dir),
        "score_out_of_10": score,
        "notes": notes,
        "passes_threshold": score >= 9,
    }
    print(json.dumps(payload, indent=2))
    return 0 if score >= 9 else 1


if __name__ == "__main__":
    raise SystemExit(main())
