#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_PATH="${1:-/Users/silverbook/Applications/Minutes Archive Dev.app}"
APP_EXECUTABLE="$APP_PATH/Contents/MacOS/minutes-archive-app"

cd "$REPO_ROOT"

if [[ ! -d "$APP_PATH" || ! -x "$APP_EXECUTABLE" ]]; then
  echo "Installed Archive development app not found: $APP_PATH" >&2
  exit 1
fi

echo "=== Bundle seal ==="
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "=== Focused Rust tests ==="
cargo fmt --all -- --check
cargo test \
  -p minutes-archive-semantic \
  -p minutes-archive-convert \
  -p minutes-archive-core \
  -p minutes-archive-app

echo "=== Strict focused lint ==="
cargo clippy \
  -p minutes-archive-semantic \
  -p minutes-archive-convert \
  -p minutes-archive-core \
  -p minutes-archive-app \
  --all-targets -- -D warnings

echo "=== macOS Archive dependency boundary ==="
ARCHIVE_TREE="$(cargo tree -p minutes-archive-app --target aarch64-apple-darwin)"
if grep -Fq "quick-xml v0.37.5" <<<"$ARCHIVE_TREE"; then
  echo "The vulnerable quick-xml 0.37.5 release entered the Archive macOS tree." >&2
  exit 1
fi
if ! grep -Fq "quick-xml v0.41.0" <<<"$ARCHIVE_TREE"; then
  echo "The expected patched quick-xml 0.41.0 release was not found." >&2
  exit 1
fi

echo "=== Installed-executable document and worker smoke ==="
cargo run -p minutes-archive-core --example document_vault_smoke -- "$APP_EXECUTABLE"

# Three documents cannot show a build stopping at 237 of 16,621, and a run
# started from a terminal never has the 256-descriptor ceiling launchd gives a
# GUI app. This one has both: archive scale, the app's real limit.
echo "=== Archive-scale soak under the GUI descriptor ceiling ==="
cargo run -p minutes-archive-core --example archive_pilot_soak -- "$APP_EXECUTABLE"

echo "=== Deterministic UI interaction smoke ==="
node scripts/archive-ui-smoke.mjs

echo "=== Installed native window lifecycle smoke ==="
scripts/archive-native-lifecycle-smoke.sh "$APP_PATH"

echo "=== Notarized artifact verifier harness ==="
scripts/verify-archive-pilot-artifact.test.sh

echo "=== Synthetic human-QA fixture generator ==="
scripts/make-archive-qa-fixtures.test.sh

echo "=== Artifact identity ==="
codesign -dv --verbose=4 "$APP_PATH" 2>&1 |
  grep -E "^(Identifier|Signature|TeamIdentifier)="
shasum -a 256 "$APP_EXECUTABLE"

echo "Archive development release candidate verification passed."
