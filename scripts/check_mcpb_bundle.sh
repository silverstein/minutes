#!/usr/bin/env bash
set -euo pipefail

bundle_path="${1:-minutes.mcpb}"

if [[ ! -f "$bundle_path" ]]; then
  echo "Missing MCPB bundle: $bundle_path" >&2
  exit 1
fi

python3 - <<'PY' "$bundle_path"
import sys
import zipfile

bundle = sys.argv[1]
required = [
    "crates/mcp/dist/index.js",
    "crates/mcp/node_modules/yaml/dist/nodes/addPairToJSMap.js",
    "crates/mcp/node_modules/yaml/dist/schema/yaml-1.1/merge.js",
    "crates/mcp/node_modules/yaml/dist/schema/yaml-1.1/schema.js",
]

with zipfile.ZipFile(bundle) as zf:
    names = set(zf.namelist())
    missing = [path for path in required if path not in names]

if missing:
    print("MCPB bundle is missing required runtime files:", file=sys.stderr)
    for path in missing:
        print(f"  - {path}", file=sys.stderr)
    raise SystemExit(1)

print(f"MCPB bundle looks healthy: {bundle}")
PY
