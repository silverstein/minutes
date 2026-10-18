#!/usr/bin/env python3
"""Evaluate the actual MCP stdio boundary; the client never reads to answer.

Only the evaluator uses oracle files to validate returned citations. A synthetic
pass establishes conformance, never production quality or a state-of-art claim.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import sys
import tempfile
import threading
import time

from corpus import synthetic_suite, validate_suite, write_suite
from metrics import percentile, retrieval_scores

REPO = Path(__file__).resolve().parents[3]


def selected_cli_metadata(server: Path, requested: Path) -> dict:
    """Assert the server's existing fixed discovery selects this repository CLI.

    Do not add an environment override to production binary selection just for
    an evaluation. Reject a release build shadowing the requested debug build.
    """
    root = server.resolve().parents[3]
    filename = "minutes.exe" if os.name == "nt" else "minutes"
    choices = [root / "target" / profile / filename for profile in ("release", "debug")]
    selected = next((path for path in choices if path.is_file()), None)
    if selected is None or selected.resolve() != requested.resolve(strict=True):
        raise ValueError(f"--cli must be the first existing repository release/debug candidate; selected={selected}")
    version = subprocess.run([str(selected), "--version"], check=True, capture_output=True,
                             text=True, timeout=10).stdout.strip()
    return {"path": str(selected.resolve()), "sha256": hashlib.sha256(selected.read_bytes()).hexdigest(),
            "version": version, "selection": "asserted first fixed repository candidate; checked again after run"}


class McpClient:
    def __init__(self, command: list[str], corpus: Path, profile: Path, timeout: float = 60):
        self.timeout, self.sequence, self.responses = timeout, 0, queue.Queue()
        self.stderr_tail = []
        env = dict(os.environ)
        env.update({"HOME": str(profile), "USERPROFILE": str(profile),
                    "XDG_CONFIG_HOME": str(profile / ".config"),
                    "MINUTES_HOME": str(profile / ".minutes"), "MEETINGS_DIR": str(corpus),
                    "MINUTES_MCP_AUTO_SETUP": "0", "MINUTES_MCP_RESTRICTED_POLICY": "deny"})
        env.pop("MINUTES_MCP_TRANSPORT", None)
        env.pop("MCP_EXTENSION_ID", None)
        self.process = subprocess.Popen(command, cwd=profile, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        text=True, encoding="utf-8", bufsize=1)
        self.readers = [threading.Thread(target=self._read, daemon=True),
                        threading.Thread(target=self._stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        try:
            initialized = self.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                      "clientInfo": {"name": "minutes-eval-v02", "version": "1.0"}})
            self.server_info = initialized.get("serverInfo", {})
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        except BaseException:
            self.close()
            raise

    def _read(self):
        for line in self.process.stdout:
            try:
                self.responses.put(json.loads(line))
            except json.JSONDecodeError:
                self.responses.put({"fatal": "non-JSON stdout from MCP server"})
        self.responses.put({"fatal": "MCP process ended before response"})

    def _stderr(self):
        for line in self.process.stderr:
            self.stderr_tail.append(line.rstrip())
            self.stderr_tail = self.stderr_tail[-20:]

    def _send(self, value):
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def call(self, method: str, params: dict) -> dict:
        self.sequence += 1
        request_id = self.sequence
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"MCP {method} timed out")
            try:
                response = self.responses.get(timeout=remaining)
            except queue.Empty as exc:
                raise TimeoutError(f"MCP {method} timed out") from exc
            if "fatal" in response:
                raise RuntimeError(response["fatal"] + "; " + " | ".join(self.stderr_tail))
            if response.get("id") != request_id:
                continue  # server notifications
            if "error" in response:
                raise RuntimeError(f"MCP {method}: {response['error']}")
            return response["result"]

    def tool(self, name: str, arguments: dict) -> tuple[dict, float]:
        start = time.perf_counter()
        result = self.call("tools/call", {"name": name, "arguments": arguments})
        elapsed = (time.perf_counter() - start) * 1000
        if result.get("isError"):
            raise RuntimeError(f"{name} failed: {result.get('content')}")
        return result, elapsed

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=3)
        for reader in self.readers:
            reader.join(timeout=1)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()


def local_path(path: str, corpus: Path) -> str:
    try:
        return str(Path(path).resolve().relative_to(corpus.resolve())).replace(os.sep, "/")
    except ValueError:
        return "<outside-corpus>"


def citation_valid(segment: dict, corpus: Path) -> bool:
    """Require authorized source bytes, exact quote, revision and UTF-16 offsets."""
    try:
        path = Path(segment["path"])
        if local_path(str(path), corpus) == "<outside-corpus>":
            return False
        data = path.read_bytes()
        text = data.decode("utf-8")
        if 'sensitivity: "restricted"' in text or "sensitivity: restricted" in text:
            return False
        if segment["source_revision"] != hashlib.sha256(data).hexdigest():
            return False
        start, end = segment["start"], segment["end"]
        encoded = text.encode("utf-16-le")
        if (not isinstance(start, int) or isinstance(start, bool) or not isinstance(end, int)
                or isinstance(end, bool) or not 0 <= start < end <= len(encoded) // 2
                or segment.get("offset_unit") != "utf16"):
            return False
        quote = encoded[start * 2:end * 2].decode("utf-16-le")
        prefix = encoded[:start * 2].decode("utf-16-le")
        return (bool(segment["quote"]) and quote == segment["quote"]
                and segment["line_start"] == prefix.count("\n") + 1
                and segment["line_end"] == prefix.count("\n") + quote.count("\n") + 1)
    except (KeyError, OSError, ValueError, UnicodeError, TypeError):
        return False


def evaluate_case(client: McpClient, case: dict, corpus: Path, k: int, history: dict | None) -> dict:
    row = {"id": case["id"], "kind": case["kind"], "domain": case.get("domain", "external"),
           "category": case.get("category", case["kind"])}
    if case["kind"] == "currentness":
        entries = (history or {}).get("structuredContent", {}).get("decision_history", [])
        current = [entry for entry in entries if entry.get("declared_id") == case["current_id"]]
        old = [entry for entry in entries if entry.get("declared_id") == case["superseded_id"]]
        evidence = [segment for entry in current + old for segment in entry.get("evidence", [])]
        row["citations_returned"] = len(evidence)
        row["citations_valid"] = sum(citation_valid(segment, corpus) for segment in evidence)
        row["currentness_correct"] = bool(len(current) == len(old) == 1
            and current[0].get("status") == "current" and old[0].get("status") == "superseded"
            and current[0].get("supersedes") == old[0].get("id")
            and current[0].get("topic") == case["topic"]
            and any(local_path(segment.get("path", ""), corpus) == case["current_path"]
                    for segment in current[0].get("evidence", [])))
        row["passed"] = row["currentness_correct"] and len(evidence) >= 2 and row["citations_valid"] == len(evidence)
        return row
    result, latency = client.tool("search_meetings", {"query": case["query"], "limit": k})
    results = result.get("structuredContent", {}).get("results")
    if not isinstance(results, list):
        raise ValueError("search response lacks structuredContent.results")
    paths = [local_path(item.get("path", ""), corpus) for item in results]
    row.update(retrieval_scores(paths, case["relevance"], k))
    row.update({"latency_ms": latency, "paths": paths})
    segments = [segment for item in results for segment in item.get("evidence", {}).get("segments", [])]
    row["citations_returned"] = len(segments)
    row["citations_valid"] = sum(citation_valid(segment, corpus) for segment in segments)
    row["citation_coverage"] = sum(bool(item.get("evidence", {}).get("segments")) for item in results)
    row["passed"] = bool((row["abstention"] is True or row["recall_at_k"] == 1.0)
        and row["citations_valid"] == row["citations_returned"]
        and row["citation_coverage"] == len(results))
    return row


def exact_substring_baseline(suite: dict, query: str, k: int) -> list[str]:
    """Old title/body substring semantics, offline on public JSON-in-YAML fixtures.

    This is not the historical executable and is never used to answer a candidate
    request. External YAML shapes are intentionally outside this baseline.
    """
    matches = []
    needle = query.strip().lower()
    for document in suite["documents"]:
        header, body = document["content"].split("\n---\n", 1)
        fields = {match.group(1): json.loads(match.group(2)) for match in
                  re.finditer(r"^([a-z_]+): (.+)$", header, re.MULTILINE)}
        if fields.get("sensitivity") == "restricted":
            continue
        if needle in fields["title"].lower() or needle in body.lower():
            matches.append((fields["date"], document["path"]))
    matches.sort(reverse=True)
    return [path for _, path in matches[:k]]


def category_comparisons(rows: list[dict]) -> dict:
    groups = {}
    for row in rows:
        if row.get("kind") != "retrieval":
            continue
        for group in ("all", f"category:{row.get('category', 'unknown')}", f"domain:{row.get('domain', 'unknown')}"):
            groups.setdefault(group, []).append(row)
    result = {}
    for group, members in groups.items():
        item = {"samples": len(members), "passed": sum(row["passed"] for row in members)}
        for metric in ("recall_at_k", "ndcg_at_k"):
            actual = sum(row.get(metric) or 0 for row in members) / len(members)
            item[f"candidate_{metric}"] = actual
            baseline_values = [row["offline_substring_baseline"][metric] for row in members
                               if "offline_substring_baseline" in row]
            if baseline_values:
                baseline = sum(baseline_values) / len(baseline_values)
                item[f"baseline_{metric}"] = baseline
                item[f"delta_{metric}"] = actual - baseline
        result[group] = item
    return result


def privacy_probes(client: McpClient, corpus: Path) -> list[dict]:
    from corpus import meeting
    rows = []
    restricted = corpus / "initial-restricted-probe.md"
    restricted.write_text(meeting("Private fixture", "2026-03-06T10:00:00Z",
                                 "[SPEAKER_0 0:01] The neverpublicmarker value is 98231.", restricted=True), encoding="utf-8")
    result, _ = client.tool("search_meetings", {"query": "neverpublicmarker"})
    rows.append({"id": "initial-restricted", "kind": "privacy", "passed":
                 result.get("structuredContent", {}).get("results") == [] and "98231" not in json.dumps(result)})
    restricted.unlink()
    content = meeting("Revocation probe", "2026-03-07T10:00:00Z",
                      "[SPEAKER_0 0:01] The ephemeralmarker value is 83197.")
    path = corpus / "revocation-probe.md"
    path.write_text(content, encoding="utf-8")
    result, _ = client.tool("search_meetings", {"query": "ephemeralmarker"})
    visible = bool(result.get("structuredContent", {}).get("results"))
    for action in ("restricted", "deleted"):
        if action == "restricted":
            path.write_text(content.replace("---\n", '---\nsensitivity: "restricted"\n', 1), encoding="utf-8")
        else:
            path.unlink()
        result, _ = client.tool("search_meetings", {"query": "ephemeralmarker"})
        rows.append({"id": f"revocation-{action}", "kind": "privacy", "passed": visible
                     and result.get("structuredContent", {}).get("results") == []
                     and "83197" not in json.dumps(result)})
    return rows


def run_size(suite: dict, size: int, repeats: int, k: int, command: list[str], timeout: float) -> dict:
    with tempfile.TemporaryDirectory(prefix="minutes-eval-v02-") as directory:
        profile = Path(directory)
        corpus = profile / "corpus"
        corpus.mkdir()
        actual_size = write_suite(suite, corpus, size)
        start = time.perf_counter()
        client = McpClient(command, corpus, profile, timeout)
        startup = (time.perf_counter() - start) * 1000
        rows, history_latencies = [], []
        try:
            available = {tool["name"] for tool in client.call("tools/list", {}).get("tools", [])}
            required = {"search_meetings"}
            if any(case["kind"] == "currentness" for case in suite["cases"]):
                required.add("consistency_report")
            if not required.issubset(available):
                raise RuntimeError(f"MCP missing required tools: {sorted(required - available)}")
            for repeat in range(repeats):
                history, history_error = None, None
                if "consistency_report" in required:
                    try:
                        history, latency = client.tool("consistency_report", {})
                        history_latencies.append(latency)
                    except Exception as exc:
                        history_error = str(exc)
                for case in suite["cases"]:
                    try:
                        if case["kind"] == "currentness" and history_error:
                            raise RuntimeError(history_error)
                        row = evaluate_case(client, case, corpus, k, history)
                    except Exception as exc:
                        row = {"id": case["id"], "kind": case["kind"], "domain": case.get("domain", "external"),
                               "category": case.get("category", case["kind"]), "passed": False, "error": str(exc)}
                    if (case["kind"] in ("retrieval", "abstention")
                            and suite["provenance"].get("kind") == "public-synthetic"):
                        baseline_paths = exact_substring_baseline(suite, case["query"], k)
                        row["offline_substring_baseline"] = {**retrieval_scores(baseline_paths, case["relevance"], k),
                                                             "paths": baseline_paths}
                    rows.append({"repeat": repeat, **row})
            try:
                rows.extend(privacy_probes(client, corpus))
            except Exception as exc:
                rows.append({"id": "privacy-probes", "kind": "privacy", "passed": False, "error": str(exc)})
            if suite.get("provenance", {}).get("kind") == "public-synthetic":
                # Inspect real tool output; never infer identity from substring matches.
                for person, expected in [("Ann Carter", "Identity Ann"), ("Ann", None)]:
                    try:
                        result, _ = client.tool("research_topic", {"query": "ambernotebook", "attendee": person})
                        output = json.dumps(result)
                        passed = (expected in output if expected else "Identity Ann" not in output)
                        passed = passed and "Identity Joanna" not in output and "Identity second Ann" not in output
                        rows.append({"id": f"identity-{person}", "kind": "identity", "passed": passed})
                    except Exception as exc:
                        rows.append({"id": f"identity-{person}", "kind": "identity", "passed": False, "error": str(exc)})
        finally:
            client.close()
        retrieval_rows = [row for row in rows if row.get("kind") == "retrieval"]
        mean = lambda field: (sum(row.get(field) or 0 for row in retrieval_rows) / len(retrieval_rows)
                              if retrieval_rows else None)
        latencies = [row["latency_ms"] for row in rows if "latency_ms" in row]
        return {"requested_documents": size, "actual_documents": actual_size,
                "server_info": client.server_info,
                "startup_ms": startup, "history_latency_ms": history_latencies,
                "search_latency_ms": {"p50": percentile(latencies, .50), "p95": percentile(latencies, .95),
                                      "max": max(latencies, default=None), "samples": len(latencies)},
                "mean_recall_at_k": mean("recall_at_k"), "mean_ndcg_at_k": mean("ndcg_at_k"),
                "comparisons": category_comparisons(rows),
                "baseline_scope": "offline public-fixture title/body exact-substring simulation; no historical executable, privacy, citation or latency comparison",
                "passed": all(row["passed"] for row in rows), "rows": rows}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", type=Path, help="Private/independent JSON suite; default is public synthetic smoke")
    parser.add_argument("--export-suite", type=Path, help="Write the default public suite and exit")
    parser.add_argument("--server", type=Path, default=REPO / "crates/mcp/dist/index.js")
    parser.add_argument("--node", default="node")
    parser.add_argument("--cli", type=Path, help="Assert selected repo release/debug CLI and record its SHA256/version")
    parser.add_argument("--sizes", default="100", help="Comma-separated total document counts, e.g. 100,1000,10000")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--k", type=int, default=10)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    suite = json.loads(args.suite.read_text()) if args.suite else synthetic_suite()
    validate_suite(suite)
    if args.export_suite:
        args.export_suite.parent.mkdir(parents=True, exist_ok=True)
        args.export_suite.write_text(json.dumps(suite, indent=2) + "\n")
        return 0
    if args.output is None:
        parser.error("--output is required when running the evaluation")
    sizes = [int(value) for value in args.sizes.split(",")]
    if args.repeats < 1 or not 1 <= args.k <= 100 or min(sizes) < 1 or args.timeout <= 0:
        parser.error("positive sizes/repeats/timeout and k in 1..100 required")
    if not args.server.is_file():
        parser.error("build crates/sdk and crates/mcp first; server not found")
    cli_metadata = selected_cli_metadata(args.server, args.cli) if args.cli else None
    report = {"schema_version": 1, "surface": "actual-mcp-stdio", "provenance": suite["provenance"],
              "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "rust_cli_provenance": cli_metadata or "Not established; pass --cli to assert and fingerprint the selected repository binary.",
              "suite_sha256": hashlib.sha256(json.dumps(suite, sort_keys=True).encode()).hexdigest(),
              "repeats": args.repeats, "k": args.k, "latency_method": "monotonic wall time; nearest-rank percentiles; startup separate; no warmup discarded",
              "qualification": "conformance-only; real consented speech, independent held-out tasks and native workflows remain required",
              "runs": []}
    for size in sizes:
        try:
            report["runs"].append(run_size(suite, size, args.repeats, args.k,
                                          [args.node, str(args.server.resolve())], args.timeout))
            if args.cli and selected_cli_metadata(args.server, args.cli) != cli_metadata:
                report["runs"][-1]["passed"] = False
                report["runs"][-1]["provenance_error"] = "Selected CLI changed during evaluation"
        except Exception as exc:
            report["runs"].append({"requested_documents": size, "passed": False, "error": str(exc)})
    report["passed"] = all(run["passed"] for run in report["runs"])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"passed": report["passed"], "report": str(args.output), "qualification": report["qualification"]}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
