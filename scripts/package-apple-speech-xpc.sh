#!/bin/bash
# Move Tauri's staged Apple Speech worker into a one-purpose XPC service,
# sign it inside-out, and seal its exact CodeDirectory hash into the parent.
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "Usage: $0 APP_BUNDLE SIGNING_IDENTITY ENTITLEMENTS_PLIST" >&2
  exit 2
fi

APP_BUNDLE="$1"
SIGNING_IDENTITY="$2"
ENTITLEMENTS_PLIST="$3"
SOURCE_WORKER="$APP_BUNDLE/Contents/MacOS/minutes-apple-speech-worker"
XPC_BUNDLE="$APP_BUNDLE/Contents/XPCServices/com.useminutes.apple-speech-worker.xpc"
XPC_CONTENTS="$XPC_BUNDLE/Contents"
XPC_EXECUTABLE="$XPC_CONTENTS/MacOS/minutes-apple-speech-worker"
XPC_INFO="$XPC_CONTENTS/Info.plist"
WORKER_CDHASH="$APP_BUNDLE/Contents/Resources/minutes-apple-speech-worker.cdhash"

test -d "$APP_BUNDLE"
test -f "$SOURCE_WORKER"
test ! -L "$SOURCE_WORKER"
test -f "$ENTITLEMENTS_PLIST"
file "$SOURCE_WORKER" | grep -q "Mach-O"

rm -rf "$XPC_BUNDLE"
mkdir -p "$XPC_CONTENTS/MacOS"
mv -f "$SOURCE_WORKER" "$XPC_EXECUTABLE"
chmod 755 "$XPC_EXECUTABLE"
test ! -e "$SOURCE_WORKER"

APP_VERSION="$(
  /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
    "$APP_BUNDLE/Contents/Info.plist"
)"
python3 - \
  "crates/cli/assets/minutes-apple-speech-worker-Info.plist" \
  "$XPC_INFO" \
  "$APP_VERSION" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text()
if source.count("__MINUTES_VERSION__") != 2:
    raise SystemExit("Apple Speech XPC Info.plist version markers are ambiguous")
version = sys.argv[3]
if not version or any(character not in "0123456789.-" for character in version):
    raise SystemExit("Apple Speech XPC app version is invalid")
pathlib.Path(sys.argv[2]).write_text(source.replace("__MINUTES_VERSION__", version))
PY
plutil -lint "$XPC_INFO" >/dev/null

if [[ "$SIGNING_IDENTITY" == "-" ]]; then
  codesign --force --options runtime \
    --entitlements "$ENTITLEMENTS_PLIST" \
    --identifier com.useminutes.apple-speech-worker \
    --sign - \
    "$XPC_BUNDLE"
else
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS_PLIST" \
    --identifier com.useminutes.apple-speech-worker \
    --sign "$SIGNING_IDENTITY" \
    "$XPC_BUNDLE"
fi
codesign --verify --strict --verbose=4 "$XPC_BUNDLE"

worker_cdhash="$(
  codesign -dvvv "$XPC_EXECUTABLE" 2>&1 |
    awk -F= '/^CDHash=/{print tolower($2); exit}'
)"
if [[ ! "$worker_cdhash" =~ ^[0-9a-f]{40}$ ]]; then
  echo "Signed Apple Speech XPC worker lacked one exact CodeDirectory hash." >&2
  exit 1
fi
mkdir -p "$(dirname "$WORKER_CDHASH")"
printf '%s\n' "$worker_cdhash" > "$WORKER_CDHASH"
chmod 444 "$WORKER_CDHASH"

APP_EXECUTABLE_NAME="$(
  /usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' \
    "$APP_BUNDLE/Contents/Info.plist"
)"
APP_EXECUTABLE="$APP_BUNDLE/Contents/MacOS/$APP_EXECUTABLE_NAME"
python3 scripts/seal_apple_speech_worker_hash.py \
  "$APP_EXECUTABLE" "$worker_cdhash"
python3 scripts/seal_apple_speech_worker_hash.py \
  --verify "$APP_EXECUTABLE" "$worker_cdhash"
