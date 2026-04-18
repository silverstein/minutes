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

# Trees that should never appear in the MCPB — they are either the desktop
# app source, landing-page artifacts, or the Rust workspace. If any of these
# leak in, .mcpbignore fell out of sync and the bundle is wasting space at
# best (100MB+ landing-page chunks) or shipping path-traversal filenames
# Claude Desktop will reject at worst (#149).
forbidden_prefixes = (
    "site/",
    "tauri/",
    "target/",
    ".vercel/",
    ".next/",
    "crates/core/",
    "crates/cli/",
    "crates/reader/",
    "crates/whisper-guard/",
)

with zipfile.ZipFile(bundle) as zf:
    names = set(zf.namelist())
    missing = [path for path in required if path not in names]
    # Claude Desktop 1.3109.0 rejects any zip entry containing `..` as path
    # traversal, even when the `..` is literal chars inside a filename.
    # Next.js chunk filenames do this routinely, so a stray `.vercel/output/`
    # or `.next/` tree at repo root sinks the whole bundle (issue #149).
    path_traversal = sorted(n for n in names if ".." in n)
    forbidden = sorted(
        n for n in names if any(n.startswith(p) for p in forbidden_prefixes)
    )

if missing:
    print("MCPB bundle is missing required runtime files:", file=sys.stderr)
    for path in missing:
        print(f"  - {path}", file=sys.stderr)
    raise SystemExit(1)

if path_traversal:
    print(
        "MCPB bundle contains paths with '..' that Claude Desktop will reject "
        "as path traversal:",
        file=sys.stderr,
    )
    for path in path_traversal[:10]:
        print(f"  - {path}", file=sys.stderr)
    if len(path_traversal) > 10:
        print(f"  ... and {len(path_traversal) - 10} more", file=sys.stderr)
    print(
        "Usually caused by a stray .vercel/output/ or .next/ tree at repo "
        "root. Add those paths to .mcpbignore and repack.",
        file=sys.stderr,
    )
    raise SystemExit(1)

if forbidden:
    # Group by leaked top-level prefix so the fix is obvious.
    by_prefix = {}
    for n in forbidden:
        for p in forbidden_prefixes:
            if n.startswith(p):
                by_prefix.setdefault(p, []).append(n)
                break
    print(
        "MCPB bundle contains trees that should not be packed:",
        file=sys.stderr,
    )
    for prefix, paths in by_prefix.items():
        print(f"  {prefix} ({len(paths)} files)", file=sys.stderr)
        for path in paths[:3]:
            print(f"    - {path}", file=sys.stderr)
        if len(paths) > 3:
            print(f"    ... and {len(paths) - 3} more", file=sys.stderr)
    print(
        "Each leaked prefix must be added to .mcpbignore. The first offender "
        "historically was a repo-root `.vercel/output/` tree from `vercel "
        "build` during release packaging (#149).",
        file=sys.stderr,
    )
    raise SystemExit(1)

print(f"MCPB bundle looks healthy: {bundle}")
PY
