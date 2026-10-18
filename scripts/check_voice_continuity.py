"""Lint changed Rust lines and format changed files in the real feature build.

The voice-only/all-targets build has pre-existing warnings in unrelated capture,
knowledge and graph code. We retain those diagnostics, reject every compiler
error, and fail warnings that touch this change. This is NOT a clean-workspace
Clippy claim. The standalone contract harness still uses -D warnings globally.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

BASE = "5a9d59fcba09d8b81dd72c96f538a7508b7a6505"


def changed_ranges(diff):
    ranges = {}
    current = None
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            current = line[6:]
            ranges.setdefault(current, [])
        elif line.startswith("+++ "):
            current = None
        elif current and line.startswith("@@ "):
            match = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", line)
            if not match:
                raise ValueError("unrecognized diff hunk")
            start = int(match[1])
            count = int(match[2]) if match[2] is not None else 1
            if count:
                ranges[current].append((start, start + count - 1))
    return ranges


def touches_change(message, ranges):
    for span in message.get("spans", []):
        path = span["file_name"].replace("\\", "/")
        root = Path.cwd().as_posix().rstrip("/") + "/"
        if path.startswith(root):
            path = path[len(root):]
        for start, end in ranges.get(path, []):
            if span["line_start"] <= end and span["line_end"] >= start:
                return True
    return any(touches_change(child, ranges) for child in message.get("children", []))


def self_test():
    ranges = changed_ranges("+++ b/new.rs\n@@ -0,0 +1,30 @@\n+++ b/old.rs\n@@ -9 +9,2 @@\n@@ -40,2 +41,0 @@\n")
    assert ranges == {"new.rs": [(1, 30)], "old.rs": [(9, 10)]}
    def diagnostic(path, start, end):
        return {"spans": [{"file_name": path, "line_start": start, "line_end": end}]}
    assert touches_change(diagnostic("new.rs", 1, 1), ranges)
    assert touches_change(diagnostic("old.rs", 10, 12), ranges)
    assert not touches_change(diagnostic("old.rs", 1, 8), ranges)
    assert not touches_change(diagnostic("untouched.rs", 1, 100), ranges)
    assert touches_change({"children": [diagnostic("old.rs", 9, 9)]}, ranges)
    assert touches_change(diagnostic("src\\new.rs", 1, 1), {"src/new.rs": [(1, 1)]})
    print("Changed-code lint boundary self-tests passed")
    subprocess.run(["node", "--test", "tooling/voice-evals/artifact-bridge.test.mjs"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["lint", "fmt", "self-test"])
    args = parser.parse_args()
    self_test()
    if args.mode == "self-test":
        return 0
    diff = subprocess.check_output(
        ["git", "-c", "core.quotePath=false", "diff", "--no-ext-diff", "--unified=0", BASE, "HEAD", "--", "*.rs"],
        text=True, encoding="utf-8",
    )
    ranges = changed_ranges(diff)
    if not ranges:
        raise RuntimeError("Expected integration Rust changes relative to pinned base")
    if args.mode == "fmt":
        for path in sorted(ranges):
            if Path(path).is_file():
                subprocess.run(["rustfmt", "--edition", "2021", "--check", "--config", "skip_children=true", path], check=True)
        print(f"Formatting passed for {len(ranges)} changed Rust files")
        return 0
    command = ["cargo", "clippy", "-p", "minutes-core", "-p", "minutes-cli", "--no-default-features", "--features", "voice-live", "--all-targets", "--message-format=json"]
    failed = 0
    unrelated = 0
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    assert process.stdout is not None
    with Path("continuity-clippy.jsonl").open("w", encoding="utf-8") as log:
        for line in process.stdout:
            log.write(line)
            item = json.loads(line)
            if item.get("reason") != "compiler-message":
                continue
            message = item["message"]
            if message.get("level") not in ("error", "warning"):
                continue
            relevant = message["level"] == "error" or touches_change(message, ranges)
            if relevant:
                failed += 1
                print(message.get("rendered") or message["message"], file=sys.stderr)
            else:
                unrelated += 1
                primary = next((s for s in message.get("spans", []) if s.get("is_primary")), {})
                print(f"EXISTING/UNCHANGED warning: {primary.get('file_name', '?')}:{primary.get('line_start', '?')}: {message['message']}")
    code = process.wait()
    print(f"Integrated changed-code lint: {failed} blocking diagnostics; {unrelated} warnings outside changed lines (retained in continuity-clippy.jsonl)")
    return 1 if code or failed else 0


if __name__ == "__main__":
    sys.exit(main())
