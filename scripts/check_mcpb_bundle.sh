#!/usr/bin/env bash
set -euo pipefail

bundle_path="${1:-minutes.mcpb}"

if [[ ! -f "$bundle_path" ]]; then
  echo "Missing MCPB bundle: $bundle_path" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$script_dir/check_mcpb_bundle.py" "$bundle_path"
