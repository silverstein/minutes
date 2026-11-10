#!/usr/bin/env python3
"""Unix fresh-process CPU controls for an explicit public WAV and local model."""

import argparse
import json
from pathlib import Path
import resource
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", help="built dictation_release_profile example")
    parser.add_argument("model", help="existing Whisper model; never downloaded")
    parser.add_argument("wav", help="public PCM16 WAV; results contain transcripts")
    parser.add_argument("output", type=Path, help="directory for explicit run evidence")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    for iteration in range(3):
        modes = ["--fresh-preview", "--fresh-final"]
        if iteration % 2:
            modes.reverse()
        for mode in modes:
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            started = time.monotonic()
            run = subprocess.run(
                [args.binary, args.model, args.wav, mode],
                capture_output=True,
                text=True,
                check=False,
            )
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            stem = f"fresh-{iteration}-{mode[2:]}"
            (args.output / f"{stem}.stdout").write_text(run.stdout)
            (args.output / f"{stem}.stderr").write_text(run.stderr)
            run.check_returncode()
            row = json.loads(run.stdout)
            row.update(
                iteration=iteration,
                mode=mode,
                wall_seconds=time.monotonic() - started,
                child_cpu_seconds=(after.ru_utime + after.ru_stime)
                - (before.ru_utime + before.ru_stime),
            )
            rows.append(row)
    result = {
        "measurement": "separate fresh processes, alternating order; OS/backend caches "
        "not cleared; CPU covers model setup, replay and teardown, not GPU time or battery",
        "all_text_identical": len({row["replay"]["text"] for row in rows}) == 1,
        "runs": rows,
    }
    (args.output / "fresh-process-controls.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    if not result["all_text_identical"]:
        raise SystemExit("fresh-process text mismatch; raw evidence preserved")


if __name__ == "__main__":
    main()
