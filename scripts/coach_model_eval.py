#!/usr/bin/env python3
"""Run the production Coach prompt against current and challenger Ollama models."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import platform as host_platform
import shutil
import subprocess
import sys
import urllib.error
import urllib.request

try:
    import tomllib
except ModuleNotFoundError as error:  # pragma: no cover - depends on host Python
    raise SystemExit("coach_model_eval.py requires Python 3.11 or newer") from error


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CANDIDATES = REPO_ROOT / "tooling" / "coach-models" / "candidates.toml"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Benchmark Coach model candidates without changing the product manifest."
    )
    parser.add_argument("--candidates", type=Path, default=DEFAULT_CANDIDATES)
    parser.add_argument("--output", type=Path, default=Path("coach-model-report.md"))
    parser.add_argument("--minutes-bin", default="minutes")
    parser.add_argument("--fixtures", type=Path)
    parser.add_argument(
        "--small-only",
        action="store_true",
        help="Run only candidates marked safe for hosted macOS runners.",
    )
    parser.add_argument(
        "--platform",
        choices=("auto", "apple_silicon", "portable"),
        default="auto",
        help="Override candidate platform selection (mainly for validation).",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="Print selected tags without pulling or evaluating them.",
    )
    return parser.parse_args()


def machine_class(override: str) -> str:
    if override != "auto":
        return override
    machine = host_platform.machine().lower()
    if sys.platform == "darwin" and machine in {"arm64", "aarch64"}:
        return "apple_silicon"
    return "portable"


def load_candidates(path: Path, machine: str, small_only: bool) -> list[dict]:
    with path.open("rb") as handle:
        payload = tomllib.load(handle)
    if payload.get("schema_version") != 1:
        raise ValueError(f"unsupported candidates schema in {path}")
    selected = []
    for candidate in payload.get("candidates", []):
        if candidate.get("platform") not in {"any", machine}:
            continue
        if small_only and not candidate.get("small_ci", False):
            continue
        selected.append(candidate)
    if not selected:
        raise ValueError(f"no candidates selected from {path}")
    return selected


def ollama_base_url() -> str:
    value = os.environ.get("OLLAMA_HOST", "http://localhost:11434").rstrip("/")
    if "://" not in value:
        value = f"http://{value}"
    return value


def warm_model(tag: str) -> None:
    body = json.dumps(
        {
            "model": tag,
            "prompt": "Warm up for a short structured coaching response.",
            "stream": False,
            "think": False,
            "options": {"num_predict": 1},
        }
    ).encode()
    request = urllib.request.Request(
        f"{ollama_base_url()}/api/generate",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=600) as response:
        response.read()


def run_candidate(candidate: dict, args: argparse.Namespace) -> dict:
    tag = candidate["tag"]
    print(f"[{tag}] pulling", flush=True)
    pull = subprocess.run(
        ["ollama", "pull", tag],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if pull.returncode != 0:
        return {"candidate": candidate, "error": pull.stdout.strip() or "ollama pull failed"}

    print(f"[{tag}] warming", flush=True)
    try:
        warm_model(tag)
    except (OSError, urllib.error.URLError) as error:
        return {"candidate": candidate, "error": f"warmup failed: {error}"}

    command = [
        args.minutes_bin,
        "copilot",
        "eval",
        "--accelerated",
        "--json",
        "--model",
        tag,
    ]
    if args.fixtures:
        command.extend(["--fixtures", str(args.fixtures)])
    print(f"[{tag}] evaluating", flush=True)
    completed = subprocess.run(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        return {"candidate": candidate, "error": detail or "minutes copilot eval failed"}
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        return {"candidate": candidate, "error": f"eval returned invalid JSON: {error}"}
    return {"candidate": candidate, "report": report}


def percentage(metric: dict | None) -> str:
    if not metric:
        return "—"
    return f"{float(metric.get('rate', 0)) * 100:.1f}%"


def latency_pair(metric: dict | None) -> str:
    if not metric:
        return "—"
    p50 = metric.get("p50_ms")
    p95 = metric.get("p95_ms")
    if p50 is None or p95 is None:
        return "—"
    return f"{p50}/{p95} ms"


def markdown_escape(value: object) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ")


def render_report(results: list[dict], machine: str, small_only: bool) -> str:
    generated = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    lines = [
        "# Coach model freshness report",
        "",
        f"Generated: {generated}",
        f"Candidate platform: `{machine}`",
        f"Scope: `{'small tiers only' if small_only else 'all hardware tiers'}`",
        "",
        "This report is evidence for human review. It does not rank, promote, or change any model.",
        "`Schema valid` means the response was accepted by Minutes' production NudgeDraft parser.",
        "",
        "| Tier | Role | Model | Schema valid | Visible nudges | Useful precision | Opportunity recall | No-nudge quality | TTFT p50/p95 | Total p50/p95 | Status |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|",
    ]
    for result in results:
        candidate = result["candidate"]
        report = result.get("report")
        if report:
            quality = report.get("quality", {})
            row = [
                candidate["tier"],
                candidate["role"],
                f"`{candidate['tag']}`",
                percentage(report.get("schema_valid")),
                report.get("visible_nudges", 0),
                percentage(quality.get("useful_nudge_precision")),
                percentage(quality.get("opportunity_recall")),
                percentage(quality.get("no_nudge_quality")),
                latency_pair(report.get("ttft")),
                latency_pair(report.get("total")),
                "completed",
            ]
        else:
            row = [
                candidate["tier"],
                candidate["role"],
                f"`{candidate['tag']}`",
                "—",
                "—",
                "—",
                "—",
                "—",
                "—",
                "—",
                f"error: {result.get('error', 'unknown error')}",
            ]
        lines.append("| " + " | ".join(markdown_escape(item) for item in row) + " |")

    lines.extend(["", "## Candidate details", ""])
    for result in results:
        candidate = result["candidate"]
        lines.extend(
            [
                f"### `{candidate['tag']}`",
                "",
                f"- Tier/role: `{candidate['tier']}` / `{candidate['role']}`",
                f"- Candidate source: {candidate['source']}",
            ]
        )
        report = result.get("report")
        if not report:
            lines.extend([f"- Error: {result.get('error', 'unknown error')}", ""])
            continue
        lines.extend(
            [
                f"- Requests: {report.get('requests', 0)}",
                f"- Schema valid: {report.get('schema_valid', {}).get('numerator', 0)}/{report.get('schema_valid', {}).get('denominator', 0)}",
                "",
                "Raw parsed nudges from the production prompt:",
                "",
            ]
        )
        for sample in report.get("samples", []):
            identity = (
                f"{sample.get('fixture_id')} / utterance "
                f"{sample.get('evidence_utterance_sequence')}"
            )
            lines.append(f"- **{identity}** — TTFT {sample.get('ttft_ms')} ms; total {sample.get('total_ms')} ms")
            if sample.get("draft") is not None:
                lines.extend(["", "  ```json", json.dumps(sample["draft"], ensure_ascii=False), "  ```"])
            elif sample.get("error"):
                lines.append(f"  Error: {sample['error']}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    args = parse_args()
    machine = machine_class(args.platform)
    try:
        candidates = load_candidates(args.candidates, machine, args.small_only)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.list:
        for candidate in candidates:
            print(f"{candidate['tier']}\t{candidate['role']}\t{candidate['tag']}")
        return 0

    if shutil.which("ollama") is None:
        print("error: ollama is not on PATH", file=sys.stderr)
        return 2
    if shutil.which(args.minutes_bin) is None and not Path(args.minutes_bin).exists():
        print(f"error: Minutes binary not found: {args.minutes_bin}", file=sys.stderr)
        return 2

    results = [run_candidate(candidate, args) for candidate in candidates]
    report = render_report(results, machine, args.small_only)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(report, encoding="utf-8")
    print(f"wrote {args.output}")
    print("\n".join(report.splitlines()[: len(results) + 11]))
    return 0 if any(result.get("report") for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
