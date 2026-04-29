#!/usr/bin/env bash
#
# Smoke-test the packed MCPB: extract it, run the bundled
# crates/mcp/dist/index.js under the same Node that ships in CI, send the
# exact Claude Desktop 1.3109.0 initialize JSON-RPC, and verify the server
# replies with protocolVersion 2025-11-25.
#
# This catches regressions that the static bundle check cannot: broken
# runtime imports, SDK major bumps, argv/__filename comparison breakage,
# server crashing during handler registration, etc.
#
# Usage: scripts/smoke_mcpb_handshake.sh path/to/minutes.mcpb

set -euo pipefail

bundle_path="${1:-minutes.mcpb}"

if [[ ! -f "$bundle_path" ]]; then
  echo "Missing MCPB bundle: $bundle_path" >&2
  exit 1
fi

tmp="$(mktemp -d)"
# Resolve symlinks so process.argv[1] matches fileURLToPath(import.meta.url)
# inside the server. On macOS, mktemp returns /var/folders/... which is a
# symlink to /private/var/folders/...; without this, the server's
# `resolve(process.argv[1]) === __filename` guard fails, main() never runs,
# and the test sees a silent no-response that isn't actually a regression.
tmp="$(cd "$tmp" && pwd -P)"
trap 'rm -rf "$tmp"' EXIT

unzip -q "$bundle_path" -d "$tmp"

if [[ ! -f "$tmp/crates/mcp/dist/index.js" ]]; then
  echo "Packed bundle is missing crates/mcp/dist/index.js" >&2
  exit 1
fi

initialize='{"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}}},"clientInfo":{"name":"ci-smoke","version":"0"}},"jsonrpc":"2.0","id":0}'
initialized='{"method":"notifications/initialized","params":{},"jsonrpc":"2.0"}'
tools_list='{"method":"tools/list","params":{},"jsonrpc":"2.0","id":1}'
resources_list='{"method":"resources/list","params":{},"jsonrpc":"2.0","id":2}'
read_dashboard='{"method":"resources/read","params":{"uri":"ui://minutes/dashboard"},"jsonrpc":"2.0","id":3}'
read_status='{"method":"resources/read","params":{"uri":"minutes://status"},"jsonrpc":"2.0","id":4}'

out="$tmp/out.txt"
err="$tmp/err.txt"

# Pipe stdin EOF after writing the initialize; the server should respond
# and then exit cleanly when stdin closes. 15s timeout guards against the
# server hanging. Pick the first available timeout binary so the script
# works on Ubuntu CI (timeout), macOS (gtimeout via coreutils), or neither.
timeout_cmd=""
for candidate in timeout gtimeout; do
  if command -v "$candidate" >/dev/null 2>&1; then
    timeout_cmd="$candidate"
    break
  fi
done

if [[ -n "$timeout_cmd" ]]; then
  printf '%s\n' "$initialize" "$initialized" "$tools_list" "$resources_list" "$read_dashboard" "$read_status" | \
    "$timeout_cmd" 15 node "$tmp/crates/mcp/dist/index.js" >"$out" 2>"$err" || rc=$?
else
  # No timeout binary available. Fall back to running without one — node
  # exits on stdin EOF so a healthy server still returns quickly.
  printf '%s\n' "$initialize" "$initialized" "$tools_list" "$resources_list" "$read_dashboard" "$read_status" | \
    node "$tmp/crates/mcp/dist/index.js" >"$out" 2>"$err" || rc=$?
fi

rc="${rc:-0}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$script_dir/smoke_mcpb_handshake.py" "$out" "$err" "$rc"
