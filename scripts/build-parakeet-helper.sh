#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
HELPER_DIR="$ROOT_DIR/helpers/parakeet-coreml"
TARGET_BIN="$HOME/.local/bin/parakeet-coreml"

cd "$HELPER_DIR"
swift build -c release

mkdir -p "$HOME/.local/bin"
cp -f .build/release/parakeet-coreml "$TARGET_BIN"

echo "Installed parakeet-coreml to $TARGET_BIN"
