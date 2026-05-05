#!/bin/bash
# Build everything: CLI, Tauri app, and optional production-style install (macOS only)
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Error: build.sh is macOS-only (requires xcrun, swiftc, codesign)."
    echo "For cross-platform CLI builds: cargo build --release -p minutes-cli"
    exit 1
fi

export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
MINUTES_BUILD_FEATURES="${MINUTES_BUILD_FEATURES:-parakeet,metal}"

# Ensure cargo runs through rustup so rust-toolchain.toml is honored.
# Without this, a system Homebrew rustc (e.g. /opt/homebrew/bin/cargo)
# silently ignores the pin and produces local-vs-CI drift on clippy lints
# that fire only on the pinned version. PR #206's CI failure was exactly
# this: Rust 1.95 lint fired on CI, Homebrew's 1.94 ignored it locally.
#
# Detection uses `rustup which cargo` rather than a hardcoded path so
# CARGO_HOME / non-default rustup install locations work too.
RUSTUP_CARGO=""
if command -v rustup >/dev/null 2>&1; then
    RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
fi
if [[ -n "$RUSTUP_CARGO" ]]; then
    RUSTUP_CARGO_DIR="$(dirname "$RUSTUP_CARGO")"
    export PATH="$RUSTUP_CARGO_DIR:$PATH"
fi
ACTIVE_CARGO="$(command -v cargo || true)"
if [[ -z "$ACTIVE_CARGO" ]]; then
    echo "Error: no cargo on PATH. Install rustup from https://rustup.rs and re-run."
    exit 1
fi
if [[ -n "$RUSTUP_CARGO" && "$ACTIVE_CARGO" != "$RUSTUP_CARGO" ]]; then
    echo "Warning: cargo at $ACTIVE_CARGO is not the rustup-managed cargo ($RUSTUP_CARGO)."
    echo "         rust-toolchain.toml may be silently ignored, causing local-vs-CI clippy drift."
    echo "         Fix: prepend rustup's bin dir to PATH in your shell, or 'brew uninstall rust' if installed via Homebrew."
fi
if [[ -z "$RUSTUP_CARGO" ]]; then
    echo "Note: rustup not found; using cargo at $ACTIVE_CARGO directly."
    echo "      rust-toolchain.toml requires rustup to be honored — install from https://rustup.rs for full reproducibility."
fi

# Code signing + notarization are optional for local source builds.
# Maintainers can export APPLE_SIGNING_IDENTITY / APPLE_API_* when they want
# cargo-tauri to produce a signed + notarized bundle.

echo "=== Building CLI (release) ==="
_build_tmp=$(mktemp)
if ! cargo build --release -p minutes-cli --features "$MINUTES_BUILD_FEATURES" 2>&1 | tee "$_build_tmp"; then
    if grep -q "library 'clang_rt\." "$_build_tmp"; then
        echo ""
        echo "  Stale ort-sys clang runtime path (Xcode/CLT upgrade detected)."
        echo "  Cleaning stale build cache and retrying..."
        rm -rf target/*/build/ort-sys-*
        cargo build --release -p minutes-cli --features "$MINUTES_BUILD_FEATURES"
    else
        rm -f "$_build_tmp"
        exit 1
    fi
fi
rm -f "$_build_tmp"

echo "=== Staging CLI as Tauri sidecar ==="
# v1: aarch64-only sidecars. x86_64 cross-compile is a v2 follow-up. The Tauri
# sidecar convention requires an arch-suffixed filename.
HOST_TARGET="$(rustc -Vv | awk '/host:/ {print $2}')"
mkdir -p tauri/src-tauri/bin
cp -f target/release/minutes "tauri/src-tauri/bin/minutes-${HOST_TARGET}"

echo "=== Building Tauri app ==="
# The calendar-events Swift helper is compiled and staged into
# tauri/src-tauri/resources/ by tauri/src-tauri/build.rs, and Tauri bundles it
# into Minutes.app/Contents/Resources/ automatically via tauri.conf.json.
TAURI_BUILD_ARGS=(cargo tauri build --features "$MINUTES_BUILD_FEATURES" --bundles app)
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    echo "  No TAURI_SIGNING_PRIVATE_KEY configured; building updater artifacts with --no-sign."
    TAURI_BUILD_ARGS+=(--no-sign)
fi
"${TAURI_BUILD_ARGS[@]}"

echo "=== Re-signing bundled CLI sidecar with its own entitlements ==="
# The CLI sidecar needs `com.apple.security.device.audio-input` so `minutes record`
# from a terminal hits the macOS TCC mic prompt instead of silently failing. The
# outer `cargo tauri build` (with `--deep` under the hood) clobbers any nested
# entitlements, so we explicitly re-sign the sidecar AFTER the bundle is built.
#
# Ad-hoc signing fallback for OSS contributors: TCC entitlements are largely
# ignored without a Team ID, so contributor builds will still see the TCC denial
# on first terminal `minutes record`. The setup UI surfaces this when the
# running bundle is ad-hoc-signed (detected via codesign -dv).
SIGN_ID="${APPLE_SIGNING_IDENTITY:-${MINUTES_DEV_SIGNING_IDENTITY:--}}"
APP_BUNDLE="target/release/bundle/macos/Minutes.app"
# Tauri's bundler strips the target-triple suffix from externalBin names when
# copying into the .app — the on-disk filename is `minutes`, not
# `minutes-${HOST_TARGET}`. Both the package input ($HOST_TARGET file in
# tauri/src-tauri/bin/) and the bundled output (plain `minutes`) are required.
SIDECAR="${APP_BUNDLE}/Contents/MacOS/minutes"
if [[ -f "$SIDECAR" ]]; then
    codesign --force --options runtime \
        --entitlements tauri/src-tauri/minutes-cli.entitlements \
        --sign "$SIGN_ID" \
        "$SIDECAR"
    echo "  Signed sidecar with identity: $SIGN_ID"
else
    echo "  WARNING: expected sidecar not found at $SIDECAR — skipping re-sign."
fi

APP_VERSION="$(python3 - <<'PY'
import json
from pathlib import Path
print(json.loads(Path("tauri/src-tauri/tauri.conf.json").read_text())["version"])
PY
)"
./scripts/create-branded-dmg.sh \
    --app target/release/bundle/macos/Minutes.app \
    --version "$APP_VERSION" \
    --output "target/release/bundle/dmg/Minutes_${APP_VERSION}_aarch64.dmg"

echo "=== Signing + Installing CLI ==="
mkdir -p ~/.local/bin
codesign -s - -f target/release/minutes 2>/dev/null || true
cp -f target/release/minutes ~/.local/bin/minutes && echo "  Installed to ~/.local/bin/"

echo ""

# Install to /Applications if --install flag is passed
if [[ " $* " == *" --install "* ]]; then
    echo "=== Installing app to /Applications ==="
    cp -rf target/release/bundle/macos/Minutes.app /Applications/
    echo "  Installed to /Applications/Minutes.app"
fi

echo "=== Done ==="
echo "  Build features: $MINUTES_BUILD_FEATURES"
RESOLVED="$(which minutes 2>/dev/null || true)"
if [ -n "$RESOLVED" ]; then
    echo "  CLI:  $RESOLVED — $("$RESOLVED" --version 2>&1)"
else
    echo "  CLI:  ~/.local/bin/minutes (not in PATH) — $(~/.local/bin/minutes --version 2>&1 || echo 'unknown')"
fi
if [ -n "$RESOLVED" ]; then
    RESOLVED_REAL="$(readlink -f "$RESOLVED" 2>/dev/null || echo "$RESOLVED")"
    EXPECTED_REAL="$(readlink -f "$HOME/.local/bin/minutes" 2>/dev/null || echo "$HOME/.local/bin/minutes")"
fi
if [ -n "$RESOLVED" ] && [ "$RESOLVED_REAL" != "$EXPECTED_REAL" ]; then
    echo ""
    echo "  ⚠  PATH shadowing: 'minutes' resolves to $RESOLVED"
    echo "     The build installed to ~/.local/bin/minutes but a stale binary takes priority."
    if [[ "$RESOLVED" == */homebrew/* ]] || [[ "$RESOLVED" == */Cellar/* ]]; then
        echo "     Fix: brew unlink minutes"
    elif [[ "$RESOLVED" == */.cargo/bin/* ]]; then
        echo "     Fix: cargo uninstall minutes"
    else
        echo "     Fix: rm '$RESOLVED'"
    fi
fi
echo "  App:  target/release/bundle/macos/Minutes.app"
echo ""
if [ -d "/Applications/Minutes.app" ]; then
    echo "  Relaunch: open /Applications/Minutes.app"
else
    echo "  Launch: open target/release/bundle/macos/Minutes.app"
    echo "  Install: ./scripts/build.sh --install"
fi
echo "  Dev app (stable TCC identity): ./scripts/install-dev-app.sh"
