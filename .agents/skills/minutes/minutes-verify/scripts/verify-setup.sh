#!/usr/bin/env bash
# Minutes setup verification script
# Checks all components needed for recording, transcription, and storage.

set -euo pipefail

PASS="PASS"
FAIL="FAIL"
WARN="WARN"
errors=0

check() {
  local label="$1" status="$2" detail="${3:-}"
  if [ "$status" = "$PASS" ]; then
    printf "  %-20s %s" "$label" "$PASS"
  elif [ "$status" = "$WARN" ]; then
    printf "  %-20s %s" "$label" "$WARN"
  else
    printf "  %-20s %s" "$label" "$FAIL"
    errors=$((errors + 1))
  fi
  [ -n "$detail" ] && printf "  (%s)" "$detail"
  printf "\n"
}

echo "Minutes Health Check"
echo "===================="
echo ""

# 1. Binary
if command -v minutes &>/dev/null; then
  version=$(minutes --version 2>/dev/null || echo "unknown")
  check "Binary" "$PASS" "$version"
else
  check "Binary" "$FAIL" "minutes not found on PATH"
fi

# 2. Whisper model
model_found=false
model_detail=""
for dir in "$HOME/.minutes/models" "$HOME/.cache/whisper"; do
  if [ -d "$dir" ]; then
    count=$(find "$dir" -name "*.bin" -o -name "ggml-*.bin" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$count" -gt 0 ]; then
      model_found=true
      model_detail="$count model(s) in $dir"
      break
    fi
  fi
done
if $model_found; then
  check "Whisper model" "$PASS" "$model_detail"
else
  check "Whisper model" "$FAIL" "no models found — run: minutes setup --model small"
fi

# 3. Meetings directory
if [ -d "$HOME/meetings" ]; then
  count=$(find "$HOME/meetings" -maxdepth 1 -name "*.md" 2>/dev/null | wc -l | tr -d ' ')
  check "Meetings dir" "$PASS" "$count meeting(s)"
else
  check "Meetings dir" "$FAIL" "~/meetings/ does not exist"
fi

# 4. Memos directory
if [ -d "$HOME/meetings/memos" ]; then
  count=$(find "$HOME/meetings/memos" -maxdepth 1 -name "*.md" 2>/dev/null | wc -l | tr -d ' ')
  check "Memos dir" "$PASS" "$count memo(s)"
else
  check "Memos dir" "$WARN" "~/meetings/memos/ does not exist — will be created on first memo"
fi

# 5. PID state
pid_file="$HOME/.minutes/recording.pid"
if [ -f "$pid_file" ]; then
  pid=$(cat "$pid_file" 2>/dev/null || echo "")
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    check "PID state" "$PASS" "recording active (PID $pid)"
  else
    check "PID state" "$FAIL" "stale PID file — run: rm $pid_file"
  fi
else
  check "PID state" "$PASS" "no active recording"
fi

# 6. Audio input (macOS only)
if [ "$(uname)" = "Darwin" ]; then
  if system_profiler SPAudioDataType 2>/dev/null | grep -q "Input Source"; then
    check "Audio input" "$PASS" "input device available"
  else
    check "Audio input" "$WARN" "no input source detected — check System Settings > Sound"
  fi
else
  check "Audio input" "$WARN" "skipped (non-macOS)"
fi

# 7. Config file (optional)
config_file="$HOME/.config/minutes/config.toml"
if [ -f "$config_file" ]; then
  check "Config" "$PASS" "$config_file"
else
  check "Config" "$WARN" "not configured — using defaults (this is fine)"
fi

echo ""
if [ "$errors" -eq 0 ]; then
  echo "All checks passed. Minutes is ready to use."
else
  echo "$errors check(s) failed. Fix the issues above before recording."
fi

exit "$errors"
