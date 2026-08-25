#!/bin/bash
set -euo pipefail

AUDIO="${1:-crates/assets/demo.wav}"
ITERATIONS="${2:-10}"
CHUNK_MS="${3:-120}"
PRESET="${4:-speech-progressive}"
EXPECTATIONS="${5:--}"
TARGET_MS="${6:-}"
OUTPUT="$(mktemp -d)/minutes-apple-speech-streaming-benchmark"

if [[ -z "$TARGET_MS" ]]; then
  if [[ "$PRESET" == speech-progressive || "$PRESET" == speech ]]; then
    TARGET_MS=2000
  else
    TARGET_MS=700
  fi
fi

swiftc -parse-as-library -O scripts/benchmark_apple_speech_streaming.swift \
  -o "$OUTPUT" \
  -framework Foundation \
  -framework AVFAudio \
  -framework CoreMedia \
  -framework Speech

values=()
provisional_values=()
for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  report="$($OUTPUT "$AUDIO" "$CHUNK_MS" "$PRESET" "$EXPECTATIONS" --metrics-only 2>/dev/null)"
  value="$(jq -r '.firstUsefulMs // empty' <<<"$report")"
  if [[ -z "$value" ]]; then
    echo "iteration $iteration emitted no useful partial" >&2
    exit 1
  fi
  values+=("$value")
  provisional="$(jq -r '.firstProvisionalMs // empty' <<<"$report")"
  if [[ "$PRESET" == speech-progressive || "$PRESET" == speech ]]; then
    if [[ -z "$provisional" ]]; then
      echo "iteration $iteration emitted no provisional SpeechTranscriber result" >&2
      exit 1
    fi
    provisional_values+=("$provisional")
  fi
  jq -c --argjson iteration "$iteration" '{iteration: $iteration, firstUsefulMs, firstProvisionalMs, provisionalEventCount, finalEventCount, revisionCount, maxProvisionalCadenceMs, completionLagMs, punctuationInsensitiveWer, referenceWordCount, requiredTermsMissingCount, forbiddenTermsFoundCount}' <<<"$report"
done

sorted="$(printf '%s\n' "${values[@]}" | sort -n)"
p95_index=$(( (ITERATIONS * 95 + 99) / 100 ))
p95="$(sed -n "${p95_index}p" <<<"$sorted")"
gate_value="$p95"
if ((${#provisional_values[@]} > 0)); then
  provisional_sorted="$(printf '%s\n' "${provisional_values[@]}" | sort -n)"
  p95_provisional="$(sed -n "${p95_index}p" <<<"$provisional_sorted")"
  gate_value="$p95_provisional"
else
  p95_provisional="null"
fi
jq -n \
  --arg engine "$([[ "$PRESET" == speech* ]] && echo apple-speech-transcriber || echo apple-dictation-transcriber)" \
  --arg preset "$PRESET" \
  --argjson iterations "$ITERATIONS" \
  --argjson chunkMs "$CHUNK_MS" \
  --argjson p95FirstUsefulPartialMs "$p95" \
  --argjson p95FirstProvisionalMs "$p95_provisional" \
  --argjson targetMs "$TARGET_MS" \
  '{engine: $engine, preset: $preset, iterations: $iterations, chunkMs: $chunkMs, p95FirstUsefulPartialMs: $p95FirstUsefulPartialMs, p95FirstProvisionalMs: $p95FirstProvisionalMs, targetMs: $targetMs, passed: (($p95FirstProvisionalMs // $p95FirstUsefulPartialMs) < $targetMs)}'

if (( gate_value >= TARGET_MS )); then
  exit 1
fi
