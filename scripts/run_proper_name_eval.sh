#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "" ]; then
  echo "Usage: scripts/run_proper_name_eval.sh /abs/path/to/corpus.json"
  exit 1
fi

CORPUS_PATH="$1"
if [ ! -f "$CORPUS_PATH" ]; then
  echo "Corpus file not found: $CORPUS_PATH"
  exit 1
fi

export MINUTES_PROPER_NAME_EVAL_CORPUS="$CORPUS_PATH"

CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1" \
  cargo test -p minutes-core --features whisper,parakeet proper_name_eval_corpus -- --ignored --nocapture
