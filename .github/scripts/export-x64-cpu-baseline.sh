#!/usr/bin/env bash
set -euo pipefail

# whisper-rs-sys forwards these variables to CMake. Native tuning on an
# AVX-512 runner can put AVX-512 instructions in the shared Whisper CPU path.
case "${1:-}" in
  Linux|Windows) platform=$1 ;;
  *)
    echo "Usage: $0 {Linux|Windows}" >&2
    exit 2
    ;;
esac
: "${GITHUB_ENV:?GITHUB_ENV must point to the GitHub Actions environment file}"

{
  echo "GGML_NATIVE=OFF"
  echo "GGML_AVX=ON"
  echo "GGML_AVX2=ON"
  echo "GGML_AVX512=OFF"
  echo "GGML_AVX512_VBMI=OFF"
  echo "GGML_AVX512_VNNI=OFF"
  echo "GGML_AVX512_BF16=OFF"
  if [[ "$platform" == Linux ]]; then
    echo "GGML_SSE42=ON"
    echo "GGML_BMI2=ON"
    echo "GGML_FMA=ON"
    echo "GGML_F16C=ON"
    echo "GGML_AVX_VNNI=OFF"
    echo "GGML_AMX_TILE=OFF"
    echo "GGML_AMX_INT8=OFF"
    echo "GGML_AMX_BF16=OFF"
  else
    # Preserve the MSVC optimization lost when the cmake crate overwrites
    # CMake's release compiler flags (#1256).
    echo "CMAKE_C_FLAGS_RELEASE=/MD /O2 /Ob2 /DNDEBUG"
    echo "CMAKE_CXX_FLAGS_RELEASE=/MD /O2 /Ob2 /DNDEBUG"
  fi
} >> "$GITHUB_ENV"

echo "$platform Whisper CPU baseline: AVX2, with AVX-512 disabled"
