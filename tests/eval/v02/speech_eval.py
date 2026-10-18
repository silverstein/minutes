#!/usr/bin/env python3
"""Score real user-provided WAVs and transcript/speaker hypotheses offline.

No recordings or model scores are bundled. Run a candidate through --command
(a JSON argv array, no shell), or score previously produced --hypotheses JSON.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import wave

from metrics import critical_errors, diarization_error, word_errors, words


def load_manifest(path: Path) -> dict:
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or not manifest.get("recordings"):
        raise ValueError("manifest needs schema_version=1 and nonempty recordings")
    ids = [item["id"] for item in manifest["recordings"]]
    if len(set(ids)) != len(ids):
        raise ValueError("recording IDs must be unique")
    for item in manifest["recordings"]:
        authorization = item.get("authorization", {})
        if authorization.get("basis") not in ("consent", "license") or not authorization.get("reference"):
            raise ValueError(f"{item['id']}: document consent or license reference before evaluating")
        audio = (path.parent / item["audio"]).resolve()
        digest = hashlib.sha256(audio.read_bytes()).hexdigest()
        if digest != item.get("sha256"):
            raise ValueError(f"{item['id']}: audio SHA256 mismatch")
        with wave.open(str(audio)) as recording:
            duration = recording.getnframes() / recording.getframerate()
        if not math.isfinite(duration) or duration <= 0:
            raise ValueError(f"{item['id']}: audio must be nonempty PCM WAV")
        if not isinstance(item.get("reference_text"), str) or not words(item["reference_text"]):
            raise ValueError(f"{item['id']}: reference_text must contain words")
        if item.get("split") not in ("development", "holdout"):
            raise ValueError(f"{item['id']}: split must be development or holdout")
        for term in item.get("critical_terms", []):
            if term.get("kind") not in ("name", "number", "negation", "other"):
                raise ValueError(f"{item['id']}: unknown critical term kind")
        # Validate reference boundaries even if no candidate supplies speakers.
        if "segments" in item:
            diarization_error(item["segments"], [], duration)
        item["_audio"] = audio
        item["_duration"] = duration
    return manifest


def all_critical_terms(recording: dict, text: str) -> list[dict]:
    terms = list(recording.get("critical_terms", []))
    existing = {tuple(words(term["text"])) for term in terms}
    vocabulary = set(words(recording["reference_text"])) | set(words(text))
    for token in sorted(vocabulary):
        kind = "number" if token[0].isdigit() else (
            "negation" if recording.get("language") == "en" and
            (token in {"no", "not", "never", "neither", "nor", "without"} or token.endswith("n't")) else None)
        if kind and (token,) not in existing:
            terms.append({"text": token, "kind": kind})
            existing.add((token,))
    return terms


def score_recording(recording: dict, hypothesis: dict) -> dict:
    if not isinstance(hypothesis.get("text"), str):
        raise ValueError(f"{recording['id']}: hypothesis text must be a string")
    duration = recording["_duration"]
    row = {"id": recording["id"], "split": recording["split"],
           "conditions": recording.get("conditions", {}), "audio_duration_seconds": duration,
           "wer": word_errors(recording["reference_text"], hypothesis["text"]),
           "critical": critical_errors(recording["reference_text"], hypothesis["text"],
                                       all_critical_terms(recording, hypothesis["text"]))}
    if "segments" in recording and "segments" in hypothesis:
        row["diarization"] = diarization_error(recording["segments"], hypothesis["segments"], duration)
    if "processing_seconds" in hypothesis:
        elapsed = hypothesis["processing_seconds"]
        if isinstance(elapsed, bool) or not isinstance(elapsed, (float, int)) or not math.isfinite(elapsed) or elapsed <= 0:
            raise ValueError("processing_seconds must be finite and positive")
        row["processing_seconds"] = elapsed
        row["real_time_factor"] = elapsed / duration
    return row


def aggregate(rows: list[dict]) -> dict:
    words_count = sum(row["wer"]["reference_words"] for row in rows)
    errors = sum(row["wer"]["errors"] for row in rows)
    diarization = [row["diarization"] for row in rows if "diarization" in row]
    speaker_seconds = sum(row["reference_speaker_seconds"] for row in diarization)
    diarization_errors = sum(row[key] for row in diarization for key in
                            ("missed_speaker_seconds", "false_alarm_speaker_seconds", "confusion_speaker_seconds"))
    duration = sum(row["audio_duration_seconds"] for row in rows)
    all_timed = bool(rows) and all("processing_seconds" in row for row in rows)
    return {"recordings": len(rows), "audio_seconds": duration,
            "reference_words": words_count, "wer": errors / words_count if words_count else None,
            "critical_errors": sum(row["critical"]["errors"] for row in rows),
            "der": diarization_errors / speaker_seconds if speaker_seconds else None,
            "diarization_recordings": len(diarization),
            "real_time_factor": sum(row["processing_seconds"] for row in rows) / duration if all_timed else None}


def gate_report(summary: dict, gates: dict | None) -> dict:
    if gates is None:
        return {"status": "not-configured", "passed": False,
                "reason": "Supply release thresholds established before evaluating the holdout."}
    allowed = {"max_wer": "wer", "max_der": "der", "max_critical_errors": "critical_errors",
               "max_real_time_factor": "real_time_factor", "min_audio_seconds": "audio_seconds",
               "min_recordings": "recordings"}
    if not gates or set(gates) - set(allowed):
        raise ValueError("gates must be a nonempty object using documented metric thresholds")
    details = []
    for gate, threshold in gates.items():
        if isinstance(threshold, bool) or not isinstance(threshold, (float, int)) or not math.isfinite(threshold) or threshold < 0:
            raise ValueError("gate thresholds must be finite nonnegative numbers")
        actual = summary[allowed[gate]]
        passed = actual is not None and (actual >= threshold if gate.startswith("min_") else actual <= threshold)
        # A subset with diarization cannot qualify the entire test corpus.
        if gate == "max_der":
            passed = passed and summary["diarization_recordings"] == summary["recordings"]
        details.append({"gate": gate, "threshold": threshold, "actual": actual, "passed": passed})
    return {"status": "scored", "passed": summary["recordings"] > 0 and all(item["passed"] for item in details), "checks": details}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--hypotheses", type=Path, help="JSON {candidate:{...}, recordings:[{id,text,segments?,processing_seconds?}]}")
    mode.add_argument("--command", help='JSON argv array with {audio} and {output} placeholders; output one hypothesis JSON')
    parser.add_argument("--candidate", help="Candidate/model configuration label required with --command")
    parser.add_argument("--candidate-config", type=Path,
                        help="JSON metadata: model, revision, hardware, platform, privacy, cost_per_audio_hour")
    parser.add_argument("--timeout", type=float, default=600)
    parser.add_argument("--gates", type=Path, help="JSON thresholds applied only to holdout aggregate")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    manifest = load_manifest(args.manifest)
    if args.hypotheses:
        supplied = json.loads(args.hypotheses.read_text())
        candidate = supplied["candidate"]
        hypotheses = supplied["recordings"]
        timing_source = "candidate-reported"
    else:
        command = json.loads(args.command)
        if (not args.candidate or not isinstance(command, list) or not command
                or not all(isinstance(part, str) for part in command)
                or not any("{audio}" in part for part in command)
                or not any("{output}" in part for part in command)):
            parser.error("--command requires --candidate and JSON argv with {audio} and {output}")
        metadata = json.loads(args.candidate_config.read_text()) if args.candidate_config else {}
        candidate, hypotheses, timing_source = {**metadata, "label": args.candidate, "command": command}, [], "harness-wall-clock"
        with tempfile.TemporaryDirectory(prefix="minutes-speech-eval-") as temporary:
            for index, recording in enumerate(manifest["recordings"]):
                output = Path(temporary) / f"hypothesis-{index}.json"
                argv = [part.replace("{audio}", str(recording["_audio"])).replace("{output}", str(output)) for part in command]
                start = time.perf_counter()
                subprocess.run(argv, check=True, timeout=args.timeout, stdin=subprocess.DEVNULL)
                elapsed = time.perf_counter() - start
                hypothesis = json.loads(output.read_text())
                hypotheses.append({**hypothesis, "id": recording["id"], "processing_seconds": elapsed})
    ids = [item["id"] for item in hypotheses]
    if len(set(ids)) != len(ids) or set(ids) != {item["id"] for item in manifest["recordings"]}:
        raise ValueError("hypothesis IDs must exactly cover manifest IDs, with no duplicates")
    by_id = {item["id"]: item for item in hypotheses}
    rows = [score_recording(item, by_id[item["id"]]) for item in manifest["recordings"]]
    summaries = {split: aggregate([row for row in rows if row["split"] == split]) for split in ("development", "holdout")}
    gates = json.loads(args.gates.read_text()) if args.gates else None
    report = {"schema_version": 1, "candidate": candidate, "timing_source": timing_source,
              "manifest_sha256": hashlib.sha256(args.manifest.read_bytes()).hexdigest(),
              "provenance": manifest.get("provenance", {}), "summaries": summaries, "rows": rows,
              "gate": gate_report(summaries["holdout"], gates),
              "qualification": "Observed supplied audio only; no automatic claim of independent blinding, capture reliability or native UX qualification."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(args.output), "gate": report["gate"]}))
    return 0 if report["gate"]["passed"] else 2


if __name__ == "__main__":
    sys.exit(main())
