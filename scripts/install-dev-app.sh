#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export CXXFLAGS="${CXXFLAGS:-"-I$(xcrun --show-sdk-path)/usr/include/c++/v1"}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
MINUTES_BUILD_FEATURES="${MINUTES_BUILD_FEATURES:-parakeet,metal}"

DEV_CONFIG="tauri/src-tauri/tauri.dev.conf.json"
DEV_PRODUCT_NAME="Minutes Dev"
BUILD_APP="target/release/bundle/macos/${DEV_PRODUCT_NAME}.app"
INSTALL_DIR="${INSTALL_DIR:-$HOME/Applications}"
INSTALL_APP="${INSTALL_DIR}/${DEV_PRODUCT_NAME}.app"
SIGNING_IDENTITY="${MINUTES_DEV_SIGNING_IDENTITY:-${APPLE_SIGNING_IDENTITY:-}}"
SIGN_MODE="adhoc"

run_with_ort_retry() {
  local _build_tmp
  _build_tmp=$(mktemp)
  if ! "$@" 2>&1 | tee "$_build_tmp"; then
    if grep -q "library 'clang_rt\." "$_build_tmp"; then
      echo ""
      echo "  Stale ort-sys clang runtime path (Xcode/CLT upgrade detected)."
      echo "  Cleaning stale build cache and retrying..."
      rm -rf target/*/build/ort-sys-*
      rm -f "$_build_tmp"
      "$@"
      return
    fi
    rm -f "$_build_tmp"
    return 1
  fi
  rm -f "$_build_tmp"
}

OPEN_AFTER_INSTALL=1
for arg in "$@"; do
  case "$arg" in
    --no-open)
      OPEN_AFTER_INSTALL=0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      echo "Usage: ./scripts/install-dev-app.sh [--no-open]" >&2
      exit 1
      ;;
  esac
done

if [[ -n "$SIGNING_IDENTITY" ]]; then
  if ! security find-identity -v -p codesigning | grep -Fq "$SIGNING_IDENTITY"; then
    echo "Signing identity not found: $SIGNING_IDENTITY" >&2
    echo "Set MINUTES_DEV_SIGNING_IDENTITY (preferred) or APPLE_SIGNING_IDENTITY to a valid codesigning identity in your keychain." >&2
    exit 1
  fi
  SIGN_MODE="identity"
fi

echo "=== Building CLI (release) ==="
run_with_ort_retry cargo build --release -p minutes-cli --features "$MINUTES_BUILD_FEATURES"

echo "=== Staging CLI as Tauri sidecar ==="
HOST_TARGET="$(rustc -Vv | awk '/host:/ {print $2}')"
mkdir -p tauri/src-tauri/bin
cp -f target/release/minutes "tauri/src-tauri/bin/minutes-${HOST_TARGET}"

echo "=== Building ${DEV_PRODUCT_NAME}.app ==="
# The calendar-events Swift helper is compiled and staged into
# tauri/src-tauri/resources/ by tauri/src-tauri/build.rs, and Tauri bundles it
# into the .app automatically via tauri.conf.json.
run_with_ort_retry cargo tauri build --bundles app --config "$DEV_CONFIG" --features "$MINUTES_BUILD_FEATURES" --no-sign
if [[ "$SIGN_MODE" == "identity" ]]; then
  echo "=== Pre-signing nested executables with configured identity ==="
  while IFS= read -r nested_executable; do
    codesign --force --options runtime --timestamp \
      --sign "$SIGNING_IDENTITY" \
      "$nested_executable"
  done < <(find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \( -perm -100 -o -perm -010 -o -perm -001 \))

  echo "=== Signing ${DEV_PRODUCT_NAME}.app with configured identity ==="
  codesign --force --deep --options runtime --timestamp \
    --entitlements tauri/src-tauri/entitlements.plist \
    --sign "$SIGNING_IDENTITY" \
    "$BUILD_APP"
else
  echo "=== Signing ${DEV_PRODUCT_NAME}.app ad-hoc ==="
  echo "No MINUTES_DEV_SIGNING_IDENTITY / APPLE_SIGNING_IDENTITY configured."
  echo "Using ad-hoc signing so the app remains runnable for contributors."
  echo "TCC-sensitive features may still require re-granting permissions after rebuilds."
  codesign --force --deep --sign - "$BUILD_APP"
fi

echo "=== Re-signing bundled CLI sidecar with its own entitlements ==="
# Outer --deep sign above clobbers nested entitlements; re-sign the CLI sidecar
# afterwards with mic input entitlement so `minutes record` from a terminal
# triggers TCC instead of failing silently. Ad-hoc-signed sidecars don't get
# entitlements honored — that's documented in §1a of the plan.
# Tauri's bundler strips the target-triple from externalBin names, so the
# sidecar lands at `Contents/MacOS/minutes` (not `minutes-${HOST_TARGET}`).
SIDECAR_RESIGN="${BUILD_APP}/Contents/MacOS/minutes"
if [[ -f "$SIDECAR_RESIGN" ]]; then
  if [[ "$SIGN_MODE" == "identity" ]]; then
    codesign --force --options runtime --timestamp \
      --entitlements tauri/src-tauri/minutes-cli.entitlements \
      --sign "$SIGNING_IDENTITY" \
      "$SIDECAR_RESIGN"
  else
    codesign --force --options runtime \
      --entitlements tauri/src-tauri/minutes-cli.entitlements \
      --sign - \
      "$SIDECAR_RESIGN"
  fi
else
  echo "  WARNING: sidecar not found at $SIDECAR_RESIGN — skipping re-sign."
fi

echo "=== Installing ${DEV_PRODUCT_NAME}.app to ${INSTALL_DIR} ==="
mkdir -p "$INSTALL_DIR"
rm -rf "$INSTALL_APP"
cp -rf "$BUILD_APP" "$INSTALL_APP"

echo "=== Running native hotkey diagnostic from installed dev app ==="
set +e
./scripts/diagnose-desktop-hotkey.sh "$INSTALL_APP"
DIAG_EXIT=$?
set -e

echo ""
echo "Installed app: $INSTALL_APP"
echo "Bundle id: com.useminutes.desktop.dev"
echo "Build features: $MINUTES_BUILD_FEATURES"
echo "Signing mode: $SIGN_MODE"
echo "Hotkey diagnostic exit code: $DIAG_EXIT"
echo "  0 = CGEventTap started successfully"
echo "  2 = Input Monitoring / macOS identity is still blocking the hotkey"
echo ""
echo "For TCC-sensitive testing, launch only this installed dev app."
echo "Avoid the repo symlink (./Minutes.app), raw target bundles, or ad-hoc builds."
if [[ "$SIGN_MODE" == "adhoc" ]]; then
  echo ""
  echo "Tip: export MINUTES_DEV_SIGNING_IDENTITY to a consistent local signing identity"
  echo "if you want more stable macOS permission behavior across rebuilds."
fi

if [[ "$OPEN_AFTER_INSTALL" == "1" ]]; then
  echo ""
  echo "=== Launching ${DEV_PRODUCT_NAME}.app ==="
  open -a "$INSTALL_APP"
fi
