#!/usr/bin/env python3
import json
import sys
import zipfile
from pathlib import Path


REQUIRED = [
    "manifest.json",
    "crates/mcp/dist/index.js",
    "crates/mcp/dist-ui/index.html",
    "crates/mcp/node_modules/@modelcontextprotocol/ext-apps/package.json",
    "crates/mcp/node_modules/minutes-sdk/dist/index.js",
    "crates/mcp/node_modules/yaml/dist/nodes/addPairToJSMap.js",
    "crates/mcp/node_modules/yaml/dist/schema/yaml-1.1/merge.js",
    "crates/mcp/node_modules/yaml/dist/schema/yaml-1.1/schema.js",
]

# Trees that should never appear in the MCPB. If any of these leak in,
# .mcpbignore fell out of sync and the bundle is wasting space at best
# (100MB+ landing-page chunks) or shipping path-traversal filenames Claude
# Desktop will reject at worst (#149).
FORBIDDEN_PREFIXES = (
    ".cargo/",
    ".claude-plugin/",
    ".devcontainer/",
    ".opencode/",
    "site/",
    "tauri/",
    "target/",
    ".vercel/",
    ".next/",
    "crates/core/",
    "crates/cli/",
    "crates/reader/",
    "crates/whisper-guard/",
    "crates/assets/",
    "crates/sdk/",
    "examples/",
    "tooling/",
    "crates/mcp/node_modules/.bin/",
)

FORBIDDEN_EXACT = (
    "nudge.json",
    "crates/mcp/node_modules/.package-lock.json",
)


def runtime_dependency_names() -> set[str]:
    lock_path = Path("crates/mcp/package-lock.json")
    if not lock_path.exists():
        return set()

    with lock_path.open() as f:
        lock = json.load(f)

    allowed: set[str] = set()
    for package_path, metadata in lock.get("packages", {}).items():
        if not package_path.startswith("node_modules/"):
            continue
        if metadata.get("dev") is True:
            continue

        parts = package_path.split("/")
        if len(parts) < 2:
            continue
        if parts[1].startswith("@"):
            if len(parts) >= 3:
                allowed.add("/".join(parts[1:3]))
        else:
            allowed.add(parts[1])

    return allowed


def bundled_node_package(path: str) -> str | None:
    prefix = "crates/mcp/node_modules/"
    if not path.startswith(prefix):
        return None

    rest = path[len(prefix) :]
    if not rest or rest.startswith("."):
        return None

    parts = rest.split("/")
    if parts[0].startswith("@"):
        if len(parts) < 2:
            return None
        return "/".join(parts[:2])

    return parts[0]


def report_forbidden(paths: list[str]) -> None:
    by_prefix: dict[str, list[str]] = {}
    for name in paths:
        for prefix in FORBIDDEN_PREFIXES:
            if name.startswith(prefix):
                by_prefix.setdefault(prefix, []).append(name)
                break
        else:
            by_prefix.setdefault(name, []).append(name)

    print("MCPB bundle contains trees that should not be packed:", file=sys.stderr)
    for prefix, names in by_prefix.items():
        print(f"  {prefix} ({len(names)} files)", file=sys.stderr)
        for path in names[:3]:
            print(f"    - {path}", file=sys.stderr)
        if len(names) > 3:
            print(f"    ... and {len(names) - 3} more", file=sys.stderr)
    print(
        "Each leaked prefix must be added to .mcpbignore. The first offender "
        "historically was a repo-root `.vercel/output/` tree from `vercel "
        "build` during release packaging (#149).",
        file=sys.stderr,
    )


def check_bundle(bundle: str) -> None:
    with zipfile.ZipFile(bundle) as zf:
        names = set(zf.namelist())
        packed_manifest = (
            json.loads(zf.read("manifest.json")) if "manifest.json" in names else {}
        )

    missing = [path for path in REQUIRED if path not in names]
    # Claude Desktop 1.3109.0 rejects any zip entry containing `..` as path
    # traversal, even when the `..` is literal chars inside a filename.
    # Next.js chunk filenames do this routinely, so a stray `.vercel/output/`
    # or `.next/` tree at repo root sinks the whole bundle (issue #149).
    path_traversal = sorted(name for name in names if ".." in name)
    forbidden = sorted(
        name
        for name in names
        if any(name.startswith(prefix) for prefix in FORBIDDEN_PREFIXES)
        or name in FORBIDDEN_EXACT
    )
    allowed_runtime_deps = runtime_dependency_names()
    bundled_deps = {
        package
        for package in (bundled_node_package(name) for name in names)
        if package is not None
    }
    unexpected_deps = sorted(bundled_deps - allowed_runtime_deps)

    expected_manifest_path = Path("manifest.mcpb.json")
    if expected_manifest_path.exists():
        with expected_manifest_path.open() as f:
            expected_manifest = json.load(f)
        mismatched_manifest_fields = [
            field
            for field in ("display_name", "description", "long_description", "version")
            if packed_manifest.get(field) != expected_manifest.get(field)
        ]
    else:
        expected_manifest = {}
        mismatched_manifest_fields = []

    root_manifest_path = Path("manifest.json")
    if root_manifest_path.exists() and expected_manifest:
        with root_manifest_path.open() as f:
            root_manifest = json.load(f)
        listing_fields = {"display_name", "description", "long_description"}
        drifted_manifest_fields = [
            field
            for field in sorted(set(root_manifest) | set(expected_manifest))
            if field not in listing_fields
            and root_manifest.get(field) != expected_manifest.get(field)
        ]
    else:
        drifted_manifest_fields = []

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
        report_forbidden(forbidden)
        raise SystemExit(1)

    if unexpected_deps:
        print(
            "MCPB bundle contains crates/mcp/node_modules packages that are not "
            "runtime dependencies:",
            file=sys.stderr,
        )
        for package in unexpected_deps:
            print(f"  - {package}", file=sys.stderr)
        print(
            "These are usually dev-only packages from npm install. Add their "
            "paths to .mcpbignore or prune dev dependencies before packing.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    if mismatched_manifest_fields:
        print(
            "MCPB bundle manifest.json does not match manifest.mcpb.json for "
            "Claude listing fields:",
            file=sys.stderr,
        )
        for field in mismatched_manifest_fields:
            print(f"  - {field}", file=sys.stderr)
        print(
            "Pack with scripts/pack_mcpb.sh so the Claude-specific listing is "
            "used inside minutes.mcpb.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    if drifted_manifest_fields:
        print(
            "manifest.mcpb.json has drifted from manifest.json outside Claude "
            "listing copy:",
            file=sys.stderr,
        )
        for field in drifted_manifest_fields:
            print(f"  - {field}", file=sys.stderr)
        print(
            "Keep runtime fields in sync; only display_name, description, and "
            "long_description should differ.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    print(f"MCPB bundle looks healthy: {bundle}")


def main() -> None:
    if len(sys.argv) != 2:
        print("Usage: check_mcpb_bundle.py path/to/minutes.mcpb", file=sys.stderr)
        raise SystemExit(2)
    check_bundle(sys.argv[1])


if __name__ == "__main__":
    main()
