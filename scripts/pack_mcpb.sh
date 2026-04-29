#!/usr/bin/env bash
#
# Pack the Claude Desktop MCPB using the Claude-specific listing manifest.
# The MCPB CLI always reads manifest.json from the package root, while this
# repo also uses manifest.json for broader agent docs. Stage a clean copy and
# swap manifest.mcpb.json into place only for the bundle.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-minutes.mcpb}"

if [[ "$output" != /* ]]; then
  output="$repo_root/$output"
fi

if [[ ! -f "$repo_root/manifest.mcpb.json" ]]; then
  echo "Missing Claude MCPB manifest: $repo_root/manifest.mcpb.json" >&2
  exit 1
fi

if ! command -v mcpb >/dev/null 2>&1; then
  echo "Missing mcpb CLI. Install with: npm install -g @anthropic-ai/mcpb" >&2
  exit 1
fi

tmp="$(mktemp -d)"
tmp="$(cd "$tmp" && pwd -P)"
trap 'rm -rf "$tmp"' EXIT

stage="$tmp/minutes"
mkdir -p "$stage"

rsync -a \
  --exclude='.git/' \
  --exclude='target/' \
  --exclude='site/' \
  --exclude='tauri/' \
  --exclude='.vercel/' \
  --exclude='.next/' \
  --exclude='minutes.mcpb' \
  "$repo_root/" "$stage/"

cp -f "$stage/manifest.mcpb.json" "$stage/manifest.json"
mcpb pack "$stage" "$output"
