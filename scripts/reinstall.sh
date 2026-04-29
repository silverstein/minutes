#!/bin/bash
# Reinstall Minutes extension into Claude Desktop
# Usage: ./scripts/reinstall.sh

set -e

EXT_DIR="$HOME/Library/Application Support/Claude/Claude Extensions/local.mcpb.mat-silverstein.minutes"
MCPB_FILE="minutes.mcpb"
LOG_FILE="$HOME/Library/Logs/Claude/mcp-server-Minutes - Meeting Workspace for Claude.log"

echo "=== Minutes Extension Reinstall ==="
echo ""

# Step 1: Build MCP server (TS + UI)
echo "[1/4] Building MCP server..."
(cd crates/mcp && npm run build) 2>&1 | tail -1

# Step 2: Pack MCPB
echo "[2/4] Packing MCPB..."
./scripts/pack_mcpb.sh "$MCPB_FILE" 2>&1 | tail -1

# Step 3: Remove old extension
if [ -d "$EXT_DIR" ]; then
  echo "[3/4] Removing old extension..."
  rm -rf "$EXT_DIR"
  echo "      Removed: $(basename "$EXT_DIR")"
else
  echo "[3/4] No existing extension found (clean install)"
fi

# Step 4: Install new extension by opening the .mcpb file
echo "[4/4] Installing new extension..."
open "$MCPB_FILE"

echo ""
echo "=== Done ==="
echo "Claude Desktop should prompt to install the extension."
echo ""
echo "To tail logs after testing:"
echo "  tail -f '$LOG_FILE'"
