#!/usr/bin/env bash
# Install the optional local Git hooks for fast feedback before a push.
# Local hooks are bypassable with `git push --no-verify`; CI remains authoritative.

set -euo pipefail

usage() {
  echo "Usage: scripts/setup-hooks.sh [--force]" >&2
}

force=false
case "${1:-}" in
  "") ;;
  --force) force=true ;;
  *)
    usage
    exit 2
    ;;
esac

if (( $# > 1 )); then
  usage
  exit 2
fi

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if ! repo_root="$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null)"; then
  echo "setup-hooks: could not find the Git repository containing this script" >&2
  exit 1
fi

existing="$(git -C "$repo_root" config --get core.hooksPath || true)"
if [[ -n "$existing" && "$existing" != ".githooks" && "$force" != true ]]; then
  echo "setup-hooks: refusing to replace existing core.hooksPath '$existing' with '.githooks'; rerun with --force to override" >&2
  exit 1
fi

git -C "$repo_root" config --local core.hooksPath .githooks

if [[ -n "$existing" && "$existing" != ".githooks" ]]; then
  echo "setup-hooks: replaced core.hooksPath '$existing' with '.githooks' for $repo_root"
else
  echo "setup-hooks: set core.hooksPath to '.githooks' for $repo_root"
fi
