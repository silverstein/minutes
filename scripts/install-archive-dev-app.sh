#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

RUSTUP_CARGO=""
if command -v rustup >/dev/null 2>&1; then
  RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
fi
if [[ -n "$RUSTUP_CARGO" ]]; then
  export PATH="$(dirname "$RUSTUP_CARGO"):$PATH"
fi

PRODUCT_NAME="Minutes Archive Dev"
BUILD_APP="$REPO_ROOT/target/release/bundle/macos/${PRODUCT_NAME}.app"
INSTALL_DIR="${INSTALL_DIR:-$HOME/Applications}"
INSTALL_APP="${INSTALL_DIR}/${PRODUCT_NAME}.app"
SIGNING_IDENTITY="${ARCHIVE_DEV_SIGNING_IDENTITY:-${MINUTES_DEV_SIGNING_IDENTITY:-${APPLE_SIGNING_IDENTITY:-}}}"
OPEN_AFTER_INSTALL=1

for arg in "$@"; do
  case "$arg" in
    --no-open)
      OPEN_AFTER_INSTALL=0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      echo "Usage: ./scripts/install-archive-dev-app.sh [--no-open]" >&2
      exit 1
      ;;
  esac
done

if [[ -n "$SIGNING_IDENTITY" ]] && ! security find-identity -v -p codesigning | grep -Fq "$SIGNING_IDENTITY"; then
  echo "Signing identity not found: $SIGNING_IDENTITY" >&2
  echo "Set ARCHIVE_DEV_SIGNING_IDENTITY to a valid code-signing identity." >&2
  exit 1
fi

echo "=== Building ${PRODUCT_NAME}.app ==="
(
  cd archive/src-tauri
  cargo tauri build --bundles app --config tauri.dev.conf.json --no-sign
)

if [[ ! -d "$BUILD_APP" ]]; then
  echo "Expected app bundle was not produced: $BUILD_APP" >&2
  exit 1
fi

if [[ -n "$SIGNING_IDENTITY" ]]; then
  echo "=== Signing with configured identity ==="
  while IFS= read -r executable; do
    codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$executable"
  done < <(
    find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \
      \( -perm -100 -o -perm -010 -o -perm -001 \)
  )
  codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$BUILD_APP"
  SIGNING_MODE="identity"
else
  echo "=== Signing ad hoc for local development ==="
  while IFS= read -r executable; do
    codesign --force --sign - "$executable"
  done < <(
    find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \
      \( -perm -100 -o -perm -010 -o -perm -001 \)
  )
  codesign --force --sign - "$BUILD_APP"
  SIGNING_MODE="ad-hoc"
fi

echo "=== Verifying bundle seal ==="
codesign --verify --deep --strict "$BUILD_APP"

echo "=== Installing to ${INSTALL_APP} ==="
mkdir -p "$INSTALL_DIR"
STAGING_APP="${INSTALL_DIR}/.${PRODUCT_NAME}.installing.$$"
rm -rf "$STAGING_APP"
ditto "$BUILD_APP" "$STAGING_APP"
rm -rf "$INSTALL_APP"
mv -f "$STAGING_APP" "$INSTALL_APP"
codesign --verify --deep --strict "$INSTALL_APP"

echo ""
echo "Installed app: $INSTALL_APP"
echo "Bundle id: com.useminutes.archive.dev"
echo "Signing mode: $SIGNING_MODE"

if [[ "$OPEN_AFTER_INSTALL" == "1" ]]; then
  echo "=== Launching ${PRODUCT_NAME}.app ==="
  open -a "$INSTALL_APP"
fi
