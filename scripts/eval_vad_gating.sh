#!/usr/bin/env bash
# Compare whole-file Whisper transcription with the same installed models.
# Usage: eval_vad_gating.sh --baseline BIN --candidate BIN --out DIR
#                           [--model small] [--corpus DIR]
# Run --self-test to check the metrics without loading Minutes or models.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 -I "$script_dir/eval_vad_gating.py" "$@"
