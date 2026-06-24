//! Persistent streaming speaker-diarization sidecar.
//!
//! NVIDIA Sortformer (streaming, 4-speaker) via `parakeet-rs` + ONNX `ort`. No
//! Python at runtime. Runs as its own cargo workspace so the `ort rc.12 /
//! ndarray 0.17` it needs never collides with Minutes' `pyannote-rs` (`ort rc.10
//! / ndarray 0.16`); the process boundary dissolves the conflict.
//!
//! Protocol (v1):
//!   args:   --model <path.onnx> (required), --config callhome|dihard3 (default callhome)
//!   stdin:  repeated frames = u32 LE byte-length L, then L bytes of f32 LE PCM (16 kHz mono).
//!           A clean EOF before a frame ends the stream (flush + exit 0).
//!   stdout: NDJSON. First line {"event":"ready","latency_s":F,"chunk_len":N}.
//!           Then one {"start_ms":U,"end_ms":U,"speaker":N} per emitted segment
//!           (absolute offsets). On EOF {"event":"flush_done","segments":N}.
//!   stderr: human logs / fatal errors. Nonzero exit on fatal error.
//!
//! CONTRACT: the consumer MUST drain stdout concurrently with writing stdin
//! (separate threads). This process reads stdin and writes stdout on one thread
//! and flushes each line, so a consumer that only writes stdin without reading
//! stdout can deadlock once the stdout pipe fills. Segment output is low-volume
//! (a handful of lines per minute), so in practice the pipe never fills before a
//! reading consumer drains it; this is a correctness contract, not a throughput
//! concern. The Minutes live-path integration uses a dedicated reader thread.

use parakeet_rs::sortformer::{DiarizationConfig, Sortformer, SpeakerSegment};
use std::io::{self, Read, Write};
use std::process::ExitCode;

const SAMPLE_RATE: f64 = 16_000.0;
/// Reject absurd frame lengths so a corrupt header can't trigger a huge alloc.
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("minutes-diarize-sidecar: fatal: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (model_path, config) = parse_args()?;

    let mut sortformer = Sortformer::with_config(&model_path, None, config)
        .map_err(|e| format!("failed to load model {model_path}: {e}"))?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    emit(
        &mut out,
        &serde_json::json!({
            "event": "ready",
            "latency_s": sortformer.latency(),
            "chunk_len": sortformer.chunk_len,
        }),
    )?;

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut total_segments: u64 = 0;

    loop {
        match read_frame(&mut input)? {
            Frame::Eof => break,
            Frame::Pcm(samples) => {
                if samples.is_empty() {
                    continue;
                }
                let segs = sortformer
                    .feed(&samples)
                    .map_err(|e| format!("feed() failed: {e}"))?;
                for seg in &segs {
                    emit_segment(&mut out, seg)?;
                    total_segments += 1;
                }
            }
        }
    }

    for seg in &sortformer.flush().map_err(|e| format!("flush() failed: {e}"))? {
        emit_segment(&mut out, seg)?;
        total_segments += 1;
    }
    emit(
        &mut out,
        &serde_json::json!({"event": "flush_done", "segments": total_segments}),
    )?;
    Ok(())
}

fn parse_args() -> Result<(String, DiarizationConfig), Box<dyn std::error::Error>> {
    let mut model_path: Option<String> = None;
    let mut config_name = String::from("callhome");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model" => model_path = Some(args.next().ok_or("--model needs a value")?),
            "--config" => config_name = args.next().ok_or("--config needs a value")?,
            "-h" | "--help" => {
                eprintln!("usage: minutes-diarize-sidecar --model <path.onnx> [--config callhome|dihard3]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }
    let model_path = model_path.ok_or("--model <path.onnx> is required")?;
    let config = match config_name.as_str() {
        "callhome" => DiarizationConfig::callhome(),
        "dihard3" => DiarizationConfig::dihard3(),
        other => return Err(format!("unknown --config: {other} (use callhome|dihard3)").into()),
    };
    Ok((model_path, config))
}

enum Frame {
    Pcm(Vec<f32>),
    Eof,
}

/// Read one length-prefixed PCM frame. A clean EOF before the header => `Eof`.
fn read_frame<R: Read>(r: &mut R) -> Result<Frame, Box<dyn std::error::Error>> {
    let mut len_buf = [0u8; 4];
    if !read_exact_or_eof(r, &mut len_buf)? {
        return Ok(Frame::Eof);
    }
    let nbytes = u32::from_le_bytes(len_buf) as usize;
    if nbytes == 0 {
        return Ok(Frame::Pcm(Vec::new()));
    }
    if nbytes % 4 != 0 {
        return Err(format!("frame length {nbytes} is not a multiple of 4 (f32)").into());
    }
    if nbytes > MAX_FRAME_BYTES {
        return Err(format!("frame length {nbytes} exceeds {MAX_FRAME_BYTES} cap").into());
    }
    let mut buf = vec![0u8; nbytes];
    r.read_exact(&mut buf)
        .map_err(|e| format!("truncated frame body ({nbytes} bytes): {e}"))?;
    let samples = buf
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(Frame::Pcm(samples))
}

/// `read_exact`, but distinguishes a clean EOF (zero bytes read => `Ok(false)`)
/// from a truncated read mid-buffer (=> `Err`). Retries on `Interrupted`.
fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<bool, Box<dyn std::error::Error>> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => {
                if filled == 0 {
                    return Ok(false);
                }
                return Err("unexpected EOF inside length header".into());
            }
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Box::new(e)),
        }
    }
    Ok(true)
}

fn emit_segment<W: Write>(w: &mut W, seg: &SpeakerSegment) -> Result<(), Box<dyn std::error::Error>> {
    emit(
        w,
        &serde_json::json!({
            "start_ms": samples_to_ms(seg.start),
            "end_ms": samples_to_ms(seg.end),
            "speaker": seg.speaker_id,
        }),
    )
}

fn samples_to_ms(sample_offset: u64) -> u64 {
    ((sample_offset as f64 / SAMPLE_RATE) * 1000.0).round() as u64
}

fn emit<W: Write>(w: &mut W, v: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(w, "{v}")?;
    // Flush so the consumer (Minutes live path) sees segments promptly, not at exit.
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(samples: &[f32]) -> Vec<u8> {
        let mut v = Vec::new();
        let bytes = (samples.len() * 4) as u32;
        v.extend_from_slice(&bytes.to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    #[test]
    fn frame_round_trips() {
        let samples = vec![0.0f32, 0.5, -0.25, 1.0];
        let mut cur = Cursor::new(frame(&samples));
        match read_frame(&mut cur).unwrap() {
            Frame::Pcm(got) => assert_eq!(got, samples),
            Frame::Eof => panic!("expected a frame, got EOF"),
        }
    }

    #[test]
    fn clean_eof_before_header() {
        let mut cur = Cursor::new(Vec::<u8>::new());
        assert!(matches!(read_frame(&mut cur).unwrap(), Frame::Eof));
    }

    #[test]
    fn two_frames_then_eof() {
        let mut bytes = frame(&[1.0, 2.0]);
        bytes.extend(frame(&[3.0]));
        let mut cur = Cursor::new(bytes);
        assert!(matches!(read_frame(&mut cur).unwrap(), Frame::Pcm(ref v) if v.len() == 2));
        assert!(matches!(read_frame(&mut cur).unwrap(), Frame::Pcm(ref v) if v.len() == 1));
        assert!(matches!(read_frame(&mut cur).unwrap(), Frame::Eof));
    }

    #[test]
    fn truncated_header_is_error() {
        let mut cur = Cursor::new(vec![0x10u8, 0x00]); // 2 of 4 header bytes
        assert!(read_frame(&mut cur).is_err());
    }

    #[test]
    fn non_multiple_of_four_is_error() {
        let mut cur = Cursor::new(vec![0x03u8, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xCC]);
        assert!(read_frame(&mut cur).is_err());
    }

    #[test]
    fn samples_to_ms_is_correct() {
        assert_eq!(samples_to_ms(16_000), 1000);
        assert_eq!(samples_to_ms(8_000), 500);
        assert_eq!(samples_to_ms(0), 0);
    }
}
