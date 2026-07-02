#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export CXXFLAGS="${CXXFLAGS:-"-I$(xcrun --show-sdk-path)/usr/include/c++/v1"}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
# engine-sherpa / vad-ort stay OPT-IN for app builds pending the #369 release
# decision. Packaging is fixed for opt-in local builds: sherpa links statically
# and the existing ort path does not leave dangling runtime dylib references.
# Opt in explicitly:
#   MINUTES_BUILD_FEATURES=parakeet,metal,vad-ort,engine-sherpa
if [ -z "${MINUTES_BUILD_FEATURES+x}" ]; then
    MINUTES_BUILD_FEATURES="parakeet,metal"
fi

# Match scripts/build.sh: route cargo through rustup so rust-toolchain.toml
# is honored and we don't drift from CI's clippy/rustfmt versions. Uses
# `rustup which cargo` so CARGO_HOME / non-default rustup paths still work.
RUSTUP_CARGO=""
if command -v rustup >/dev/null 2>&1; then
    RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
fi
if [[ -n "$RUSTUP_CARGO" ]]; then
    export PATH="$(dirname "$RUSTUP_CARGO"):$PATH"
fi
ACTIVE_CARGO="$(command -v cargo || true)"
if [[ -z "$ACTIVE_CARGO" ]]; then
    echo "Error: no cargo on PATH. Install rustup from https://rustup.rs and re-run."
    exit 1
fi
if [[ -n "$RUSTUP_CARGO" && "$ACTIVE_CARGO" != "$RUSTUP_CARGO" ]]; then
    echo "Warning: cargo at $ACTIVE_CARGO is not the rustup-managed cargo ($RUSTUP_CARGO); rust-toolchain.toml may be ignored."
fi

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
INSTALL_AFTER_BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-open)
      OPEN_AFTER_INSTALL=0
      ;;
    --target-only)
      OPEN_AFTER_INSTALL=0
      INSTALL_AFTER_BUILD=0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      echo "Usage: ./scripts/install-dev-app.sh [--no-open] [--target-only]" >&2
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
# Inside-out signing (#311): sign every nested executable FIRST (the CLI
# sidecar with its own entitlements), then the outer bundle WITHOUT --deep.
# --deep re-signs nested code with the outer entitlements (clobbering the
# sidecar's), and any post-seal patching of nested code invalidates the
# bundle seal, so copied/downloaded apps fail Gatekeeper as "damaged".
SIDECAR_BIN="$BUILD_APP/Contents/MacOS/minutes"
if [[ "$SIGN_MODE" == "identity" ]]; then
  echo "=== Signing nested executables (inside-out) with configured identity ==="
  while IFS= read -r nested_executable; do
    if [[ "$nested_executable" == "$SIDECAR_BIN" ]]; then
      codesign --force --options runtime --timestamp \
        --entitlements tauri/src-tauri/minutes-cli.entitlements \
        --sign "$SIGNING_IDENTITY" \
        "$nested_executable"
    else
      codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$nested_executable"
    fi
  done < <(find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \( -perm -100 -o -perm -010 -o -perm -001 \))

  echo "=== Signing ${DEV_PRODUCT_NAME}.app (outer, no --deep) ==="
  codesign --force --options runtime --timestamp \
    --entitlements tauri/src-tauri/entitlements.plist \
    --sign "$SIGNING_IDENTITY" \
    "$BUILD_APP"
else
  echo "=== Signing ${DEV_PRODUCT_NAME}.app ad-hoc (inside-out) ==="
  echo "No MINUTES_DEV_SIGNING_IDENTITY / APPLE_SIGNING_IDENTITY configured."
  echo "Using ad-hoc signing so the app remains runnable for contributors."
  echo "TCC-sensitive features may still require re-granting permissions after rebuilds."
  while IFS= read -r nested_executable; do
    if [[ "$nested_executable" == "$SIDECAR_BIN" ]]; then
      codesign --force --options runtime \
        --entitlements tauri/src-tauri/minutes-cli.entitlements \
        --sign - "$nested_executable"
    else
      codesign --force --sign - "$nested_executable"
    fi
  done < <(find "$BUILD_APP/Contents/MacOS" -maxdepth 1 -type f \( -perm -100 -o -perm -010 -o -perm -001 \))
  codesign --force --sign - "$BUILD_APP"
fi

echo "=== Verifying bundle seal (strict) ==="
codesign --verify --deep --strict "$BUILD_APP" && echo "  Seal OK"

if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo "=== Installing ${DEV_PRODUCT_NAME}.app to ${INSTALL_DIR} ==="
  mkdir -p "$INSTALL_DIR"
  rm -rf "$INSTALL_APP"
  cp -rf "$BUILD_APP" "$INSTALL_APP"

  echo "=== Running native hotkey diagnostic from installed dev app ==="
  set +e
  ./scripts/diagnose-desktop-hotkey.sh "$INSTALL_APP"
  DIAG_EXIT=$?
  set -e
else
  DIAG_EXIT="skipped"
fi

echo ""
if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo "Installed app: $INSTALL_APP"
else
  echo "Built app: $BUILD_APP"
fi
echo "Bundle id: com.useminutes.desktop.dev"
echo "Build features: $MINUTES_BUILD_FEATURES"
echo "Signing mode: $SIGN_MODE"
echo "Hotkey diagnostic exit code: $DIAG_EXIT"
echo "  0 = CGEventTap started successfully"
echo "  2 = Input Monitoring / macOS identity is still blocking the hotkey"
if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo ""
  echo "For TCC-sensitive testing, launch only this installed dev app."
  echo "Avoid the repo symlink (./Minutes.app), raw target bundles, or ad-hoc builds."
fi
if [[ "$SIGN_MODE" == "adhoc" ]]; then
  echo ""
  echo "Tip: export MINUTES_DEV_SIGNING_IDENTITY to a consistent local signing identity"
  echo "if you want more stable macOS permission behavior across rebuilds."
fi

if [[ "$INSTALL_AFTER_BUILD" == "0" ]]; then
  echo ""
  echo "Target-only mode: launch $BUILD_APP directly for packaging checks."
fi

if [[ "$OPEN_AFTER_INSTALL" == "1" && "$INSTALL_AFTER_BUILD" == "1" ]]; then
  echo ""
  echo "=== Launching ${DEV_PRODUCT_NAME}.app ==="
  open -a "$INSTALL_APP"
fi
