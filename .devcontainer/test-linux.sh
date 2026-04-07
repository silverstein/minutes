#!/usr/bin/env bash
# End-to-end Linux sanity check for Minutes inside the Codespace.
# Exercises the full pipeline on the bundled demo.wav so you can verify
# whisper, diarization, and audio device enumeration all work on Linux
# without needing a microphone.
set -uo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

PASS=0
FAIL=0

run() {
  local name="$1"
  shift
  echo ""
  echo "─── $name ──────────────────────────────────────────────"
  echo "\$ $*"
  if "$@"; then
    echo "✓ PASS: $name"
    PASS=$((PASS+1))
  else
    echo "✗ FAIL: $name (exit $?)"
    FAIL=$((FAIL+1))
  fi
}

echo "════════════════════════════════════════════════════════════"
echo " Minutes — Linux sanity tests"
echo "════════════════════════════════════════════════════════════"

run "minutes --version" \
  minutes --version

run "minutes health (json)" \
  minutes health --json

run "minutes paths" \
  minutes paths

run "list audio devices (cpal enumeration on ALSA/PipeWire)" \
  minutes devices

run "list audio sources (capture vs loopback categorization)" \
  minutes sources

# demo.wav is a synthetic 3-second silence-plus-beep file (see crates/cli/src/main.rs:3940).
# A successful run will produce words=0 in the JSON output — that's correct, not a bug.
# What we're verifying here is that the full pipeline (decode → VAD → whisper → diarize →
# markdown write) executes end-to-end without panicking, not that it produces a transcript.
run "process bundled demo.wav as a meeting (whisper end-to-end)" \
  minutes process crates/assets/demo.wav -t meeting --title "Linux smoke test"

run "list recent meetings" \
  minutes list --limit 5

run "show extracted action items" \
  minutes actions

run "core unit tests (no whisper feature)" \
  cargo test -p minutes-core --no-default-features --lib -- --test-threads=1

run "diarize unit tests (cross-platform ONNX)" \
  cargo test -p minutes-core --no-default-features --features diarize --lib -- diarize --test-threads=1

run "whisper-guard unit tests" \
  cargo test -p whisper-guard

run "reader crate tests" \
  cargo test -p minutes-reader

run "MCP TypeScript build" \
  bash -c "cd crates/mcp && npx tsc --noEmit"

echo ""
echo "════════════════════════════════════════════════════════════"
echo " Results: $PASS passed, $FAIL failed"
echo "════════════════════════════════════════════════════════════"

exit $FAIL
