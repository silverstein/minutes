#!/usr/bin/env python3
"""Recommend a measured candidate; never change application model defaults."""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
import re
import sys

from speech_eval import aggregate


def finite(value) -> bool:
    return isinstance(value, (float, int)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def assess(report: dict, requirements: dict) -> tuple[list[str], dict]:
    reasons = []
    candidate, provenance = report.get("candidate", {}), report.get("provenance", {})
    if report.get("schema_version") != 1:
        reasons.append("unsupported report schema")
    if report.get("manifest_sha256") != requirements["manifest_sha256"]:
        reasons.append("different or missing evaluation corpus")
    if (provenance.get("kind") not in ("consented-recordings", "licensed-recordings")
            or provenance.get("independent_holdout") is not True or provenance.get("synthetic") is not False):
        reasons.append("missing declared real independent holdout provenance")
    if report.get("timing_source") != "harness-wall-clock":
        reasons.append("timing was not measured by the harness")
    for field in ("model", "revision", "hardware", "platform", "privacy"):
        if not isinstance(candidate.get(field), str) or not candidate[field].strip():
            reasons.append(f"missing candidate {field}")
    for field in ("hardware", "platform"):
        if candidate.get(field) != requirements[field]:
            reasons.append(f"measured {field} does not match requested {field}")
    if candidate.get("privacy") not in ("local", "cloud"):
        reasons.append("unrecognized privacy mode")
    elif requirements["privacy"] == "local" and candidate["privacy"] != "local":
        reasons.append("cloud inference is excluded by local-only requirement")
    cost = candidate.get("cost_per_audio_hour")
    if not finite(cost) or cost > requirements["max_cost_per_audio_hour"]:
        reasons.append("missing or over-budget estimated cost")
    if report.get("gate", {}).get("passed") is not True:
        reasons.append("candidate did not pass its predeclared release gates")
    rows = [row for row in report.get("rows", []) if row.get("split") == "holdout"]
    ids = [row.get("id") for row in rows]
    if not ids or any(not isinstance(value, str) or not value for value in ids) or len(set(ids)) != len(ids):
        reasons.append("missing or duplicate holdout recordings")
    try:
        for row in rows:
            if not finite(row.get("audio_duration_seconds")) or row["audio_duration_seconds"] <= 0:
                raise ValueError("invalid duration")
            for field in ("reference_words", "errors", "substitutions", "deletions", "insertions"):
                if not finite(row["wer"][field]):
                    raise ValueError("invalid WER counts")
            if row["wer"]["reference_words"] <= 0 or not finite(row["critical"]["errors"]):
                raise ValueError("invalid reference or critical counts")
            if row["wer"]["errors"] != sum(row["wer"][key] for key in ("substitutions", "deletions", "insertions")):
                raise ValueError("inconsistent word error counts")
            if not finite(row.get("processing_seconds")) or row["processing_seconds"] <= 0:
                raise ValueError("invalid processing time")
            if "diarization" in row:
                for field in ("missed_speaker_seconds", "false_alarm_speaker_seconds", "confusion_speaker_seconds", "reference_speaker_seconds"):
                    if not finite(row["diarization"][field]):
                        raise ValueError("invalid speaker durations")
        # Recompute metrics from per-recording rows, not supplied summary rankings.
        summary = aggregate(rows)
    except (KeyError, TypeError, ValueError, ZeroDivisionError):
        return reasons + ["malformed or incomplete per-recording metrics"], {}
    for field in ("wer", "der", "real_time_factor", "audio_seconds", "reference_words", "critical_errors"):
        if not finite(summary.get(field)):
            reasons.append(f"missing or invalid {field}")
    if summary["diarization_recordings"] != summary["recordings"]:
        reasons.append("speaker reference/hypothesis coverage is incomplete")
    if summary["recordings"] < requirements["min_recordings"]:
        reasons.append("too few holdout recordings")
    if summary["audio_seconds"] < requirements["min_audio_seconds"]:
        reasons.append("insufficient holdout audio duration")
    for field in ("wer", "der", "critical_errors"):
        if finite(summary.get(field)) and summary[field] > requirements[f"max_{field}"]:
            reasons.append(f"{field} exceeds requested quality bound")
    return reasons, summary


def select(reports: list[tuple[str, dict]], requirements: dict) -> dict:
    candidates, excluded = [], []
    coverage_sets = {tuple(sorted(row.get("id", "") for row in report.get("rows", [])
                                 if row.get("split") == "holdout")) for _, report in reports
                     if report.get("manifest_sha256") == requirements["manifest_sha256"]}
    for source, report in reports:
        reasons, summary = assess(report, requirements)
        if len(coverage_sets) > 1:
            reasons.append("candidate reports disagree on holdout recording coverage")
        row = {"report": source, "candidate": report.get("candidate"), "summary": summary}
        if reasons:
            excluded.append({**row, "reasons": reasons})
        else:
            candidates.append(row)
    # Same corpus and hardware; safety-critical errors precede general word/
    # speaker accuracy, then measured latency. Cost is an eligibility constraint.
    candidates.sort(key=lambda item: (item["summary"]["critical_errors"], item["summary"]["wer"],
                                     item["summary"]["der"], item["summary"]["real_time_factor"], item["report"]))
    return {"schema_version": 1, "requirements": requirements,
            "status": "recommendation" if candidates else "insufficient-evidence",
            "recommendation": candidates[0] if candidates else None,
            "ranking": candidates, "excluded": excluded,
            "ordering": ["critical_errors", "wer", "der", "real_time_factor"],
            "limitations": "Provenance and cost are evaluator declarations, not independently certified. This is evidence for the specified corpus and hardware only; no defaults are changed."}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--hardware", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--privacy", choices=("local", "either"), default="local")
    parser.add_argument("--max-cost-per-audio-hour", type=float, required=True)
    parser.add_argument("--min-recordings", type=int, default=10)
    parser.add_argument("--min-audio-seconds", type=float, default=1800)
    parser.add_argument("--max-wer", type=float, required=True)
    parser.add_argument("--max-der", type=float, required=True)
    parser.add_argument("--max-critical-errors", type=int, default=0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    requirements = {key: value for key, value in vars(args).items() if key not in ("reports", "output")}
    if (not re.fullmatch(r"[a-fA-F0-9]{64}", args.manifest_sha256)
            or args.min_recordings < 1 or args.min_audio_seconds <= 0
            or any(not finite(value) for value in requirements.values() if isinstance(value, (int, float)))):
        parser.error("valid corpus SHA256 and finite nonnegative constraints with positive coverage are required")
    result = select([(str(path), json.loads(path.read_text())) for path in args.reports], requirements)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "output": str(args.output)}))
    return 0 if result["recommendation"] else 2


if __name__ == "__main__":
    sys.exit(main())
