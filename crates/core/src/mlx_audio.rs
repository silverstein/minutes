use crate::config::Config;
use crate::error::TranscribeError;
use crate::transcribe::{
    load_audio_samples, transcript_from_timed_segments, FilterStats, TimedTranscriptSegment,
    TranscribeResult,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const HELPER_CODE: &str = r#"
import json
import sys
import time
import traceback

from mlx_audio.stt.utils import load

models = {}

def segment_dict(segment):
    if hasattr(segment, "__dict__"):
        segment = segment.__dict__
    text = segment.get("text") or segment.get("Content") or segment.get("content") or ""
    start = segment.get("start", segment.get("Start"))
    end = segment.get("end", segment.get("End"))
    if start is None or end is None:
        return None
    return {"start_secs": float(start), "end_secs": float(end), "text": str(text)}

def collect_segments(result):
    if getattr(result, "segments", None):
        return [s for s in (segment_dict(segment) for segment in result.segments) if s]
    if getattr(result, "sentences", None):
        return [s for s in (segment_dict(sentence) for sentence in result.sentences) if s]
    return []

for line in sys.stdin:
    request_id = None
    started = time.time()
    try:
        req = json.loads(line)
        request_id = req.get("request_id")
        model_id = req["model"]
        model_warm = model_id in models
        cold_load_ms = 0
        if not model_warm:
            load_started = time.time()
            models[model_id] = load(model_id)
            cold_load_ms = int((time.time() - load_started) * 1000)
        kwargs = {}
        if req.get("language") is not None:
            kwargs["language"] = req.get("language")
        if req.get("context") is not None:
            kwargs["context"] = req.get("context")
        if req.get("chunk_duration_secs") is not None:
            kwargs["chunk_duration"] = float(req.get("chunk_duration_secs"))
        infer_started = time.time()
        result = models[model_id].generate(req["audio_path"], **kwargs)
        inference_ms = int((time.time() - infer_started) * 1000)
        segments = collect_segments(result)
        text = getattr(result, "text", "")
        response = {
            "request_id": request_id,
            "ok": True,
            "text": text,
            "segments": segments,
            "stats": {
                "model_warm": model_warm,
                "cold_load_ms": cold_load_ms,
                "inference_ms": inference_ms,
                "rtf": None,
            },
        }
    except Exception as exc:
        response = {
            "request_id": request_id,
            "ok": False,
            "error": f"{type(exc).__name__}: {exc}",
            "traceback": traceback.format_exc(),
        }
    sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
    sys.stdout.flush()
"#;

#[derive(Debug, Serialize)]
struct HelperRequest<'a> {
    request_id: String,
    audio_path: &'a str,
    model: &'a str,
    language: Option<&'a str>,
    context: Option<&'a str>,
    chunk_duration_secs: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct HelperResponse {
    request_id: Option<String>,
    ok: Option<bool>,
    error: Option<String>,
    text: Option<String>,
    segments: Option<Vec<HelperSegment>>,
    stats: Option<HelperStats>,
}

#[derive(Debug, Deserialize)]
struct HelperSegment {
    start_secs: f64,
    end_secs: f64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct HelperStats {
    model_warm: Option<bool>,
    cold_load_ms: Option<u64>,
    inference_ms: Option<u64>,
    rtf: Option<f64>,
}

#[derive(Default)]
struct MlxAudioHelperManager {
    running: Option<RunningHelper>,
    request_counter: u64,
}

struct RunningHelper {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<Result<String, std::io::Error>>,
    stdout_thread: Option<JoinHandle<()>>,
}

impl Drop for RunningHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(stdout_thread) = self.stdout_thread.take() {
            let _ = stdout_thread.join();
        }
    }
}

pub fn transcribe(audio_path: &Path, config: &Config) -> Result<TranscribeResult, TranscribeError> {
    let samples = load_audio_samples(audio_path)?;
    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }
    let stats = FilterStats {
        audio_duration_secs: samples.len() as f64 / 16_000.0,
        samples_after_silence_strip: samples.len(),
        ..FilterStats::default()
    };
    drop(samples);

    let started = Instant::now();
    let response = request_helper(audio_path, config)?;

    let result = response_to_transcribe_result(response, stats, config)?;
    tracing::info!(
        engine = "mlx-audio",
        elapsed_ms = started.elapsed().as_millis() as u64,
        words = result.stats.final_words,
        diagnosis = result.stats.diagnosis(),
        "mlx-audio transcription complete"
    );
    Ok(result)
}

fn request_helper(audio_path: &Path, config: &Config) -> Result<HelperResponse, TranscribeError> {
    if config.transcription.mlx_audio_warm
        && std::env::var_os("MINUTES_MLX_AUDIO_FORCE_ONESHOT").is_none()
    {
        global_manager()
            .lock()
            .map_err(|_| {
                TranscribeError::TranscriptionFailed("mlx-audio helper lock poisoned".into())
            })?
            .request(audio_path, config)
    } else {
        request_one_shot(audio_path, config)
    }
}

fn global_manager() -> &'static Mutex<MlxAudioHelperManager> {
    static MANAGER: OnceLock<Mutex<MlxAudioHelperManager>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(MlxAudioHelperManager::default()))
}

impl MlxAudioHelperManager {
    fn request(
        &mut self,
        audio_path: &Path,
        config: &Config,
    ) -> Result<HelperResponse, TranscribeError> {
        let request_id = self.next_request_id();
        match self.request_once(&request_id, audio_path, config) {
            Ok(response) => Ok(response),
            Err(first_error) => {
                self.stop();
                tracing::warn!(error = %first_error, "mlx-audio helper request failed; restarting once");
                self.request_once(&request_id, audio_path, config)
            }
        }
    }

    fn request_once(
        &mut self,
        request_id: &str,
        audio_path: &Path,
        config: &Config,
    ) -> Result<HelperResponse, TranscribeError> {
        if self.running.is_none() {
            self.running = Some(start_helper(config)?);
        }
        let running = self.running.as_mut().expect("helper just started");
        request_running_helper(running, request_id, audio_path, config)
    }

    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("minutes-mlx-{}", self.request_counter)
    }

    fn stop(&mut self) {
        self.running.take();
    }
}

fn request_one_shot(audio_path: &Path, config: &Config) -> Result<HelperResponse, TranscribeError> {
    let mut running = start_helper(config)?;
    request_running_helper(&mut running, "minutes-mlx-oneshot", audio_path, config)
}

fn request_running_helper(
    running: &mut RunningHelper,
    request_id: &str,
    audio_path: &Path,
    config: &Config,
) -> Result<HelperResponse, TranscribeError> {
    let audio_path = audio_path.to_str().ok_or_else(|| {
        TranscribeError::TranscriptionFailed("audio path is not valid UTF-8".into())
    })?;
    let request = HelperRequest {
        request_id: request_id.to_string(),
        audio_path,
        model: &config.transcription.mlx_audio_model,
        language: config.transcription.language.as_deref(),
        context: None,
        chunk_duration_secs: Some(config.transcription.mlx_audio_chunk_secs),
    };

    serde_json::to_writer(&mut running.stdin, &request)
        .map_err(|error| TranscribeError::TranscriptionFailed(error.to_string()))?;
    running.stdin.write_all(b"\n")?;
    running.stdin.flush()?;

    let line = read_helper_line(running, config)?;
    let response: HelperResponse = serde_json::from_str(&line).map_err(|error| {
        TranscribeError::TranscriptionFailed(format!(
            "invalid mlx-audio helper JSON: {error}; line={line}"
        ))
    })?;
    if response.request_id.as_deref() != Some(request_id) {
        return Err(TranscribeError::TranscriptionFailed(format!(
            "mlx-audio helper request_id mismatch: expected {request_id}, got {:?}",
            response.request_id
        )));
    }
    Ok(response)
}

fn read_helper_line(
    running: &mut RunningHelper,
    config: &Config,
) -> Result<String, TranscribeError> {
    if config.transcription.mlx_audio_timeout_secs == 0 {
        return read_helper_line_result(running.stdout_rx.recv().map_err(|_| {
            TranscribeError::TranscriptionFailed(
                "mlx-audio helper closed stdout before responding".into(),
            )
        })?);
    }

    match running.stdout_rx.recv_timeout(Duration::from_secs(
        config.transcription.mlx_audio_timeout_secs,
    )) {
        Ok(result) => read_helper_line_result(result),
        Err(RecvTimeoutError::Timeout) => Err(TranscribeError::TranscriptionFailed(format!(
            "mlx-audio helper timed out after {}s",
            config.transcription.mlx_audio_timeout_secs
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(TranscribeError::TranscriptionFailed(
            "mlx-audio helper closed stdout before responding".into(),
        )),
    }
}

fn read_helper_line_result(
    result: Result<String, std::io::Error>,
) -> Result<String, TranscribeError> {
    let line = result?;
    if line.is_empty() {
        return Err(TranscribeError::TranscriptionFailed(
            "mlx-audio helper closed stdout before responding".into(),
        ));
    }
    Ok(line)
}

fn start_helper(config: &Config) -> Result<RunningHelper, TranscribeError> {
    let mut command = helper_command(config);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            TranscribeError::TranscriptionFailed(format!(
                "could not spawn mlx-audio helper: {error}"
            ))
        })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        TranscribeError::TranscriptionFailed("could not open mlx-audio helper stdin".into())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        TranscribeError::TranscriptionFailed("could not open mlx-audio helper stdout".into())
    })?;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let stdout_thread = thread::spawn(move || read_stdout_lines(stdout, stdout_tx));
    Ok(RunningHelper {
        child,
        stdin,
        stdout_rx,
        stdout_thread: Some(stdout_thread),
    })
}

fn read_stdout_lines(stdout: ChildStdout, stdout_tx: mpsc::Sender<Result<String, std::io::Error>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if stdout_tx.send(Ok(line)).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = stdout_tx.send(Err(error));
                break;
            }
        }
    }
}

fn helper_command(config: &Config) -> Command {
    if let Ok(explicit) = std::env::var("MINUTES_MLX_AUDIO_HELPER") {
        return Command::new(explicit);
    }
    let mut command = Command::new(&config.transcription.mlx_audio_python);
    command.args(["-u", "-c", HELPER_CODE]);
    command
}

fn response_to_transcribe_result(
    response: HelperResponse,
    mut stats: FilterStats,
    config: &Config,
) -> Result<TranscribeResult, TranscribeError> {
    if response.ok == Some(false) {
        return Err(TranscribeError::TranscriptionFailed(format!(
            "mlx-audio helper failed: {}",
            response.error.unwrap_or_else(|| "unknown error".into())
        )));
    }

    let segments = response.segments.unwrap_or_default();
    if segments.is_empty() {
        let preview: String = response
            .text
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        return Err(TranscribeError::TranscriptionFailed(format!(
            "mlx-audio output did not include timed segments; text-only output is unsupported. First 200 chars: {preview}"
        )));
    }

    let timed_segments: Vec<TimedTranscriptSegment> = segments
        .into_iter()
        .map(|segment| TimedTranscriptSegment {
            start_secs: segment.start_secs,
            end_secs: segment.end_secs,
            text: segment.text,
        })
        .collect();
    let (text, cleanup_stats) = transcript_from_timed_segments(&timed_segments, config)?;

    stats.raw_segments = cleanup_stats.raw_segments;
    stats.after_no_speech_filter = cleanup_stats.raw_segments;
    stats.after_dedup = cleanup_stats.after_dedup;
    stats.after_interleaved = cleanup_stats.after_interleaved;
    stats.after_script_filter = cleanup_stats.after_script_filter;
    stats.after_noise_markers = cleanup_stats.after_noise_markers;
    stats.after_trailing_trim = cleanup_stats.after_trailing_trim;
    stats.final_words = text.split_whitespace().count();

    if let Some(helper_stats) = response.stats {
        tracing::debug!(
            model_warm = helper_stats.model_warm.unwrap_or(false),
            cold_load_ms = helper_stats.cold_load_ms.unwrap_or(0),
            inference_ms = helper_stats.inference_ms.unwrap_or(0),
            rtf = helper_stats.rtf.unwrap_or(-1.0),
            "mlx-audio helper stats"
        );
    }

    Ok(TranscribeResult { text, stats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::write_wav_16k_mono;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::MutexGuard;

    fn test_lock() -> MutexGuard<'static, ()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("mlx-audio test lock")
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.transcription.mlx_audio_timeout_secs = 5;
        config
    }

    fn config_with_helper(helper: &Path, warm: bool) -> Config {
        let mut config = config();
        config.transcription.engine = "mlx-audio".into();
        config.transcription.mlx_audio_python = helper.display().to_string();
        config.transcription.mlx_audio_warm = warm;
        config
    }

    fn write_audio(path: &Path) {
        let samples: Vec<f32> = (0..1600)
            .map(|i| 0.2 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        write_wav_16k_mono(path, &samples).expect("write test wav");
    }

    #[derive(Debug, Clone)]
    struct VttCue {
        start_secs: f64,
        end_secs: f64,
        text: String,
    }

    #[derive(Debug)]
    struct ErrorRate {
        edits: usize,
        reference_len: usize,
        rate: f64,
    }

    fn parse_vtt_cues(vtt: &str) -> Vec<VttCue> {
        let lines: Vec<&str> = vtt.lines().collect();
        let mut cues = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index].trim();
            if let Some((start, end)) = parse_vtt_timing(line) {
                index += 1;
                let mut cue_lines = Vec::new();
                while index < lines.len() && !lines[index].trim().is_empty() {
                    cue_lines.push(strip_vtt_speaker_label(lines[index].trim()));
                    index += 1;
                }
                let text = cue_lines
                    .into_iter()
                    .filter(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.trim().is_empty() {
                    cues.push(VttCue {
                        start_secs: start,
                        end_secs: end,
                        text,
                    });
                }
            } else {
                index += 1;
            }
        }
        cues
    }

    fn parse_vtt_timing(line: &str) -> Option<(f64, f64)> {
        let (start, rest) = line.split_once("-->")?;
        let end = rest.split_whitespace().next()?;
        Some((
            parse_vtt_timestamp(start.trim())?,
            parse_vtt_timestamp(end)?,
        ))
    }

    fn parse_vtt_timestamp(value: &str) -> Option<f64> {
        let mut parts: Vec<&str> = value.split(':').collect();
        if parts.len() == 2 {
            parts.insert(0, "0");
        }
        if parts.len() != 3 {
            return None;
        }
        let hours: f64 = parts[0].parse().ok()?;
        let minutes: f64 = parts[1].parse().ok()?;
        let seconds: f64 = parts[2].replace(',', ".").parse().ok()?;
        Some(hours * 3600.0 + minutes * 60.0 + seconds)
    }

    fn strip_vtt_speaker_label(line: &str) -> String {
        let without_tags = strip_vtt_tags(line);
        if let Some((speaker, text)) = without_tags.split_once(':') {
            let speaker = speaker.trim();
            if !speaker.is_empty()
                && speaker.len() <= 48
                && speaker
                    .chars()
                    .all(|c| c.is_alphabetic() || c.is_whitespace() || c == '-' || c == '.')
            {
                return text.trim().to_string();
            }
        }
        without_tags
    }

    fn strip_vtt_tags(line: &str) -> String {
        let mut output = String::new();
        let mut in_tag = false;
        for ch in line.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => output.push(ch),
                _ => {}
            }
        }
        output
    }

    fn reference_text_for_window(cues: &[VttCue], start_secs: f64, duration_secs: f64) -> String {
        let end_secs = start_secs + duration_secs;
        cues.iter()
            .filter(|cue| cue.end_secs > start_secs && cue.start_secs < end_secs)
            .map(|cue| cue.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn benchmark_audio_window(
        audio_path: &Path,
        start_secs: f64,
        duration_secs: Option<f64>,
    ) -> (tempfile::NamedTempFile, f64) {
        let samples = load_audio_samples(audio_path).expect("load benchmark audio");
        let start = ((start_secs.max(0.0) * 16_000.0).round() as usize).min(samples.len());
        let requested_end = duration_secs
            .map(|duration| start + (duration.max(0.0) * 16_000.0).round() as usize)
            .unwrap_or(samples.len());
        let end = requested_end.min(samples.len());
        assert!(end > start, "benchmark audio window must not be empty");
        let duration_secs = (end - start) as f64 / 16_000.0;
        let tmp_wav = tempfile::Builder::new()
            .prefix("minutes-mlx-audio-bench-")
            .suffix(".wav")
            .tempfile()
            .expect("create benchmark wav");
        write_wav_16k_mono(tmp_wav.path(), &samples[start..end]).expect("write benchmark wav");
        (tmp_wav, duration_secs)
    }

    fn words_for_error_rate(text: &str) -> Vec<String> {
        normalized_words(text)
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect()
    }

    fn normalized_words(text: &str) -> String {
        let text = strip_minutes_timestamp_markers(text);
        let mut output = String::new();
        let mut last_was_space = true;
        for ch in text.chars() {
            if ch.is_alphanumeric() || ch == '\'' {
                for lower in ch.to_lowercase() {
                    output.push(lower);
                }
                last_was_space = false;
            } else if !last_was_space {
                output.push(' ');
                last_was_space = true;
            }
        }
        output.trim().to_string()
    }

    fn strip_minutes_timestamp_markers(text: &str) -> String {
        text.lines()
            .map(|line| {
                let trimmed = line.trim_start();
                if let Some(rest) = strip_minutes_timestamp_prefix(trimmed) {
                    rest.trim_start()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn strip_minutes_timestamp_prefix(line: &str) -> Option<&str> {
        let rest = line.strip_prefix('[')?;
        let (timestamp, rest) = rest.split_once(']')?;
        if is_minutes_timestamp(timestamp) {
            Some(rest)
        } else {
            None
        }
    }

    fn is_minutes_timestamp(value: &str) -> bool {
        let parts: Vec<&str> = value.split(':').collect();
        (parts.len() == 2 || parts.len() == 3)
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    }

    fn chars_for_error_rate(text: &str) -> Vec<char> {
        normalized_words(text)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect()
    }

    fn error_rate<T: Eq>(reference: &[T], candidate: &[T]) -> ErrorRate {
        let edits = levenshtein(reference, candidate);
        ErrorRate {
            edits,
            reference_len: reference.len(),
            rate: if reference.is_empty() {
                0.0
            } else {
                edits as f64 / reference.len() as f64
            },
        }
    }

    fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
        if a.is_empty() {
            return b.len();
        }
        if b.is_empty() {
            return a.len();
        }

        let mut previous: Vec<usize> = (0..=b.len()).collect();
        let mut current = vec![0; b.len() + 1];
        for (i, a_item) in a.iter().enumerate() {
            current[0] = i + 1;
            for (j, b_item) in b.iter().enumerate() {
                let substitution_cost = usize::from(a_item != b_item);
                current[j + 1] = (previous[j + 1] + 1)
                    .min(current[j] + 1)
                    .min(previous[j] + substitution_cost);
            }
            std::mem::swap(&mut previous, &mut current);
        }
        previous[b.len()]
    }

    #[cfg(unix)]
    fn write_fake_helper(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("write fake helper");
        let mut permissions = fs::metadata(&path)
            .expect("fake helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod fake helper");
        path
    }

    fn stop_global_helper() {
        if let Ok(mut manager) = global_manager().lock() {
            manager.stop();
        }
    }

    #[test]
    fn response_with_timed_segments_formats_transcript() {
        let response = HelperResponse {
            request_id: Some("r1".into()),
            ok: Some(true),
            error: None,
            text: Some("hello world next line".into()),
            segments: Some(vec![
                HelperSegment {
                    start_secs: 0.0,
                    end_secs: 1.0,
                    text: "hello world".into(),
                },
                HelperSegment {
                    start_secs: 61.2,
                    end_secs: 63.0,
                    text: "next line".into(),
                },
            ]),
            stats: None,
        };
        let result =
            response_to_transcribe_result(response, FilterStats::default(), &config()).unwrap();
        assert_eq!(result.text, "[0:00] hello world\n[1:01] next line\n");
        assert_eq!(result.stats.raw_segments, 2);
    }

    #[test]
    fn response_with_text_only_is_hard_error() {
        let response = HelperResponse {
            request_id: Some("r1".into()),
            ok: Some(true),
            error: None,
            text: Some("plain text without timestamps".into()),
            segments: Some(Vec::new()),
            stats: None,
        };
        let err = response_to_transcribe_result(response, FilterStats::default(), &config())
            .expect_err("text-only output must be rejected");
        assert!(err.to_string().contains("text-only output is unsupported"));
    }

    #[test]
    fn response_with_invalid_timestamps_is_hard_error() {
        let response = HelperResponse {
            request_id: Some("r1".into()),
            ok: Some(true),
            error: None,
            text: None,
            segments: Some(vec![HelperSegment {
                start_secs: 2.0,
                end_secs: 1.0,
                text: "backwards".into(),
            }]),
            stats: None,
        };
        let err = response_to_transcribe_result(response, FilterStats::default(), &config())
            .expect_err("invalid timestamps must be rejected");
        assert!(err.to_string().contains("invalid bounds"));
    }

    #[test]
    #[cfg(unix)]
    fn transcribe_with_fake_helper_uses_timed_segments() {
        let _guard = test_lock();
        stop_global_helper();
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join("audio.wav");
        write_audio(&audio_path);
        let helper = write_fake_helper(
            dir.path(),
            "helper_success.py",
            r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    req = json.loads(line)
    if req.get("chunk_duration_secs") != 30.0:
        sys.stdout.write(json.dumps({
            "request_id": req["request_id"],
            "ok": False,
            "error": "missing chunk_duration_secs"
        }) + "\n")
        sys.stdout.flush()
        continue
    sys.stdout.write(json.dumps({
        "request_id": req["request_id"],
        "ok": True,
        "text": "hello from mlx audio",
        "segments": [
            {"start_secs": 0.0, "end_secs": 0.7, "text": "hello from mlx"},
            {"start_secs": 61.0, "end_secs": 62.0, "text": "audio"}
        ],
        "stats": {"model_warm": False, "cold_load_ms": 1, "inference_ms": 2, "rtf": 0.2}
    }) + "\n")
    sys.stdout.flush()
"#,
        );
        let config = config_with_helper(&helper, false);

        let result = transcribe(&audio_path, &config).unwrap();

        assert_eq!(result.text, "[0:00] hello from mlx\n[1:01] audio\n");
        assert_eq!(result.stats.raw_segments, 2);
        assert_eq!(result.stats.after_trailing_trim, 2);
        assert_eq!(result.stats.samples_after_silence_strip, 1600);
        stop_global_helper();
    }

    #[test]
    #[cfg(unix)]
    fn helper_failure_response_errors() {
        let _guard = test_lock();
        stop_global_helper();
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join("audio.wav");
        write_audio(&audio_path);
        let helper = write_fake_helper(
            dir.path(),
            "helper_failure.py",
            r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    req = json.loads(line)
    sys.stdout.write(json.dumps({
        "request_id": req["request_id"],
        "ok": False,
        "error": "model cache missing"
    }) + "\n")
    sys.stdout.flush()
"#,
        );
        let config = config_with_helper(&helper, false);

        let err = transcribe(&audio_path, &config).expect_err("helper failure must error");

        assert!(err.to_string().contains("mlx-audio helper failed"));
        assert!(err.to_string().contains("model cache missing"));
        stop_global_helper();
    }

    #[test]
    #[cfg(unix)]
    fn helper_request_id_mismatch_errors() {
        let _guard = test_lock();
        stop_global_helper();
        let dir = tempfile::tempdir().unwrap();
        let helper = write_fake_helper(
            dir.path(),
            "helper_mismatch.py",
            r#"#!/usr/bin/env python3
import json
import sys

for _line in sys.stdin:
    sys.stdout.write(json.dumps({
        "request_id": "wrong-request",
        "ok": True,
        "segments": [{"start_secs": 0.0, "end_secs": 0.5, "text": "hello"}]
    }) + "\n")
    sys.stdout.flush()
"#,
        );
        let config = config_with_helper(&helper, false);

        let err =
            request_one_shot(&dir.path().join("audio.wav"), &config).expect_err("mismatch errors");

        assert!(err.to_string().contains("request_id mismatch"));
        stop_global_helper();
    }

    #[test]
    #[cfg(unix)]
    fn warm_helper_restarts_once_after_closed_stdout() {
        let _guard = test_lock();
        stop_global_helper();
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join("audio.wav");
        write_audio(&audio_path);
        let state_path = dir.path().join("attempts.txt");
        let helper_body = format!(
            r#"#!/usr/bin/env python3
import json
import pathlib
import sys

state_path = pathlib.Path({state_path:?})
attempts = int(state_path.read_text()) if state_path.exists() else 0
line = sys.stdin.readline()
state_path.write_text(str(attempts + 1))
if attempts == 0:
    sys.exit(0)
req = json.loads(line)
sys.stdout.write(json.dumps({{
    "request_id": req["request_id"],
    "ok": True,
    "segments": [{{"start_secs": 0.0, "end_secs": 0.5, "text": "after restart"}}]
}}) + "\n")
sys.stdout.flush()
"#,
            state_path = state_path.display().to_string()
        );
        let helper = write_fake_helper(dir.path(), "helper_restart.py", &helper_body);
        let config = config_with_helper(&helper, true);

        let result = transcribe(&audio_path, &config).unwrap();

        assert_eq!(result.text, "[0:00] after restart\n");
        assert_eq!(fs::read_to_string(&state_path).unwrap(), "2");
        stop_global_helper();
    }

    #[test]
    #[cfg(unix)]
    fn helper_request_times_out() {
        let _guard = test_lock();
        stop_global_helper();
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join("audio.wav");
        write_audio(&audio_path);
        let helper = write_fake_helper(
            dir.path(),
            "helper_timeout.py",
            r#"#!/usr/bin/env python3
import sys
import time

for _line in sys.stdin:
    time.sleep(5)
"#,
        );
        let mut config = config_with_helper(&helper, false);
        config.transcription.mlx_audio_timeout_secs = 1;

        let err = transcribe(&audio_path, &config).expect_err("slow helper must time out");

        assert!(err.to_string().contains("timed out after 1s"));
        stop_global_helper();
    }

    #[test]
    fn vtt_reference_text_strips_cue_numbers_timestamps_and_speakers() {
        let vtt = r#"WEBVTT

1
00:00:02.750 --> 00:00:08.729
Speaker One: Okay, yeah, let me just repeat myself.

2
00:00:09.160 --> 00:00:27.820
Speaker Two: Is there any metadata?

3
00:00:27.820 --> 00:00:31.459
Outside Window: this should not be included
"#;

        let cues = parse_vtt_cues(vtt);
        let text = reference_text_for_window(&cues, 0.0, 20.0);

        assert_eq!(
            text,
            "Okay, yeah, let me just repeat myself. Is there any metadata?"
        );
    }

    #[test]
    fn error_rate_normalizes_words_and_counts_edits() {
        let reference = words_for_error_rate("Hello, ASR world!");
        let candidate = words_for_error_rate("hello world");

        let wer = error_rate(&reference, &candidate);

        assert_eq!(words_for_error_rate("[0:00] hello"), vec!["hello"]);
        assert_eq!(reference, vec!["hello", "asr", "world"]);
        assert_eq!(candidate, vec!["hello", "world"]);
        assert_eq!(wer.edits, 1);
        assert_eq!(wer.reference_len, 3);
        assert!((wer.rate - (1.0 / 3.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn mlx_audio_reference_benchmark_when_env_is_set() {
        let Some(audio_path) = std::env::var_os("MINUTES_MLX_AUDIO_BENCH_AUDIO") else {
            return;
        };
        let Some(reference_vtt_path) = std::env::var_os("MINUTES_MLX_AUDIO_BENCH_REFERENCE_VTT")
        else {
            return;
        };
        let _guard = test_lock();
        stop_global_helper();
        let mut config = Config::default();
        config.transcription.engine = "mlx-audio".into();
        config.transcription.mlx_audio_python = std::env::var("MINUTES_MLX_AUDIO_BENCH_PYTHON")
            .or_else(|_| std::env::var("MINUTES_MLX_AUDIO_E2E_PYTHON"))
            .unwrap_or_else(|_| config.transcription.mlx_audio_python.clone());
        config.transcription.mlx_audio_model = std::env::var("MINUTES_MLX_AUDIO_BENCH_MODEL")
            .or_else(|_| std::env::var("MINUTES_MLX_AUDIO_E2E_MODEL"))
            .unwrap_or_else(|_| config.transcription.mlx_audio_model.clone());
        config.transcription.mlx_audio_chunk_secs =
            std::env::var("MINUTES_MLX_AUDIO_BENCH_CHUNK_SECS")
                .or_else(|_| std::env::var("MINUTES_MLX_AUDIO_E2E_CHUNK_SECS"))
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(10.0);
        config.transcription.mlx_audio_timeout_secs =
            std::env::var("MINUTES_MLX_AUDIO_BENCH_TIMEOUT_SECS")
                .or_else(|_| std::env::var("MINUTES_MLX_AUDIO_E2E_TIMEOUT_SECS"))
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1800);

        let start_secs = std::env::var("MINUTES_MLX_AUDIO_BENCH_START_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0);
        let requested_duration_secs = std::env::var("MINUTES_MLX_AUDIO_BENCH_DURATION_SECS")
            .ok()
            .and_then(|value| value.parse().ok());
        let (bench_wav, duration_secs) =
            benchmark_audio_window(Path::new(&audio_path), start_secs, requested_duration_secs);
        let reference_vtt =
            fs::read_to_string(reference_vtt_path).expect("read benchmark reference VTT");
        let reference_text =
            reference_text_for_window(&parse_vtt_cues(&reference_vtt), start_secs, duration_secs);
        assert!(
            !reference_text.trim().is_empty(),
            "benchmark reference VTT window must include transcript text"
        );

        if let Some(warmup_secs) = std::env::var("MINUTES_MLX_AUDIO_BENCH_WARMUP_SECS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
        {
            let (warmup_wav, _) =
                benchmark_audio_window(Path::new(&audio_path), start_secs, Some(warmup_secs));
            let _ = transcribe(warmup_wav.path(), &config).expect("warm up mlx-audio model");
        }

        let started = Instant::now();
        let result = transcribe(bench_wav.path(), &config).expect("benchmark transcription");
        let elapsed_secs = started.elapsed().as_secs_f64();
        let rtf = elapsed_secs / duration_secs;

        let reference_words = words_for_error_rate(&reference_text);
        let candidate_words = words_for_error_rate(&result.text);
        let wer = error_rate(&reference_words, &candidate_words);
        let reference_chars = chars_for_error_rate(&reference_text);
        let candidate_chars = chars_for_error_rate(&result.text);
        let cer = error_rate(&reference_chars, &candidate_chars);

        eprintln!(
            "mlx-audio reference benchmark: model={} duration_secs={:.2} elapsed_secs={:.2} rtf={:.3} wer={:.3} wer_edits={}/{} cer={:.3} cer_edits={}/{} candidate_words={} reference_words={}",
            config.transcription.mlx_audio_model,
            duration_secs,
            elapsed_secs,
            rtf,
            wer.rate,
            wer.edits,
            wer.reference_len,
            cer.rate,
            cer.edits,
            cer.reference_len,
            candidate_words.len(),
            reference_words.len()
        );

        assert!(
            !candidate_words.is_empty(),
            "benchmark candidate transcript must not be empty"
        );
        if let Some(max_wer) = std::env::var("MINUTES_MLX_AUDIO_BENCH_MAX_WER")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
        {
            assert!(
                wer.rate <= max_wer,
                "WER {:.3} exceeded threshold {:.3}",
                wer.rate,
                max_wer
            );
        }
        if let Some(max_cer) = std::env::var("MINUTES_MLX_AUDIO_BENCH_MAX_CER")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
        {
            assert!(
                cer.rate <= max_cer,
                "CER {:.3} exceeded threshold {:.3}",
                cer.rate,
                max_cer
            );
        }
        if let Some(max_rtf) = std::env::var("MINUTES_MLX_AUDIO_BENCH_MAX_RTF")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
        {
            assert!(
                rtf <= max_rtf,
                "RTF {:.3} exceeded threshold {:.3}",
                rtf,
                max_rtf
            );
        }
        stop_global_helper();
    }

    #[test]
    fn mlx_audio_real_model_e2e_when_env_is_set() {
        let Some(audio_path) = std::env::var_os("MINUTES_MLX_AUDIO_E2E_AUDIO") else {
            return;
        };
        let _guard = test_lock();
        stop_global_helper();
        let mut config = Config::default();
        config.transcription.engine = "mlx-audio".into();
        config.transcription.mlx_audio_python = std::env::var("MINUTES_MLX_AUDIO_E2E_PYTHON")
            .unwrap_or_else(|_| config.transcription.mlx_audio_python.clone());
        config.transcription.mlx_audio_model = std::env::var("MINUTES_MLX_AUDIO_E2E_MODEL")
            .unwrap_or_else(|_| config.transcription.mlx_audio_model.clone());
        config.transcription.mlx_audio_chunk_secs =
            std::env::var("MINUTES_MLX_AUDIO_E2E_CHUNK_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(10.0);
        config.transcription.mlx_audio_timeout_secs =
            std::env::var("MINUTES_MLX_AUDIO_E2E_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1800);

        let result = transcribe(Path::new(&audio_path), &config).unwrap();

        assert!(result.text.contains("[0:00]"));
        assert!(
            result.stats.raw_segments > 0,
            "real MLX model must return timed segments"
        );

        stop_global_helper();
    }
}
