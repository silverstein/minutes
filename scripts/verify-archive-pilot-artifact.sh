#!/bin/bash
set -euo pipefail

EXPECTED_TEAM_ID="63TMLKT8HN"
EXPECTED_IDENTIFIER="com.useminutes.archive"
ZIP_NAME="minutes-archive-pilot-notarized.zip"
SHA_NAME="${ZIP_NAME}.sha256"
PROVENANCE_NAME="signed-archive-provenance.txt"

fail() {
  printf 'Archive pilot artifact verification failed: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'Usage: %s <artifact-directory>\n' "$(basename "$0")" >&2
  printf 'The directory must contain %s, %s, and %s.\n' \
    "$ZIP_NAME" "$SHA_NAME" "$PROVENANCE_NAME" >&2
  exit 2
}

[[ $# -eq 1 ]] || usage
[[ "$(uname -s)" == "Darwin" ]] ||
  fail "verification requires macOS Gatekeeper, codesign, and stapler"

ARTIFACT_DIR="$1"
[[ -d "$ARTIFACT_DIR" ]] || fail "artifact directory does not exist"
ARTIFACT_DIR="$(cd "$ARTIFACT_DIR" && pwd -P)"

ZIP_PATH="$ARTIFACT_DIR/$ZIP_NAME"
SHA_PATH="$ARTIFACT_DIR/$SHA_NAME"
PROVENANCE_PATH="$ARTIFACT_DIR/$PROVENANCE_NAME"

[[ -f "$ZIP_PATH" ]] || fail "missing $ZIP_NAME"
[[ -f "$SHA_PATH" ]] || fail "missing $SHA_NAME"
[[ -f "$PROVENANCE_PATH" ]] || fail "missing $PROVENANCE_NAME"
[[ ! -L "$ZIP_PATH" && ! -L "$SHA_PATH" && ! -L "$PROVENANCE_PATH" ]] ||
  fail "artifact inputs must be regular files, not symbolic links"

sha_lines="$(wc -l <"$SHA_PATH" | tr -d '[:space:]')"
[[ "$sha_lines" == "1" ]] || fail "$SHA_NAME must contain exactly one line"
sha_fields="$(awk 'NR == 1 { print NF }' "$SHA_PATH")"
expected_zip_sha="$(awk 'NR == 1 { print $1 }' "$SHA_PATH")"
declared_zip_name="$(awk 'NR == 1 { print $2 }' "$SHA_PATH")"
[[ "$sha_fields" == "2" && "$declared_zip_name" == "$ZIP_NAME" ]] ||
  fail "$SHA_NAME must bind only $ZIP_NAME"
[[ ${#expected_zip_sha} -eq 64 && ! "$expected_zip_sha" =~ [^0-9a-f] ]] ||
  fail "$SHA_NAME does not contain a lowercase SHA-256"

actual_zip_sha="$(shasum -a 256 "$ZIP_PATH" | awk '{print $1}')"
[[ "$actual_zip_sha" == "$expected_zip_sha" ]] ||
  fail "notarized zip SHA-256 does not match"

provenance_lines="$(wc -l <"$PROVENANCE_PATH" | tr -d '[:space:]')"
[[ "$provenance_lines" == "6" ]] ||
  fail "$PROVENANCE_NAME must contain the six reviewed fields"

candidate_sha="$(sed -n 's/^candidate=//p' "$PROVENANCE_PATH")"
team_id="$(sed -n 's/^team_id=//p' "$PROVENANCE_PATH")"
identifier="$(sed -n 's/^identifier=//p' "$PROVENANCE_PATH")"
expected_executable_sha="$(sed -n 's/^executable_sha256=//p' "$PROVENANCE_PATH")"
notarized="$(sed -n 's/^notarized=//p' "$PROVENANCE_PATH")"
stapled="$(sed -n 's/^stapled=//p' "$PROVENANCE_PATH")"

[[ ${#candidate_sha} -eq 40 && ! "$candidate_sha" =~ [^0-9a-f] ]] ||
  fail "candidate provenance is not one exact lowercase commit SHA"
[[ "$team_id" == "$EXPECTED_TEAM_ID" ]] ||
  fail "provenance Team ID is not the reviewed Minutes team"
[[ "$identifier" == "$EXPECTED_IDENTIFIER" ]] ||
  fail "provenance bundle identifier is not the Archive production identifier"
[[ ${#expected_executable_sha} -eq 64 &&
  ! "$expected_executable_sha" =~ [^0-9a-f] ]] ||
  fail "executable provenance is not a lowercase SHA-256"
[[ "$notarized" == "true" && "$stapled" == "true" ]] ||
  fail "provenance does not claim both notarization and stapling"

expected_fields="$(
  grep -Ec \
    '^(candidate|team_id|identifier|executable_sha256|notarized|stapled)=' \
    "$PROVENANCE_PATH"
)"
[[ "$expected_fields" == "6" ]] ||
  fail "$PROVENANCE_NAME contains an unknown or duplicate field"

EXTRACT_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/minutes-archive-verify.XXXXXX")"
cleanup() {
  rm -rf "$EXTRACT_ROOT"
}
trap cleanup EXIT HUP INT TERM

ditto -x -k "$ZIP_PATH" "$EXTRACT_ROOT"
APP_PATH="$EXTRACT_ROOT/Minutes Archive.app"
EXECUTABLE="$APP_PATH/Contents/MacOS/minutes-archive-app"
INFO_PLIST="$APP_PATH/Contents/Info.plist"

[[ -d "$APP_PATH" && -x "$EXECUTABLE" && -f "$INFO_PLIST" ]] ||
  fail "zip does not contain the expected Minutes Archive application"
if find "$APP_PATH" -type l -print -quit | grep -q .; then
  fail "application contains a symbolic link"
fi

bundle_identifier="$(plutil -extract CFBundleIdentifier raw -o - "$INFO_PLIST")"
[[ "$bundle_identifier" == "$EXPECTED_IDENTIFIER" ]] ||
  fail "Info.plist bundle identifier does not match"

codesign --verify --deep --strict --verbose=4 "$APP_PATH"
identity="$(codesign -dv --verbose=4 "$APP_PATH" 2>&1)"
signed_team_id="$(awk -F= '/^TeamIdentifier=/{print $2}' <<<"$identity")"
signed_identifier="$(awk -F= '/^Identifier=/{print $2}' <<<"$identity")"
[[ "$signed_team_id" == "$EXPECTED_TEAM_ID" ]] ||
  fail "code signature Team ID does not match"
[[ "$signed_identifier" == "$EXPECTED_IDENTIFIER" ]] ||
  fail "code signature identifier does not match"
grep -Fq "Authority=Developer ID Application:" <<<"$identity" ||
  fail "application is not signed with a Developer ID Application identity"

# The hardened runtime must be on, or the forbidden-entitlement list below is
# moot: without it, DYLD_INSERT_LIBRARIES can inject into the process holding
# the in-memory index of privileged documents.
grep -Fq "flags=0x10000(runtime)" <<<"$identity" ||
  grep -Eq "flags=0x[0-9a-f]*10000" <<<"$identity" ||
  fail "application is not signed with the hardened runtime enabled"

entitlements_path="$EXTRACT_ROOT/entitlements.plist"
# Fail closed. This previously used `|| true` and then skipped the whole loop
# when the file was empty, so any codesign failure silently reported success
# on every forbidden entitlement.
if ! codesign -d --entitlements - "$APP_PATH" >"$entitlements_path" 2>/dev/null; then
  fail "could not read entitlements; refusing to certify the artifact"
fi
if [[ -s "$entitlements_path" ]]; then
  for forbidden_entitlement in \
    "com.apple.security.get-task-allow" \
    "com.apple.security.cs.disable-library-validation" \
    "com.apple.security.cs.allow-dyld-environment-variables" \
    "com.apple.security.cs.allow-unsigned-executable-memory"; do
    if plutil -p "$entitlements_path" |
      grep -Fq "\"$forbidden_entitlement\" => true"; then
      fail "forbidden entitlement enabled: $forbidden_entitlement"
    fi
  done
fi

xcrun stapler validate "$APP_PATH"
spctl --assess --type execute --verbose=4 "$APP_PATH"

actual_executable_sha="$(shasum -a 256 "$EXECUTABLE" | awk '{print $1}')"
[[ "$actual_executable_sha" == "$expected_executable_sha" ]] ||
  fail "signed executable SHA-256 does not match provenance"

printf 'artifact_verification=passed\n'
printf 'candidate_sha=%s\n' "$candidate_sha"
printf 'zip_sha256=%s\n' "$actual_zip_sha"
printf 'team_id=%s\n' "$signed_team_id"
printf 'identifier=%s\n' "$signed_identifier"
printf 'executable_sha256=%s\n' "$actual_executable_sha"
