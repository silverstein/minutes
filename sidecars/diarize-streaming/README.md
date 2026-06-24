# minutes-diarize-sidecar

Persistent **streaming speaker-diarization** sidecar for Minutes' live path.
NVIDIA Sortformer (streaming, 4-speaker) via [`parakeet-rs`](https://crates.io/crates/parakeet-rs) + ONNX `ort`. No Python at runtime.

## Why a separate process / workspace

`parakeet-rs` needs `ort 2.0.0-rc.12` (ndarray 0.17); Minutes' `pyannote-rs` needs
`ort 2.0.0-rc.10` (ndarray 0.16). They cannot coexist in one binary (the `ort-sys`
`links = "onnxruntime"` manifest + the ndarray major bump). Tracked upstream as
`thewh1teagle/pyannote-rs#27`. Until that lands, this diarizer lives in its **own
cargo workspace** and is reached only across the process boundary. The Minutes
side (`crates/core/src/streaming_diarize.rs`, feature `streaming-diarize`) is pure
subprocess IPC + JSON and pulls none of these deps, so there is no conflict.

## Build

```bash
cargo build --release          # from this directory (its own workspace)
```

## Models (manual download, like whisper models)

```bash
# Sortformer v2.1 (4-speaker streaming), ~492MB fp32:
curl -L -o sortformer.onnx \
  https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.1.onnx
```

Note: this export's latency is ~10s (`chunk_len`/`right_context` are baked into the
ONNX metadata, not runtime-tunable). Sub-second latency needs a custom export with
smaller streaming params. See the strategy doc for the latency analysis.

## Protocol (v1)

- **args:** `--model <path.onnx>` (required), `--config callhome|dihard3` (default `callhome`).
- **stdin:** repeated frames = `u32` LE byte-length `L`, then `L` bytes of `f32` LE PCM (16 kHz mono). A clean EOF flushes and exits 0.
- **stdout:** NDJSON. First line `{"event":"ready","latency_s":F,"chunk_len":N}`; then one `{"start_ms":U,"end_ms":U,"speaker":N}` per segment (absolute offsets); on EOF `{"event":"flush_done","segments":N}`.
- **stderr:** logs / fatal errors; nonzero exit on fatal error.
- **Contract:** the consumer MUST drain stdout concurrently with writing stdin (separate threads), or a full stdout pipe can deadlock the single-threaded sidecar. Output is low-volume, so this is a correctness contract, not throughput.

## Test

```bash
cargo test --release                         # frame-codec unit tests
python3 test_harness.py target/release/minutes-diarize-sidecar sortformer.onnx <16k_mono.wav>
# asserts: ready event, >= min_segments, == expected_speakers (default 4), flush count matches
```

Limits: 4-speaker ceiling (NUM_SPEAKERS=4); ~10s latency on the standard export.
