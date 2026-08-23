#!/usr/bin/env bash
set -euo pipefail

# Shipped Windows binaries must run on ordinary x86-64 consumer CPUs. GitHub's
# Windows runners can expose AVX-512, and whisper-rs-sys forwards these GGML_*
# variables into CMake. Without an explicit ceiling, a runner-specific native
# build can fault with STATUS_ILLEGAL_INSTRUCTION on machines such as Intel
# Arrow Lake and Lunar Lake, which intentionally do not implement AVX-512.
: "${GITHUB_ENV:?GITHUB_ENV must point to the GitHub Actions environment file}"

{
  echo "GGML_NATIVE=OFF"
  echo "GGML_AVX=ON"
  echo "GGML_AVX2=ON"
  echo "GGML_AVX512=OFF"
  echo "GGML_AVX512_VBMI=OFF"
  echo "GGML_AVX512_VNNI=OFF"
  echo "GGML_AVX512_BF16=OFF"
} >> "$GITHUB_ENV"

echo "Windows native CPU baseline: AVX2 maximum; AVX-512 disabled"
