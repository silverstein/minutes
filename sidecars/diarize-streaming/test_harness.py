#!/usr/bin/env python3
"""End-to-end harness for minutes-diarize-sidecar.

Pipes a 16 kHz mono WAV through the sidecar over real stdin/stdout using the
length-prefixed PCM frame protocol, parses the NDJSON output, and asserts the
acceptance criteria (ready event, >=1 segment, expected speaker count). Reports
end-to-end latency and RTF.

Usage: test_harness.py <sidecar_bin> <model.onnx> <audio_16k_mono.wav>
"""
import json
import struct
import subprocess
import sys
import threading
import time
import wave


def main():
    if len(sys.argv) < 4:
        print("usage: test_harness.py <sidecar_bin> <model.onnx> <audio.wav> [expected_speakers] [min_segments]", file=sys.stderr)
        return 2
    sidecar, model, audio = sys.argv[1], sys.argv[2], sys.argv[3]
    expected_speakers = int(sys.argv[4]) if len(sys.argv) > 4 else 4
    min_segments = int(sys.argv[5]) if len(sys.argv) > 5 else 4

    with wave.open(audio, "rb") as w:
        assert w.getframerate() == 16000, f"need 16kHz, got {w.getframerate()}"
        assert w.getsampwidth() == 2, "expect 16-bit PCM"
        ch = w.getnchannels()
        raw = w.readframes(w.getnframes())
    ints = struct.unpack("<%dh" % (len(raw) // 2), raw)
    if ch > 1:  # downmix
        ints = [sum(ints[i:i + ch]) // ch for i in range(0, len(ints), ch)]
    samples = [x / 32768.0 for x in ints]
    duration = len(samples) / 16000.0
    print(f"audio: {duration:.1f}s, {len(samples)} samples")

    proc = subprocess.Popen([sidecar, "--model", model],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE)

    # Feed framed PCM from a writer thread so a full stdout pipe can never deadlock us.
    def feed():
        chunk = 320  # 20ms @ 16kHz
        try:
            for i in range(0, len(samples), chunk):
                blk = samples[i:i + chunk]
                payload = struct.pack("<%df" % len(blk), *blk)
                proc.stdin.write(struct.pack("<I", len(payload)))
                proc.stdin.write(payload)
            proc.stdin.close()  # clean EOF -> sidecar flushes
        except BrokenPipeError:
            pass

    # Drain stderr concurrently so a chatty child can't block on a full stderr pipe.
    err_chunks = []
    err_thread = threading.Thread(target=lambda: err_chunks.append(proc.stderr.read()), daemon=True)
    err_thread.start()

    t0 = time.time()
    threading.Thread(target=feed, daemon=True).start()

    ready = None
    flush_done = None
    segments = []
    for line in proc.stdout:
        obj = json.loads(line)
        if obj.get("event") == "ready":
            ready = obj
        elif obj.get("event") == "flush_done":
            flush_done = obj
        else:
            segments.append(obj)
    rc = proc.wait()
    err_thread.join(timeout=2)
    elapsed = time.time() - t0
    err = (b"".join(c for c in err_chunks if c)).decode(errors="replace").strip()

    print(f"exit={rc} | ready={ready} | flush_done={flush_done}")
    print(f"segments={len(segments)} | speakers={sorted({s['speaker'] for s in segments})}")
    for s in segments[:12]:
        print(f"  [{s['start_ms']/1000:6.2f}s-{s['end_ms']/1000:6.2f}s] Speaker {s['speaker']}")
    print(f"end-to-end {elapsed:.2f}s for {duration:.1f}s audio => RTF {elapsed/duration:.3f}")
    if err:
        print("stderr:\n" + err)

    distinct = len({s["speaker"] for s in segments})
    checks = {
        "exit_0": rc == 0,
        "ready_event": ready is not None,
        f"min_segments>={min_segments}": len(segments) >= min_segments,
        f"speakers=={expected_speakers}": distinct == expected_speakers,
        "flush_count_matches": flush_done is not None and flush_done.get("segments") == len(segments),
    }
    for name, passed in checks.items():
        print(f"  {'ok ' if passed else 'FAIL'} {name}")
    ok = all(checks.values())
    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
