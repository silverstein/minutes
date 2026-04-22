#!/usr/bin/env python3
"""
Minutes Video Review

Build a durable artifact bundle from a local video file or hosted video URL:
- transcript via hosted captions or Minutes-first transcription
- sampled key frames with adaptive caps
- metadata and a lightweight analysis summary
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path
from typing import Any


SUPPORTED_VIDEO_EXTENSIONS = {
    ".mp4",
    ".mov",
    ".mkv",
    ".webm",
    ".m4v",
}


def now_utc() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def slugify(value: str) -> str:
    lowered = value.lower()
    lowered = re.sub(r"[^a-z0-9]+", "-", lowered)
    lowered = lowered.strip("-")
    return lowered or "video-review"


def load_env_file(env_file: Path) -> None:
    if not env_file.exists():
        raise RuntimeError(f"Env file not found: {env_file}")

    for raw_line in env_file.read_text(encoding="utf-8", errors="ignore").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :].strip()
        if "=" not in line:
            continue

        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            continue

        if (value.startswith("'") and value.endswith("'")) or (
            value.startswith('"') and value.endswith('"')
        ):
            value = value[1:-1]

        os.environ[key] = value


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    check: bool = True,
    text: bool = True,
    env: dict[str, str] | None = None,
    timeout: int | None = None,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            args,
            cwd=str(cwd) if cwd else None,
            check=check,
            capture_output=True,
            text=text,
            env=env,
            timeout=timeout,
        )
    except FileNotFoundError as exc:
        cmd = args[0] if args else "<unknown>"
        raise RuntimeError(f"Required command not found: {cmd}") from exc
    except subprocess.CalledProcessError as exc:
        stderr = (exc.stderr or "").strip()
        stdout = (exc.stdout or "").strip()
        message = stderr if stderr else stdout
        raise RuntimeError(f"Command failed: {' '.join(args)}\n{message}") from exc


def kill_process_tree(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return

    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except PermissionError:
        process.terminate()
    time.sleep(0.5)

    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            return
        except PermissionError:
            process.kill()


def extract_last_json_object(text: str) -> dict[str, Any] | None:
    lines = text.splitlines()
    for idx in range(len(lines) - 1, -1, -1):
        if lines[idx].lstrip().startswith("{"):
            candidate = "\n".join(lines[idx:]).strip()
            try:
                return json.loads(candidate)
            except json.JSONDecodeError:
                continue
    return None


def extract_whisper_stdout_transcript(stdout_output: str) -> str | None:
    cleaned_lines: list[str] = []
    timestamp_pattern = re.compile(
        r"^\[[0-9:.]+\s+-->\s+[0-9:.]+\]\s*(?P<text>.+?)\s*$"
    )

    for raw_line in stdout_output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        match = timestamp_pattern.match(line)
        if match:
            cleaned_lines.append(match.group("text").strip())

    if not cleaned_lines:
        return None

    return "\n".join(cleaned_lines).strip()


def is_url(value: str) -> bool:
    return value.startswith("http://") or value.startswith("https://")


def detect_source_kind(value: str) -> str:
    if not is_url(value):
        return "local-file"

    lowered = value.lower()
    if "loom.com/" in lowered:
        return "loom"
    if "screenpal.com/" in lowered or "screencast-o-matic.com/" in lowered:
        return "screenpal"
    return "hosted-url"


def ensure_dependencies(source_is_url: bool) -> None:
    required = {"ffmpeg": "-version"}
    if source_is_url:
        required["yt-dlp"] = "--version"

    for cmd, version_flag in required.items():
        run([cmd, version_flag], check=True)


def pick_video_file(directory: Path) -> Path:
    candidates: list[Path] = []
    for path in directory.glob("source.*"):
        if path.suffix.lower() in SUPPORTED_VIDEO_EXTENSIONS:
            candidates.append(path)

    if not candidates:
        raise RuntimeError("Could not find downloaded video file in workspace")

    return max(candidates, key=lambda p: p.stat().st_size)


def pick_vtt_file(directory: Path) -> Path | None:
    vtts = sorted(directory.glob("source*.vtt"))
    if not vtts:
        return None

    preferred_patterns = [
        re.compile(r"\.en(?:[-_][A-Z]{2})?\.vtt$", re.IGNORECASE),
        re.compile(r"\.en\.vtt$", re.IGNORECASE),
    ]

    for pattern in preferred_patterns:
        for candidate in vtts:
            if pattern.search(candidate.name):
                return candidate

    return vtts[0]


def fetch_hosted_video_metadata(
    url: str,
    cookies_from_browser: str | None,
) -> dict[str, Any]:
    args = [
        "yt-dlp",
        "--no-warnings",
        "--dump-single-json",
        "--no-playlist",
        url,
    ]
    if cookies_from_browser:
        args.extend(["--cookies-from-browser", cookies_from_browser])

    result = run(args)
    raw = (result.stdout or "").strip()
    if not raw:
        return {}
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return {}

    return {
        "title": data.get("title"),
        "description": data.get("description"),
        "uploader": data.get("uploader"),
        "duration_seconds": data.get("duration"),
        "webpage_url": data.get("webpage_url") or url,
    }


def download_hosted_video(
    url: str,
    workspace: Path,
    cookies_from_browser: str | None,
) -> tuple[Path, Path | None, dict[str, Any]]:
    metadata = fetch_hosted_video_metadata(url, cookies_from_browser)
    output_template = workspace / "source.%(ext)s"
    args = [
        "yt-dlp",
        "--no-playlist",
        "--no-progress",
        "--no-warnings",
        "--force-overwrites",
        "--write-auto-subs",
        "--write-subs",
        "--sub-langs",
        "en.*,en",
        "--sub-format",
        "vtt",
        "-f",
        "best[ext=mp4]/best",
        "-o",
        str(output_template),
    ]

    if cookies_from_browser:
        args.extend(["--cookies-from-browser", cookies_from_browser])

    args.append(url)
    run(args)

    return pick_video_file(workspace), pick_vtt_file(workspace), metadata


def seconds_to_clock(total_seconds: float) -> str:
    seconds_int = max(0, int(total_seconds))
    hours = seconds_int // 3600
    minutes = (seconds_int % 3600) // 60
    seconds = seconds_int % 60
    if hours > 0:
        return f"{hours}:{minutes:02d}:{seconds:02d}"
    return f"{minutes}:{seconds:02d}"


def parse_vtt_timestamp(value: str) -> float:
    clean = value.strip().split(" ")[0]
    parts = clean.split(":")
    if len(parts) == 3:
        hours = int(parts[0])
        minutes = int(parts[1])
        seconds = float(parts[2].replace(",", "."))
        return hours * 3600 + minutes * 60 + seconds
    if len(parts) == 2:
        minutes = int(parts[0])
        seconds = float(parts[1].replace(",", "."))
        return minutes * 60 + seconds
    return 0.0


def clean_caption_text(text: str) -> str:
    text = re.sub(r"<[^>]+>", " ", text)
    text = text.replace("&nbsp;", " ")
    text = text.replace("&amp;", "&")
    text = text.replace("&quot;", '"')
    text = text.replace("&#39;", "'")
    text = re.sub(r"\s+", " ", text)
    return text.strip()


def parse_vtt(vtt_path: Path) -> list[dict[str, Any]]:
    lines = vtt_path.read_text(encoding="utf-8", errors="ignore").splitlines()
    segments: list[dict[str, Any]] = []
    idx = 0
    while idx < len(lines):
        line = lines[idx].strip()
        if not line or line.startswith("WEBVTT") or line.startswith("NOTE"):
            idx += 1
            continue
        if "-->" not in line:
            idx += 1
            continue

        start_raw, end_raw = [part.strip() for part in line.split("-->", 1)]
        start = parse_vtt_timestamp(start_raw)
        end = parse_vtt_timestamp(end_raw)
        idx += 1
        chunk_lines: list[str] = []
        while idx < len(lines) and lines[idx].strip():
            chunk_lines.append(lines[idx])
            idx += 1

        text = clean_caption_text(" ".join(chunk_lines))
        if text:
            duration = max(0.3, end - start)
            segments.append(
                {
                    "start_seconds": round(start, 3),
                    "duration_seconds": round(duration, 3),
                    "start_timestamp": seconds_to_clock(start),
                    "text": text,
                }
            )
        idx += 1

    deduped: list[dict[str, Any]] = []
    for segment in segments:
        if deduped and deduped[-1]["text"] == segment["text"]:
            continue
        deduped.append(segment)
    return deduped


def segments_to_markdown(segments: list[dict[str, Any]]) -> str:
    lines = ["## Transcript", ""]
    for seg in segments:
        lines.append(f"{seg['start_timestamp']} {seg['text']}")
    lines.append("")
    return "\n".join(lines)


def write_transcript_markdown(
    out_path: Path,
    source_label: str,
    method: str,
    segments: list[dict[str, Any]] | None = None,
    transcript_text: str | None = None,
) -> None:
    safe_source = source_label.replace('"', '\\"')
    header = [
        "---",
        f'source: "{safe_source}"',
        f'generated_at: "{now_utc()}"',
        f"method: {method}",
        "---",
        "",
    ]
    if segments is not None:
        header.insert(3, f"segment_count: {len(segments)}")
        body = segments_to_markdown(segments)
    else:
        body = "\n".join(["## Transcript", "", (transcript_text or "").strip(), ""])
    out_path.write_text("\n".join(header) + body, encoding="utf-8")


def transcript_quality(transcript_text: str) -> dict[str, Any]:
    stripped = transcript_text.strip()
    if not stripped:
        return {
            "quality": "none",
            "word_count": 0,
            "unique_words": 0,
            "repeated_line_ratio": 0.0,
            "reason": "no transcript text",
        }

    lines = [line.strip() for line in stripped.splitlines() if line.strip()]
    cleaned_lines = [re.sub(r"^\[[0-9:.]+\]\s*", "", line) for line in lines]
    words = re.findall(r"[a-zA-Z0-9']+", stripped.lower())
    unique_words = set(words)
    repeated_ratio = 0.0
    if cleaned_lines:
        repeated_ratio = 1.0 - (len(set(cleaned_lines)) / len(cleaned_lines))

    if len(words) < 8:
        quality = "low"
        reason = "very short transcript"
    elif repeated_ratio > 0.45:
        quality = "low"
        reason = "high line repetition"
    elif len(unique_words) < 12:
        quality = "low"
        reason = "very low vocabulary diversity"
    elif len(words) < 35:
        quality = "medium"
        reason = "short transcript"
    else:
        quality = "high"
        reason = "usable transcript"

    return {
        "quality": quality,
        "word_count": len(words),
        "unique_words": len(unique_words),
        "repeated_line_ratio": round(repeated_ratio, 3),
        "reason": reason,
    }


def extract_audio_for_fallback(video_path: Path, audio_path: Path) -> None:
    run(
        [
            "ffmpeg",
            "-y",
            "-i",
            str(video_path),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "mp3",
            str(audio_path),
        ]
    )


def detect_minutes_config_engine(source_config: Path) -> str:
    if source_config.exists():
        try:
            data = tomllib.loads(source_config.read_text(encoding="utf-8", errors="ignore"))
            transcription = data.get("transcription", {})
            engine = str(transcription.get("engine", "whisper")).strip().lower()
            if engine in {"whisper", "parakeet"}:
                return engine
        except tomllib.TOMLDecodeError:
            pass
    return "whisper"


def write_minutes_config(
    config_path: Path,
    output_dir: Path,
    source_engine: str,
    language: str | None,
    forced_engine: str | None,
) -> None:
    source_config = Path.home() / ".config" / "minutes" / "config.toml"
    data: dict[str, Any] = {}
    if source_config.exists():
        data = tomllib.loads(source_config.read_text(encoding="utf-8", errors="ignore"))

    transcription = dict(data.get("transcription", {}))
    transcription["engine"] = forced_engine or source_engine
    if "model_path" not in transcription:
        transcription["model_path"] = str(Path.home() / ".minutes" / "models")
    if language:
        transcription["language"] = language
    if "min_words" not in transcription:
        transcription["min_words"] = 1

    summarization = {"engine": "none"}
    diarization = {"engine": "none"}

    lines = [f'output_dir = "{output_dir}"', ""]

    def append_table(name: str, values: dict[str, Any]) -> None:
        lines.append(f"[{name}]")
        for key, value in values.items():
            if isinstance(value, bool):
                rendered = str(value).lower()
            elif isinstance(value, (int, float)):
                rendered = json.dumps(value)
            elif isinstance(value, list):
                rendered = json.dumps(value)
            else:
                rendered = json.dumps(str(value))
            lines.append(f"{key} = {rendered}")
        lines.append("")

    append_table("transcription", transcription)
    append_table("summarization", summarization)
    append_table("diarization", diarization)
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")


def extract_minutes_transcript(markdown_text: str) -> str | None:
    transcript_marker = "\n## Transcript"
    marker_index = markdown_text.find(transcript_marker)
    if marker_index == -1:
        if markdown_text.startswith("## Transcript"):
            transcript_start = len("## Transcript")
        else:
            return None
    else:
        transcript_start = marker_index + len(transcript_marker)

    transcript_text = markdown_text[transcript_start:].lstrip("\n")
    next_section = transcript_text.find("\n## ")
    if next_section != -1:
        transcript_text = transcript_text[:next_section]
    transcript_text = transcript_text.strip()
    return transcript_text or None


def transcribe_with_minutes(audio_path: Path, workspace: Path) -> tuple[str | None, str | None]:
    minutes_bin = shutil.which("minutes")
    if not minutes_bin:
        return None, None

    backend_mode = os.environ.get("VIDEO_REVIEW_MINUTES_MODE", "auto").strip().lower()
    if backend_mode in {"0", "false", "off", "disabled"}:
        return None, None

    source_config = Path.home() / ".config" / "minutes" / "config.toml"
    configured_engine = detect_minutes_config_engine(source_config)
    requested_engine = configured_engine if backend_mode == "auto" else backend_mode
    language = os.environ.get("VIDEO_REVIEW_TRANSCRIPT_LANGUAGE", "en")
    timeout_seconds = int(os.environ.get("VIDEO_REVIEW_MINUTES_TIMEOUT_SECONDS", "180"))

    xdg_config_home = workspace / "minutes-xdg"
    output_dir = workspace / "minutes-output"
    config_path = xdg_config_home / "minutes" / "config.toml"

    def run_minutes_process(forced_engine: str | None) -> tuple[str | None, str | None]:
        write_minutes_config(config_path, output_dir, configured_engine, language, forced_engine)
        env = os.environ.copy()
        env["XDG_CONFIG_HOME"] = str(xdg_config_home)
        result = subprocess.run(
            [
                minutes_bin,
                "process",
                str(audio_path),
                "--content-type",
                "memo",
                "--title",
                "video review transcription",
            ],
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            env=env,
        )

        payload = extract_last_json_object(result.stdout or "")
        combined_stderr = (result.stderr or "").strip()
        if result.returncode != 0:
            return None, combined_stderr or (result.stdout or "").strip() or "minutes process failed"

        output_path: Path | None = None
        if payload and isinstance(payload.get("file"), str):
            output_path = Path(payload["file"])
        if not output_path or not output_path.exists():
            return None, "minutes process succeeded but did not return a readable output file"

        markdown_text = output_path.read_text(encoding="utf-8", errors="ignore")
        transcript = extract_minutes_transcript(markdown_text)
        if transcript:
            method_engine = forced_engine or requested_engine
            return transcript, f"minutes-{method_engine}"
        return None, "minutes process succeeded but transcript section was empty"

    transcript, method = run_minutes_process(None if backend_mode == "auto" else requested_engine)
    if transcript:
        return transcript, method

    if method and "engine 'parakeet' not compiled in" in method.lower():
        retry_transcript, retry_method = run_minutes_process("whisper")
        if retry_transcript:
            return retry_transcript, "minutes-whisper-fallback"
        method = retry_method or method

    if method:
        print(f"Warning: minutes backend unavailable.\n{method}", file=sys.stderr)
    return None, None


def transcribe_with_local_whisper(audio_path: Path, workspace: Path) -> str | None:
    whisper_bin = shutil.which("whisper")
    if not whisper_bin:
        return None

    out_dir = workspace / "whisper-out"
    out_dir.mkdir(parents=True, exist_ok=True)
    model = os.environ.get("VIDEO_REVIEW_LOCAL_WHISPER_MODEL", "turbo")
    language = os.environ.get("VIDEO_REVIEW_TRANSCRIPT_LANGUAGE", "en")
    transcript_file = out_dir / f"{audio_path.stem}.txt"
    timeout_seconds = int(os.environ.get("VIDEO_REVIEW_LOCAL_WHISPER_TIMEOUT_SECONDS", "150"))
    ready_grace_seconds = int(os.environ.get("VIDEO_REVIEW_LOCAL_WHISPER_READY_GRACE_SECONDS", "8"))

    command = [
        whisper_bin,
        str(audio_path),
        "--model",
        model,
        "--task",
        "transcribe",
        "--language",
        language,
        "--output_format",
        "txt",
        "--output_dir",
        str(out_dir),
        "--fp16",
        "False",
    ]

    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )

    start_time = time.monotonic()
    transcript_ready_at: float | None = None
    stdout_output = ""
    stderr_output = ""
    try:
        while True:
            if transcript_file.exists() and transcript_file.stat().st_size > 0:
                transcript_ready_at = transcript_ready_at or time.monotonic()
            if process.poll() is not None:
                break

            elapsed = time.monotonic() - start_time
            if transcript_ready_at is not None and (time.monotonic() - transcript_ready_at) >= ready_grace_seconds:
                kill_process_tree(process)
                break
            if transcript_ready_at is None and elapsed >= timeout_seconds:
                kill_process_tree(process)
                break
            time.sleep(0.5)
    finally:
        if process.poll() is not None:
            try:
                stdout_output, stderr_output = process.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                kill_process_tree(process)
                stdout_output, stderr_output = process.communicate()
        else:
            kill_process_tree(process)
            stdout_output, stderr_output = process.communicate()

    if transcript_file.exists():
        transcript = transcript_file.read_text(encoding="utf-8", errors="ignore").strip()
        if transcript:
            return transcript

    stdout_transcript = extract_whisper_stdout_transcript(stdout_output)
    if stdout_transcript:
        return stdout_transcript

    stderr_output = stderr_output.strip()
    if stderr_output:
        print(
            f"Warning: local whisper did not return a transcript cleanly.\n{stderr_output}",
            file=sys.stderr,
        )
    return None


def transcribe_with_openai(audio_path: Path) -> str | None:
    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        return None

    model = os.environ.get("VIDEO_REVIEW_OPENAI_TRANSCRIBE_MODEL", "gpt-4o-transcribe")
    try:
        result = run(
            [
                "openai",
                "api",
                "audio.transcriptions.create",
                "-m",
                model,
                "-f",
                str(audio_path),
                "--response-format",
                "text",
            ]
        )
    except RuntimeError:
        result = run(
            [
                "openai",
                "api",
                "audio.transcriptions.create",
                "-m",
                "whisper-1",
                "-f",
                str(audio_path),
                "--response-format",
                "text",
            ]
        )

    transcript = (result.stdout or "").strip()
    return transcript if transcript else None


def probe_duration_seconds(video_path: Path) -> float:
    result = run(
        [
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            str(video_path),
        ]
    )
    raw = (result.stdout or "").strip()
    try:
        return max(0.0, float(raw))
    except ValueError as exc:
        raise RuntimeError(f"Could not parse video duration from ffprobe output: {raw}") from exc


def video_has_audio_stream(video_path: Path) -> bool:
    result = run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            str(video_path),
        ]
    )
    return bool((result.stdout or "").strip())


def choose_frame_sampling(
    duration_seconds: float,
    requested_frame_step: int | None,
    requested_max_frames: int | None,
) -> tuple[int, int]:
    if requested_frame_step is not None and requested_max_frames is not None:
        return max(1, requested_frame_step), max(1, requested_max_frames)

    if duration_seconds <= 600:
        default_max = 30
    elif duration_seconds <= 1800:
        default_max = 42
    else:
        default_max = 54

    if requested_max_frames is not None:
        default_max = max(1, requested_max_frames)

    step = max(8, int(duration_seconds / default_max)) if duration_seconds > 0 else 20
    if requested_frame_step is not None:
        step = max(1, requested_frame_step)

    estimated_frames = max(1, int(duration_seconds / step)) if duration_seconds > 0 else default_max
    max_frames = min(default_max, estimated_frames) if duration_seconds > 0 else default_max
    max_frames = max(1, max_frames)
    return step, max_frames


def extract_keyframes(
    video_path: Path,
    keyframes_dir: Path,
    frame_step_seconds: int,
    max_frames: int,
) -> list[Path]:
    keyframes_dir.mkdir(parents=True, exist_ok=True)
    pattern = keyframes_dir / "frame-%03d.jpg"
    run(
        [
            "ffmpeg",
            "-y",
            "-i",
            str(video_path),
            "-vf",
            f"fps=1/{frame_step_seconds},scale=960:-2",
            "-q:v",
            "3",
            "-frames:v",
            str(max_frames),
            str(pattern),
        ]
    )
    return sorted(keyframes_dir.glob("frame-*.jpg"))


def build_contact_sheet(keyframes_dir: Path, output_path: Path) -> Path | None:
    frames = sorted(keyframes_dir.glob("frame-*.jpg"))
    if not frames:
        return None
    columns = 4 if len(frames) > 8 else 3
    rows = max(1, (len(frames) + columns - 1) // columns)
    run(
        [
            "ffmpeg",
            "-y",
            "-pattern_type",
            "glob",
            "-i",
            str(keyframes_dir / "frame-*.jpg"),
            "-vf",
            f"scale=480:-1,tile={columns}x{rows}:padding=8:margin=8:color=white",
            "-frames:v",
            "1",
            str(output_path),
        ]
    )
    return output_path if output_path.exists() else None


def heuristic_analysis(
    transcript_text: str,
    focus: str,
    source_kind: str,
    media_title: str | None,
    media_description: str | None,
    transcript_stats: dict[str, Any],
    sampled_frames: int,
    has_contact_sheet: bool,
) -> dict[str, Any]:
    def parse_timestamped_segments(text: str) -> list[dict[str, str]]:
        segments: list[dict[str, str]] = []
        pattern = re.compile(
            r"^(?:\[(?P<bracket_ts>[0-9:.]+)\]|(?P<plain_ts>[0-9:.]+))\s+(?P<body>.+?)\s*$"
        )
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            match = pattern.match(line)
            if match:
                ts = match.group("bracket_ts") or match.group("plain_ts") or "unknown"
                body = match.group("body").strip()
            else:
                ts = "unknown"
                body = line
            segments.append({"timestamp": ts, "text": body, "lower": body.lower()})
        return segments

    segments = parse_timestamped_segments(transcript_text)
    lowered = transcript_text.lower()
    title = (media_title or "").strip()
    description = (media_description or "").strip()
    title_lower = title.lower()
    description_lower = description.lower()
    combined_meta = " ".join(part for part in [title_lower, description_lower, focus.lower()] if part).strip()
    if not segments:
        return {
            "overall_summary": "No transcript content was available to analyze.",
            "sender_intent": f"Video likely related to: {focus}",
            "primary_signal": "unclear",
            "confidence": 0.05,
            "content_type": "unknown",
            "review_mode": "frame-first",
            "likely_product_areas": [focus],
            "problem_signals": [],
            "proposal_signals": [],
            "evidence": [],
            "recommended_next_actions": [
                "Review the sampled frames directly because no usable transcript was available.",
            ],
            "clarifying_questions": [
                "Can the sender provide a short written summary of what they are trying to show?",
            ],
        }

    content_type = "walkthrough"
    if any(token in combined_meta for token in ["tutorial", "how to", "guide"]):
        content_type = "tutorial"
    elif any(token in combined_meta for token in ["demo", "prototype", "walkthrough"]):
        content_type = "product-demo"
    elif any(token in combined_meta for token in ["team culture", "just for fun", "culture"]):
        content_type = "culture-update"
    elif any(token in combined_meta for token in ["bug", "issue", "broken", "error"]):
        content_type = "bug-report"

    issue_rules: list[dict[str, Any]] = [
        {
            "issue": "Pending introductions are not appearing in the approval queue after the invitation flow.",
            "patterns": [
                "zero intro pending",
                "review pending introduction zero",
                "pending introductions for approval",
                "no introductions right now",
                "still not seeing it",
            ],
            "area": "intro approvals / invitation review",
        },
        {
            "issue": "Team settings show duplicate participant entries.",
            "patterns": [
                "twice",
                "duplicate",
            ],
            "area": "team settings / participant visibility",
        },
        {
            "issue": "Unexpected or duplicated people in the team list may confuse recipients.",
            "patterns": [
                "this may confuse them",
                "not sure know who",
                "why they're here",
                "filter out these guys",
            ],
            "area": "team settings / recipient visibility",
        },
        {
            "issue": "Advisor relationship state may be missing or disconnected.",
            "patterns": [
                "no advisor relationship found",
            ],
            "area": "advisor relationships",
        },
    ]

    walkthrough_theme_rules: list[dict[str, Any]] = [
        {
            "theme": "cross-functional solution workshop",
            "patterns": ["solution workshop", "cross functional", "cross-functional"],
            "area": "product workflow / planning",
        },
        {
            "theme": "research or technical investigation before committing to a solution",
            "patterns": ["technical spike", "user research", "data investigation", "gather more information"],
            "area": "discovery / validation",
        },
        {
            "theme": "design and technical breakdown into atomic work units",
            "patterns": ["design process", "technical breakdown", "atomic units", "data requirements"],
            "area": "design and implementation planning",
        },
        {
            "theme": "continuous shipping behind feature flags",
            "patterns": ["feature flags", "ship continuously", "launching and experimenting"],
            "area": "delivery / rollout",
        },
        {
            "theme": "feeding learnings back into future user stories",
            "patterns": ["learnings", "user stories", "future definition of user stories"],
            "area": "feedback loop / iteration",
        },
        {
            "theme": "team alignment through a walkthrough of a diagram or flowchart",
            "patterns": ["diagram", "flow chart", "workflow", "walkthrough"],
            "area": "communication / alignment",
        },
    ]

    matched_issues: list[str] = []
    matched_areas: list[str] = []
    evidence: list[dict[str, str]] = []
    seen_issue = set()
    seen_evidence = set()

    for rule in issue_rules:
        matched_segment: dict[str, str] | None = None
        for segment in segments:
            if any(pattern in segment["lower"] for pattern in rule["patterns"]):
                matched_segment = segment
                break
        if matched_segment:
            issue = rule["issue"]
            if issue not in seen_issue:
                matched_issues.append(issue)
                seen_issue.add(issue)
            area = rule["area"]
            if area not in matched_areas:
                matched_areas.append(area)
            evidence_key = (matched_segment["timestamp"], matched_segment["text"])
            if evidence_key not in seen_evidence:
                evidence.append(
                    {
                        "timestamp": matched_segment["timestamp"],
                        "note": matched_segment["text"],
                    }
                )
                seen_evidence.add(evidence_key)

    walkthrough_themes: list[str] = []
    walkthrough_evidence: list[dict[str, str]] = []
    seen_theme = set()
    for rule in walkthrough_theme_rules:
        matched_segment: dict[str, str] | None = None
        for segment in segments:
            if any(pattern in segment["lower"] for pattern in rule["patterns"]):
                matched_segment = segment
                break
        if matched_segment:
            theme = rule["theme"]
            if theme not in seen_theme:
                walkthrough_themes.append(theme)
                seen_theme.add(theme)
            area = rule["area"]
            if area not in matched_areas:
                matched_areas.append(area)
            walkthrough_evidence.append(
                {
                    "timestamp": matched_segment["timestamp"],
                    "note": matched_segment["text"],
                }
            )

    proposal_tokens = [
        "should",
        "could",
        "feature",
        "proposal",
        "idea",
        "improve",
        "add",
        "change",
        "would just",
        "filter out",
    ]
    proposal_hits = []
    for segment in segments:
        if any(token in segment["lower"] for token in proposal_tokens):
            if len(segment["text"].split()) >= 3:
                proposal_hits.append(segment["text"])
    proposal_hits = proposal_hits[:3]

    if transcript_stats.get("quality") == "low" and content_type in {"product-demo", "tutorial", "culture-update"}:
        primary_signal = "unclear"
    elif matched_issues and proposal_hits:
        primary_signal = "bug" if len(matched_issues) >= len(proposal_hits) else "mixed"
    elif matched_issues:
        primary_signal = "bug"
    elif walkthrough_themes:
        primary_signal = "proposal"
    elif proposal_hits:
        primary_signal = "proposal"
    elif transcript_text.strip():
        primary_signal = "question"
    else:
        primary_signal = "unclear"

    likely_product_areas = matched_areas[:]
    if "invitation" in lowered or "intro request" in lowered:
        if "intro approvals / invitation review" not in likely_product_areas:
            likely_product_areas.insert(0, "intro approvals / invitation review")
    if (
        "settings" in lowered
        and any(token in lowered for token in ["results facilitator", "twice", "duplicate", "pending introductions"])
    ):
        if "team settings / participant visibility" not in likely_product_areas:
            likely_product_areas.append("team settings / participant visibility")
    if not likely_product_areas:
        likely_product_areas = [focus]
    if walkthrough_themes and "product workflow / planning" not in likely_product_areas:
        likely_product_areas.insert(0, "product workflow / planning")
    if content_type == "tutorial" and "tutorial / education flow" not in likely_product_areas:
        likely_product_areas.insert(0, "tutorial / education flow")
    if content_type == "product-demo" and "product demo / showcase" not in likely_product_areas:
        likely_product_areas.insert(0, "product demo / showcase")
    if content_type == "culture-update" and "team culture / async update" not in likely_product_areas:
        likely_product_areas.insert(0, "team culture / async update")

    first_lines = [segment["text"] for segment in segments[:4]]
    first_line_summary = " ".join(first_lines[:2]).strip()
    if transcript_stats.get("quality") == "low" and content_type in {"product-demo", "tutorial", "culture-update"}:
        sender_intent = (
            title
            or description
            or f"Show a {content_type.replace('-', ' ')} related to {focus}"
        )
    elif walkthrough_themes and title:
        sender_intent = (
            f"Walk through {title.lower()} and explain how the proposed workflow should operate end to end."
        )
    elif matched_issues:
        sender_intent = (
            "Show that the invitation / intro-review flow is not surfacing the expected pending introductions, "
            "and point out confusing duplicate participant visibility in team settings."
            if len(matched_issues) > 1
            else f"Show that: {matched_issues[0]}"
        )
    else:
        sender_intent = first_line_summary or f"Walkthrough likely related to: {focus}"

    if transcript_stats.get("quality") == "low" and content_type in {"product-demo", "tutorial", "culture-update"}:
        overall_summary = (
            f"Transcript quality is {transcript_stats.get('quality')}, so this bundle should be reviewed in frame-first mode. "
            f"The available metadata suggests this is a {content_type.replace('-', ' ')} video."
        )
    elif walkthrough_themes:
        summary_bits = walkthrough_themes[:4]
        overall_summary = (
            "This walkthrough proposes a delivery workflow that emphasizes "
            + "; ".join(summary_bits[:-1] + [summary_bits[-1]])
            + "."
        )
    elif matched_issues:
        overall_summary = " ".join(
            [
                "The sender demonstrates that a resent invitation appears to send successfully,",
                "but the intro review flow still shows no pending introductions.",
                "They also call out duplicate or confusing people listed in team settings."
                if any("duplicate" in issue.lower() or "confus" in issue.lower() for issue in matched_issues)
                else "",
            ]
        ).strip()
        overall_summary = re.sub(r"\s+", " ", overall_summary)
    else:
        overall_summary = "Transcript-first analysis found no strong bug or proposal pattern yet."

    if transcript_stats.get("quality") == "low" and content_type in {"product-demo", "tutorial", "culture-update"}:
        confidence = 0.55
    elif len(matched_issues) >= 2:
        confidence = 0.82
    elif len(matched_issues) == 1:
        confidence = 0.68
    elif walkthrough_themes:
        confidence = 0.76
    elif proposal_hits:
        confidence = 0.45
    else:
        confidence = 0.25

    recommended_next_actions = []
    review_mode = "transcript-first"
    if transcript_stats.get("quality") == "low":
        review_mode = "frame-first"
    if any("pending introductions" in issue.lower() for issue in matched_issues):
        recommended_next_actions.append(
            "Trace the intro invitation state transition from resend through the approval queue and verify why the pending-intro count remains zero."
        )
    if any("duplicate" in issue.lower() for issue in matched_issues):
        recommended_next_actions.append(
            "Inspect the team-members / results-facilitator list for duplicate entries and confirm whether the duplication is data-level or presentation-level."
        )
    if any("confus" in issue.lower() or "recipients" in issue.lower() for issue in matched_issues):
        recommended_next_actions.append(
            "Review which people should be visible to this user in settings and whether unexpected names should be filtered from the recipient-facing experience."
        )
    if any("advisor relationship" in issue.lower() for issue in matched_issues):
        recommended_next_actions.append(
            "Check the advisor relationship lookup for this account and verify whether the missing relationship is expected or a data-linking bug."
        )
    if walkthrough_themes and not matched_issues:
        recommended_next_actions = [
            "Summarize the proposed workflow into a short sequence of stages that a PM or eng lead could react to quickly.",
            "Identify where this proposal expects cross-functional alignment, research, and technical breakdown before implementation.",
            "Pull out any rollout mechanics like feature flags, experimentation, or support/go-to-market handoff for follow-up discussion.",
        ]
    if not recommended_next_actions:
        recommended_next_actions = [
            "Review the transcript and sampled frames together to confirm the first important visual state change.",
            "Turn each confirmed issue or request into one clear engineering or product follow-up item.",
        ]
    if review_mode == "frame-first":
        recommended_next_actions = [
            "Open the contact sheet first to map the main screens and UI transitions before drilling into individual frames.",
            "Use the sampled frames to identify the key app surfaces, workflows, or forms being demonstrated.",
            "Only use the transcript as a weak supporting signal because the speech recognition quality is low for this clip.",
        ]

    if content_type in {"product-demo", "tutorial", "culture-update"} and transcript_stats.get("quality") == "low":
        clarifying_questions = [
            "What part of the visual flow is most important for the reviewer to focus on?",
            "Is this video meant to demonstrate a feature, teach a workflow, or communicate an update?",
            "What decision or follow-up do you want from the viewer after watching this clip?",
        ]
    elif walkthrough_themes and not matched_issues:
        clarifying_questions = [
            "Is this walkthrough intended to propose a new team workflow or document the current one?",
            "Which step in the proposed flow is the biggest open question or coordination risk?",
            "What decision do you want stakeholders to make after reviewing this workflow?",
        ]
    else:
        clarifying_questions = [
            "Which screen is supposed to show the pending introduction after resend?",
            "Are the duplicated people in settings also duplicated in the underlying data, or only in the UI?",
            "Should the user be able to approve the introduction from the current account role, or is that expected to happen elsewhere?",
        ]

    if review_mode == "frame-first":
        evidence = [
            {
                "timestamp": "metadata",
                "note": f"Title suggests a {content_type.replace('-', ' ')}: {title or focus}",
            },
            {
                "timestamp": "metadata",
                "note": (
                    f"Transcript quality is {transcript_stats.get('quality')} "
                    f"({transcript_stats.get('reason')}); rely more on visual evidence."
                ),
            },
        ]
        if sampled_frames > 0:
            frame_note = f"Sampled {sampled_frames} frames"
            if has_contact_sheet:
                frame_note += " with a generated contact sheet for quick visual sweep."
            else:
                frame_note += " for manual visual review."
            evidence.append({"timestamp": "frames", "note": frame_note})
    elif walkthrough_themes and not matched_issues:
        evidence = walkthrough_evidence[:5]

    return {
        "overall_summary": overall_summary,
        "sender_intent": sender_intent,
        "primary_signal": primary_signal,
        "confidence": confidence,
        "content_type": content_type,
        "review_mode": review_mode,
        "likely_product_areas": likely_product_areas[:5],
        "problem_signals": matched_issues[:5],
        "proposal_signals": proposal_hits[:5],
        "evidence": evidence[:5],
        "recommended_next_actions": recommended_next_actions[:5],
        "clarifying_questions": clarifying_questions,
    }


def build_markdown_report(
    *,
    source_label: str,
    source_kind: str,
    focus: str,
    transcript_method: str,
    transcript_path: Path | None,
    metadata_path: Path,
    keyframes_dir: Path,
    contact_sheet_path: Path | None,
    analysis: dict[str, Any],
) -> str:
    lines = [
        "# Video Review",
        "",
        f"- Source: `{source_label}`",
        f"- Source kind: `{source_kind}`",
        f"- Generated: `{now_utc()}`",
        f"- Focus: `{focus}`",
        f"- Transcript method: `{transcript_method}`",
        f"- Frames: `{keyframes_dir}`",
        f"- Contact sheet: `{contact_sheet_path}`" if contact_sheet_path else "- Contact sheet: `none`",
        f"- Metadata: `{metadata_path}`",
        "",
        "## Executive Summary",
        "",
        str(analysis.get("overall_summary", "No summary produced.")),
        "",
        "## Sender Intent",
        "",
        str(analysis.get("sender_intent", "Unknown")),
        "",
        "## Primary Signal",
        "",
        f"- `{analysis.get('primary_signal', 'unclear')}`",
        f"- Confidence: `{analysis.get('confidence', 'n/a')}`",
        f"- Content type: `{analysis.get('content_type', 'unknown')}`",
        f"- Review mode: `{analysis.get('review_mode', 'transcript-first')}`",
        "",
        "## Likely Product Areas",
        "",
    ]

    for item in analysis.get("likely_product_areas", []) or ["Unknown"]:
        lines.append(f"- {item}")

    lines.extend(["", "## Problem Signals", ""])
    for item in analysis.get("problem_signals", []) or ["None detected"]:
        lines.append(f"- {item}")

    lines.extend(["", "## Proposal Signals", ""])
    for item in analysis.get("proposal_signals", []) or ["None detected"]:
        lines.append(f"- {item}")

    lines.extend(["", "## Evidence", ""])
    evidence = analysis.get("evidence", []) or []
    if evidence:
        for ev in evidence:
            timestamp = ev.get("timestamp", "unknown")
            note = ev.get("note", "")
            lines.append(f"- `{timestamp}` {note}")
    else:
        lines.append("- No explicit timestamp evidence extracted.")

    lines.extend(["", "## Recommended Next Actions", ""])
    for item in analysis.get("recommended_next_actions", []) or ["No actions generated"]:
        lines.append(f"1. {item}")

    lines.extend(["", "## Clarifying Questions", ""])
    for item in analysis.get("clarifying_questions", []) or ["None"]:
        lines.append(f"- {item}")

    if transcript_path:
        lines.extend(["", "## Transcript Artifact", "", f"- `{transcript_path}`"])

    lines.append("")
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze local or hosted walkthrough videos into durable Minutes video-review bundles."
    )
    parser.add_argument(
        "source",
        help="Loom/ScreenPal URL, another yt-dlp-resolvable hosted video URL, or local video file path",
    )
    parser.add_argument(
        "--out-dir",
        default=str(Path.home() / ".minutes" / "video-reviews"),
        help="Directory for generated bundles (default: ~/.minutes/video-reviews)",
    )
    parser.add_argument(
        "--focus",
        default="product walkthrough",
        help="Context focus for analysis (example: customer signup bug repro)",
    )
    parser.add_argument(
        "--cookies-from-browser",
        default=None,
        help="Browser name for yt-dlp cookies (example: chrome, brave, safari)",
    )
    parser.add_argument(
        "--env-file",
        default=None,
        help="Optional .env file to load before provider auth/model resolution",
    )
    parser.add_argument(
        "--frame-step",
        type=int,
        default=None,
        help="Override adaptive seconds between sampled frames",
    )
    parser.add_argument(
        "--max-frames",
        type=int,
        default=None,
        help="Override adaptive cap on sampled frames",
    )
    parser.add_argument(
        "--keep-temp",
        action="store_true",
        help="Keep temporary workspace for debugging",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.env_file:
        load_env_file(Path(args.env_file).expanduser().resolve())

    source_is_url = is_url(args.source)
    source_kind = detect_source_kind(args.source)
    ensure_dependencies(source_is_url)

    out_root = Path(args.out_dir).expanduser().resolve()
    out_root.mkdir(parents=True, exist_ok=True)

    timestamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    source_slug = slugify(Path(args.source).stem if not source_is_url else source_kind)
    bundle_dir = out_root / f"{timestamp}-{source_slug}"
    bundle_dir.mkdir(parents=True, exist_ok=True)

    transcript_out = bundle_dir / "transcript.md"
    analysis_md = bundle_dir / "analysis.md"
    analysis_json = bundle_dir / "analysis.json"
    metadata_json = bundle_dir / "metadata.json"
    keyframes_dir = bundle_dir / "frames"

    temp_dir_obj = tempfile.TemporaryDirectory(prefix="minutes-video-review-")
    workspace = Path(temp_dir_obj.name)

    source_label = args.source
    transcript_text = ""
    transcript_method = "none"
    transcript_artifact: Path | None = None

    try:
        if source_is_url:
            video_path, vtt_path, media_info = download_hosted_video(
                args.source,
                workspace,
                args.cookies_from_browser,
            )
        else:
            path = Path(args.source).expanduser().resolve()
            if not path.exists():
                raise RuntimeError(f"Local file does not exist: {path}")
            if path.suffix.lower() not in SUPPORTED_VIDEO_EXTENSIONS:
                raise RuntimeError(
                    f"Unsupported video extension: {path.suffix}. "
                    f"Expected one of: {', '.join(sorted(SUPPORTED_VIDEO_EXTENSIONS))}"
                )
            video_path = workspace / f"source{path.suffix.lower()}"
            shutil.copy2(path, video_path)
            vtt_path = None
            source_label = str(path)
            media_info = {
                "title": path.stem,
                "description": None,
                "uploader": None,
                "duration_seconds": None,
                "webpage_url": None,
            }

        duration_seconds = probe_duration_seconds(video_path)
        has_audio_stream = video_has_audio_stream(video_path)

        if vtt_path and vtt_path.exists():
            segments = parse_vtt(vtt_path)
            if segments:
                write_transcript_markdown(
                    transcript_out,
                    source_label,
                    "vtt_captions",
                    segments=segments,
                )
                transcript_artifact = transcript_out
                transcript_text = "\n".join(
                    f"{seg['start_timestamp']} {seg['text']}" for seg in segments
                )
                transcript_method = "vtt_captions"

        audio_path = workspace / "audio-fallback.mp3"
        if has_audio_stream and not transcript_text.strip():
            extract_audio_for_fallback(video_path, audio_path)
            minutes_transcript, minutes_method = transcribe_with_minutes(audio_path, workspace)
            if minutes_transcript:
                write_transcript_markdown(
                    transcript_out,
                    source_label,
                    minutes_method or "minutes",
                    transcript_text=minutes_transcript,
                )
                transcript_artifact = transcript_out
                transcript_text = minutes_transcript
                transcript_method = minutes_method or "minutes"

        if has_audio_stream and not transcript_text.strip():
            if not audio_path.exists():
                extract_audio_for_fallback(video_path, audio_path)
            local_transcript = transcribe_with_local_whisper(audio_path, workspace)
            if local_transcript:
                write_transcript_markdown(
                    transcript_out,
                    source_label,
                    "local_whisper_cli",
                    transcript_text=local_transcript,
                )
                transcript_artifact = transcript_out
                transcript_text = local_transcript
                transcript_method = "local_whisper_cli"

        if has_audio_stream and not transcript_text.strip():
            if not audio_path.exists():
                extract_audio_for_fallback(video_path, audio_path)
            openai_transcript = transcribe_with_openai(audio_path)
            if openai_transcript:
                write_transcript_markdown(
                    transcript_out,
                    source_label,
                    "openai_audio_transcription",
                    transcript_text=openai_transcript,
                )
                transcript_artifact = transcript_out
                transcript_text = openai_transcript
                transcript_method = "openai_audio_transcription"

        frame_step_seconds, max_frames = choose_frame_sampling(
            duration_seconds,
            args.frame_step,
            args.max_frames,
        )
        keyframes = extract_keyframes(
            video_path,
            keyframes_dir,
            frame_step_seconds,
            max_frames,
        )
        contact_sheet_path = build_contact_sheet(keyframes_dir, bundle_dir / "contact-sheet.jpg")
        transcript_stats = transcript_quality(transcript_text)
        analysis = heuristic_analysis(
            transcript_text,
            args.focus,
            source_kind,
            media_info.get("title"),
            media_info.get("description"),
            transcript_stats,
            len(keyframes),
            contact_sheet_path is not None,
        )
        metadata = {
            "source": source_label,
            "source_kind": source_kind,
            "generated_at": now_utc(),
            "bundle_dir": str(bundle_dir),
            "video_path": str(video_path),
            "video_duration_seconds": round(duration_seconds, 3),
            "video_has_audio_stream": has_audio_stream,
            "media_title": media_info.get("title"),
            "media_description": media_info.get("description"),
            "media_uploader": media_info.get("uploader"),
            "transcript_method": transcript_method,
            "transcript_quality": transcript_stats,
            "transcript_artifact": str(transcript_artifact) if transcript_artifact else None,
            "analysis_artifact": str(analysis_md),
            "frames_dir": str(keyframes_dir),
            "contact_sheet_artifact": str(contact_sheet_path) if contact_sheet_path else None,
            "frame_step_seconds": frame_step_seconds,
            "max_frames": max_frames,
            "sampled_frames": len(keyframes),
            "focus": args.focus,
        }
        metadata_json.write_text(json.dumps(metadata, indent=2), encoding="utf-8")

        analysis_payload = {
            "source": source_label,
            "source_kind": source_kind,
            "generated_at": now_utc(),
            "focus": args.focus,
            "transcript_method": transcript_method,
            "transcript_quality": transcript_stats,
            "media_title": media_info.get("title"),
            "media_description": media_info.get("description"),
            "transcript_artifact": str(transcript_artifact) if transcript_artifact else None,
            "metadata_artifact": str(metadata_json),
            "frames_dir": str(keyframes_dir),
            "contact_sheet_artifact": str(contact_sheet_path) if contact_sheet_path else None,
            "analysis": analysis,
        }
        analysis_json.write_text(json.dumps(analysis_payload, indent=2), encoding="utf-8")

        analysis_md.write_text(
            build_markdown_report(
                source_label=source_label,
                source_kind=source_kind,
                focus=args.focus,
                transcript_method=transcript_method,
                transcript_path=transcript_artifact,
                metadata_path=metadata_json,
                keyframes_dir=keyframes_dir,
                contact_sheet_path=contact_sheet_path,
                analysis=analysis,
            ),
            encoding="utf-8",
        )

        result = {
            "bundle_dir": str(bundle_dir),
            "analysis_md": str(analysis_md),
            "analysis_json": str(analysis_json),
            "transcript_md": str(transcript_artifact) if transcript_artifact else None,
            "metadata_json": str(metadata_json),
            "frames_dir": str(keyframes_dir),
            "contact_sheet_artifact": str(contact_sheet_path) if contact_sheet_path else None,
            "transcript_method": transcript_method,
            "transcript_quality": transcript_stats,
            "sampled_frames": len(keyframes),
            "frame_step_seconds": frame_step_seconds,
            "max_frames": max_frames,
        }
        print(json.dumps(result, indent=2))
        return 0
    finally:
        if args.keep_temp:
            print(f"Kept temp workspace: {workspace}", file=sys.stderr)
            temp_dir_obj.cleanup = lambda: None  # type: ignore[method-assign]
        else:
            temp_dir_obj.cleanup()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        raise SystemExit(1)
