"""Exercise the actual CLI offline host without audio, keys, agents or network."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
if os.name == "nt":
    binary = binary.with_suffix(".exe")
with tempfile.TemporaryDirectory() as directory:
    env = dict(os.environ)
    env["MINUTES_HOME"] = str(Path(directory) / "minutes")
    env.pop("GEMINI_API_KEY", None)

    def run(commands):
        process = subprocess.run(
            [str(binary), "talk", "--local-work", "--json"],
            input="\n".join(commands + ["q", ""]), text=True,
            capture_output=True, timeout=30, env=env, check=True,
        )
        return [json.loads(line) for line in process.stdout.splitlines() if line.startswith("{")]

    events = run(["/work new Resume the proposal", "/work debrief I did not approve scope", "/work park", "/work list", "/work share", "/approve 1"])
    assert [event["type"] for event in events] == ["local"] * 4 + ["local_error"] * 2, events
    identifiers = json.loads(events[3]["text"])
    assert len(identifiers) == 1, identifiers  # debrief and park have identical contents
    events = run([f"/work resume {identifiers[0]}", "/work show"])
    assert all(event["type"] == "local" for event in events), events
    capsule = json.loads(events[1]["text"])
    assert capsule["goal"] == "Resume the proposal", capsule
    assert capsule["records"][0]["kind"] == "user_interpretation", capsule
    assert capsule["records"][0]["text"] == "I did not approve scope", capsule
print("Offline CLI create/debrief/park/list/resume and no-sharing smoke passed")
