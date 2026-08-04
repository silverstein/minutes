#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERIFIER="$REPO_ROOT/scripts/verify-archive-pilot-artifact.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/minutes-archive-verifier-test.XXXXXX")"

cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT HUP INT TERM

MOCK_BIN="$TEST_ROOT/bin"
ARTIFACT_DIR="$TEST_ROOT/artifacts"
SOURCE_ROOT="$TEST_ROOT/source"
APP_PATH="$SOURCE_ROOT/Minutes Archive.app"
EXECUTABLE="$APP_PATH/Contents/MacOS/minutes-archive-app"
ZIP_NAME="minutes-archive-pilot-notarized.zip"
SHA_NAME="${ZIP_NAME}.sha256"
PROVENANCE_NAME="signed-archive-provenance.txt"

mkdir -p "$MOCK_BIN" "$APP_PATH/Contents/MacOS"
printf 'synthetic signed executable fixture\n' >"$EXECUTABLE"
chmod 755 "$EXECUTABLE"
printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '<key>CFBundleIdentifier</key><string>com.useminutes.archive</string>' \
  '</dict></plist>' \
  >"$APP_PATH/Contents/Info.plist"

printf '%s\n' \
  '#!/bin/bash' \
  "if [[ \"\$1\" == \"-dv\" ]]; then" \
  '  printf "Identifier=com.useminutes.archive\n" >&2' \
  '  printf "Authority=Developer ID Application: Test (63TMLKT8HN)\n" >&2' \
  "  printf \"TeamIdentifier=%s\\n\" \"\${ARCHIVE_TEST_TEAM_ID:-63TMLKT8HN}\" >&2" \
  "  printf \"CodeDirectory v=20500 flags=%s\\n\" \"\${ARCHIVE_TEST_CS_FLAGS:-0x10000(runtime)}\" >&2" \
  'fi' \
  'exit 0' \
  >"$MOCK_BIN/codesign"
printf '%s\n' '#!/bin/bash' 'exit 0' >"$MOCK_BIN/xcrun"
printf '%s\n' '#!/bin/bash' 'exit 0' >"$MOCK_BIN/spctl"
chmod 755 "$MOCK_BIN/codesign" "$MOCK_BIN/xcrun" "$MOCK_BIN/spctl"

make_artifact() {
  rm -rf "$ARTIFACT_DIR"
  mkdir -p "$ARTIFACT_DIR"
  ditto -c -k --sequesterRsrc --keepParent \
    "$APP_PATH" "$ARTIFACT_DIR/$ZIP_NAME"
  (
    cd "$ARTIFACT_DIR"
    shasum -a 256 "$ZIP_NAME" >"$SHA_NAME"
  )
  executable_sha="$(shasum -a 256 "$EXECUTABLE" | awk '{print $1}')"
  printf '%s\n' \
    "candidate=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
    "team_id=63TMLKT8HN" \
    "identifier=com.useminutes.archive" \
    "executable_sha256=$executable_sha" \
    "notarized=true" \
    "stapled=true" \
    >"$ARTIFACT_DIR/$PROVENANCE_NAME"
}

expect_failure() {
  description="$1"
  shift
  if "$@" >"$TEST_ROOT/failure.out" 2>&1; then
    printf 'Expected failure did not occur: %s\n' "$description" >&2
    exit 1
  fi
}

make_artifact
PATH="$MOCK_BIN:$PATH" "$VERIFIER" "$ARTIFACT_DIR" >"$TEST_ROOT/success.out"
grep -Fq "artifact_verification=passed" "$TEST_ROOT/success.out"

make_artifact
printf '0  %s\n' "$ZIP_NAME" >"$ARTIFACT_DIR/$SHA_NAME"
expect_failure "mismatched zip digest" \
  env PATH="$MOCK_BIN:$PATH" "$VERIFIER" "$ARTIFACT_DIR"

make_artifact
sed -i '' 's/^team_id=.*/team_id=WRONGTEAM/' \
  "$ARTIFACT_DIR/$PROVENANCE_NAME"
expect_failure "wrong provenance team" \
  env PATH="$MOCK_BIN:$PATH" "$VERIFIER" "$ARTIFACT_DIR"

make_artifact
expect_failure "wrong code-signing team" \
  env PATH="$MOCK_BIN:$PATH" ARCHIVE_TEST_TEAM_ID=WRONGTEAM \
  "$VERIFIER" "$ARTIFACT_DIR"

make_artifact
# Without the hardened runtime the forbidden-entitlement list below is moot:
# DYLD_INSERT_LIBRARIES can inject into the process holding the in-memory
# index of privileged documents.
expect_failure "not signed with the hardened runtime" \
  env PATH="$MOCK_BIN:$PATH" ARCHIVE_TEST_CS_FLAGS="0x2(adhoc)" \
  "$VERIFIER" "$ARTIFACT_DIR"

make_artifact
sed -i '' 's/^stapled=true$/unexpected=true/' \
  "$ARTIFACT_DIR/$PROVENANCE_NAME"
expect_failure "unexpected provenance field" \
  env PATH="$MOCK_BIN:$PATH" "$VERIFIER" "$ARTIFACT_DIR"

printf 'archive_pilot_artifact_verifier_tests=passed\n'
