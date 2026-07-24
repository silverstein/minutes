#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-${ROOT}/target/sidekick-eval/live-sidekick-engine-eval.json}"

cd "${ROOT}"
cargo run --quiet -p minutes-core --no-default-features \
  --example live_sidekick_engine_eval -- --out "${OUT}"
