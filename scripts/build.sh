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

# Code signing + notarization are optional for local source builds.
# Maintainers can export APPLE_SIGNING_IDENTITY / APPLE_API_* when they want
# cargo-tauri to produce a signed + notarized bundle.

echo "=== Building CLI (release) ==="
cargo build --release -p minutes-cli --features metal

echo "=== Building calendar helper ==="
swiftc -O \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist \
    -Xlinker scripts/calendar-helper-Info.plist \
    scripts/calendar-events.swift -o target/release/calendar-events
echo "  Built target/release/calendar-events"

echo "=== Building Tauri app ==="
cargo tauri build --features metal --bundles app

echo "=== Embedding calendar helper in app bundle ==="
APP_RESOURCES="target/release/bundle/macos/Minutes.app/Contents/Resources"
mkdir -p "$APP_RESOURCES"
cp -f target/release/calendar-events "$APP_RESOURCES/calendar-events"
echo "  Embedded in $APP_RESOURCES/"

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
