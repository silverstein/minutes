#!/bin/bash
# Prove Apple Speech actually transcribes on this machine.
#
# Hosted macOS CI runners are VMs that report Apple Intelligence
# "deviceNotCapable", and on those the macOS 26 Speech symbols are absent, so
# the signed acceptance run can prove the byte transport but never the
# analyzer. This script closes that gap on real hardware.
#
# It drives the exact `@_cdecl("minutes_apple_speech_transcribe_pcm")` entry the
# signed XPC worker calls, using the bundled demo audio, with no Rust, no XPC
# and no signing in the way, so a failure here is the Speech capability itself
# rather than the transport around it.
#
# Usage: ./scripts/verify_apple_speech_hardware.sh [mode] [locale]
set -euo pipefail

MODE="${1:-dictation}"
LOCALE="${2:-en-US}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRIDGE="$REPO_ROOT/crates/core/src/macos_apple_speech_bridge.swift"
AUDIO="$REPO_ROOT/crates/assets/demo.wav"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Apple Speech hardware verification requires macOS" >&2
  exit 2
fi
os_major="$(sw_vers -productVersion | cut -d. -f1)"
if (( os_major < 26 )); then
  echo "Apple Speech requires macOS 26 or newer; this machine is $(sw_vers -productVersion)" >&2
  exit 2
fi
test -f "$BRIDGE"
test -f "$AUDIO"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# audioop was removed in Python 3.13, so downmix and resample directly.
python3 - "$AUDIO" "$WORK/audio.f32" <<'PY'
import pathlib
import struct
import sys
import wave

with wave.open(sys.argv[1]) as source:
    channels, width, rate = source.getnchannels(), source.getsampwidth(), source.getframerate()
    raw = source.readframes(source.getnframes())
if width != 2:
    raise SystemExit(f"expected 16-bit PCM demo audio, found {width * 8}-bit")

values = struct.unpack(f"<{len(raw) // 2}h", raw)
if channels > 1:
    frames = [
        sum(values[index : index + channels]) / channels
        for index in range(0, len(values) - channels + 1, channels)
    ]
else:
    frames = list(values)

if rate != 16000:
    ratio = rate / 16000
    resampled = []
    position = 0.0
    while position < len(frames) - 1:
        low = int(position)
        weight = position - low
        resampled.append(frames[low] * (1 - weight) + frames[low + 1] * weight)
        position += ratio
    frames = resampled

samples = [max(-1.0, min(1.0, frame / 32768.0)) for frame in frames]
pathlib.Path(sys.argv[2]).write_bytes(struct.pack(f"<{len(samples)}f", *samples))
print(f"prepared {len(samples)} samples ({len(samples) / 16000:.2f}s of 16 kHz mono audio)")
PY

cat >"$WORK/driver.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
extern unsigned char *minutes_apple_speech_transcribe_pcm(
    const float *samples, long sample_count, const char *mode,
    const char *locale, int ensure_assets, long *response_length);
extern void minutes_apple_speech_free_response(unsigned char *response,
                                               long response_length);
int main(int argc, char **argv) {
  FILE *source = fopen(argv[1], "rb");
  if (!source) { perror("open"); return 1; }
  fseek(source, 0, SEEK_END);
  long bytes = ftell(source);
  fseek(source, 0, SEEK_SET);
  float *samples = malloc(bytes);
  if (!samples || fread(samples, 1, bytes, source) != (size_t)bytes) { perror("read"); return 1; }
  fclose(source);
  long length = 0;
  unsigned char *response = minutes_apple_speech_transcribe_pcm(
      samples, bytes / 4, argv[2], argv[3], 1, &length);
  if (!response) { fprintf(stderr, "bridge returned no response\n"); return 1; }
  fwrite(response, 1, length, stdout);
  printf("\n");
  minutes_apple_speech_free_response(response, length);
  free(samples);
  return 0;
}
EOF

ARCH="$(uname -m)"
TOOLCHAIN="$(dirname "$(dirname "$(xcrun --find swiftc)")")"
swiftc -parse-as-library -O -target "${ARCH}-apple-macos11.0" -emit-library -static \
  -o "$WORK/libbridge.a" "$BRIDGE"
clang "$WORK/driver.c" "$WORK/libbridge.a" -o "$WORK/verify" \
  -L/usr/lib/swift -L"$TOOLCHAIN/lib/swift/macosx" -lswiftCore \
  -framework Foundation -framework Security -framework AVFAudio \
  -framework CoreMedia -framework Speech

echo "running the real bridge: mode=$MODE locale=$LOCALE"
RESPONSE="$("$WORK/verify" "$WORK/audio.f32" "$MODE" "$LOCALE")"

python3 - "$RESPONSE" <<'PY'
import json
import sys

report = json.loads(sys.argv[1])
transcript = report.get("transcript", "")
print(json.dumps(report, indent=2, sort_keys=True))
if not report.get("runtimeSupported"):
    raise SystemExit(
        "FAIL: this machine reports the Speech runtime unsupported; "
        f"error={report.get('error')!r}"
    )
if not transcript.strip():
    raise SystemExit("FAIL: the Speech runtime is supported but produced no transcript")
print(f"\nPASS: transcribed {report.get('wordCount')} words in {report.get('totalElapsedMs')}ms")
PY
