#!/bin/bash
set -euo pipefail

APP_PATH="${1:-/Users/silverbook/Applications/Minutes Archive Dev.app}"
APP_EXECUTABLE="$APP_PATH/Contents/MacOS/minutes-archive-app"

if [[ ! -x "$APP_EXECUTABLE" ]]; then
  echo "Installed Archive executable not found: $APP_EXECUTABLE" >&2
  exit 1
fi

SMOKE_DIR="$(mktemp -d)"
SMOKE_LOG="$SMOKE_DIR/native-lifecycle.log"
cleanup() {
  rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

"$APP_EXECUTABLE" --archive-native-lifecycle-selftest >"$SMOKE_LOG" 2>&1 &
APP_PID=$!

for _ in {1..200}; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    break
  fi
  sleep 0.05
done

if kill -0 "$APP_PID" 2>/dev/null; then
  kill -TERM "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  echo "Archive native lifecycle self-test did not exit after closing its window." >&2
  sed -n '1,120p' "$SMOKE_LOG" >&2
  exit 1
fi

if ! wait "$APP_PID"; then
  echo "Archive native lifecycle self-test exited unsuccessfully." >&2
  sed -n '1,120p' "$SMOKE_LOG" >&2
  exit 1
fi

grep -Fxq "archive_native_window=visible" "$SMOKE_LOG"
grep -Fxq "archive_native_close=requested" "$SMOKE_LOG"
grep -Fxq "archive_native_close_event=received" "$SMOKE_LOG"

echo "archive_native_lifecycle=passed window=visible close=purged"
