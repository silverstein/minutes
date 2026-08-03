#!/usr/bin/env bash
# Run the Coach model freshness eval on this machine and file the monthly report.
#
# This used to run on a GitHub-hosted macOS runner. It produced nothing usable:
# every candidate errored, mostly because a hosted runner cannot serve a 9B or
# 12B local model (see #620). The report's question is "has a better local model
# appeared for Coach", and that only means something measured on hardware
# resembling where the model would actually run.
#
# Intended to be driven by the LaunchAgent in
# tauri/src-tauri/assets/app.minutes.coacheval.plist, but safe to run by hand.

set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir/.." rev-parse --show-toplevel)"
cd "$repo_root"

log() { printf '%s coach-eval: %s\n' "$(date -u +%H:%M:%S)" "$*"; }

# ── Guards ───────────────────────────────────────────────────────────────────
# Never compete with a capture. Loading a multi-billion-parameter model pins the
# GPU and memory, and the standing rule here is that an optional consumer must
# never degrade recording. A month is a long window; skipping this run costs
# nothing and the next one picks it up.
if [[ -f "$HOME/.minutes/recording.pid" ]]; then
  log "a recording is in progress; skipping this run"
  exit 0
fi

if ! curl --fail --silent --max-time 5 http://localhost:11434/api/tags >/dev/null; then
  log "Ollama is not serving on :11434; skipping (start Ollama.app and it will run next month)"
  exit 0
fi

# On battery this would be both slow and rude. Only meaningful on laptops.
if command -v pmset >/dev/null && ! pmset -g ps 2>/dev/null | grep -q "AC Power"; then
  log "on battery; skipping this run"
  exit 0
fi

# ── Run ──────────────────────────────────────────────────────────────────────
report="$repo_root/coach-model-report.md"

log "building the eval CLI"
cargo build -p minutes-cli --no-default-features --release

log "evaluating small-tier candidates (this pulls models and can take a while)"
# nice: this is background housekeeping and must yield to anything interactive.
nice -n 10 python3 scripts/coach_model_eval.py \
  --minutes-bin target/release/minutes \
  --small-only \
  --output "$report"

# ── File the report ──────────────────────────────────────────────────────────
if [[ ! -s "$report" ]]; then
  log "no report produced; nothing to file"
  exit 1
fi

month="$(date -u +%Y-%m)"
title="Coach model freshness report $month"
body="$repo_root/coach-eval-issue-body.md"

python3 - "$report" "$body" <<'PY'
import sys
from pathlib import Path

report = Path(sys.argv[1]).read_text(encoding="utf-8")
suffix = "\n\n_Generated locally on the maintainer's Mac; see scripts/coach-model-eval-local.sh._\n"
Path(sys.argv[2]).write_text(report[:60000] + suffix, encoding="utf-8")
PY

existing="$(gh issue list --state all --limit 100 --json number,title \
  --jq ".[] | select(.title == \"$title\") | .number" | head -n 1)"

if [[ -n "$existing" ]]; then
  log "updating existing issue #$existing"
  gh issue reopen "$existing" >/dev/null 2>&1 || true
  gh issue edit "$existing" --body-file "$body"
else
  log "opening a new issue"
  gh issue create --title "$title" --body-file "$body"
fi

rm -f "$body"
log "done"
