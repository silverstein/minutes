#!/usr/bin/env bash
set -euo pipefail

# Installs the Tauri CLI from the prebuilt binary that tauri-apps publishes on
# its GitHub release, verified against a SHA-256 pinned here. Building it with
# `cargo install tauri-cli --locked` took 8.5 to 10.5 minutes on every macOS and
# Windows job that needed it, and the repository's Actions cache is too full to
# keep a compiled copy around between runs.
#
# Trust boundary: the pinned digests are the ones GitHub records for the
# tauri-cli-v<version> release assets. tauri-apps does not publish build
# attestations for them, so this trusts the same maintainers that
# `cargo install` trusts, through their release pipeline instead of crates.io.
#
# A version with no pinned digest for this runner falls back to the old
# from-source install, so bumping the version without new digests is slower but
# never broken. To add digests for a new version:
#   gh release view tauri-cli-v<version> -R tauri-apps/tauri --json assets \
#     --jq '.assets[] | "\(.name) \(.digest)"'

version="${1:?usage: install-tauri-cli.sh <version>}"
: "${RUNNER_OS:?RUNNER_OS must be set by the Actions runner}"
: "${RUNNER_ARCH:?RUNNER_ARCH must be set by the Actions runner}"

asset=""
digest=""
case "$version/$RUNNER_OS/$RUNNER_ARCH" in
  2.10.1/macOS/ARM64)
    asset=cargo-tauri-aarch64-apple-darwin.zip
    digest=f89d7d9485d3a6a6bcbc88275b0bddea61fe582ca636fc02c286994af1f6e36b ;;
  2.10.1/macOS/X64)
    asset=cargo-tauri-x86_64-apple-darwin.zip
    digest=5e37b09604cf4d16dc6e83423e728e10f376b737f6443d1dd3121b9f8f050a1b ;;
  2.10.1/Windows/X64)
    asset=cargo-tauri-x86_64-pc-windows-msvc.zip
    digest=50a3ed1c43f75ea3b6e7233a52d8a7c994cfffd98d8545ff397db8b464338d9c ;;
  2.10.1/Linux/X64)
    asset=cargo-tauri-x86_64-unknown-linux-gnu.tgz
    digest=ac869c0ecd657bf5fa9ae9bcc6664b1994b478abea512b3023c2e0d86e735833 ;;
esac

if [[ -z "$asset" ]]; then
  echo "::warning::No pinned prebuilt tauri-cli $version for $RUNNER_OS/$RUNNER_ARCH; building from source"
  cargo install tauri-cli --version "$version" --locked
else
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  curl --fail --silent --show-error --location --retry 3 \
    --output "$work/$asset" \
    "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v$version/$asset"

  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$work/$asset" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$work/$asset" | cut -d' ' -f1)"
  fi
  if [[ "$actual" != "$digest" ]]; then
    echo "tauri-cli $asset digest mismatch: expected $digest, got $actual" >&2
    exit 1
  fi

  mkdir -p "$work/unpacked"
  case "$asset" in
    *.zip) unzip -q "$work/$asset" -d "$work/unpacked" ;;
    *.tgz) tar -xzf "$work/$asset" -C "$work/unpacked" ;;
  esac

  bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
  mkdir -p "$bin_dir"
  exe=cargo-tauri
  [[ "$RUNNER_OS" == Windows ]] && exe=cargo-tauri.exe
  install -m 755 "$work/unpacked/$exe" "$bin_dir/$exe"
fi

installed="$(cargo tauri --version)"
if [[ "$installed" != "tauri-cli $version" ]]; then
  echo "expected tauri-cli $version, cargo tauri reports: $installed" >&2
  exit 1
fi
echo "Installed $installed"
