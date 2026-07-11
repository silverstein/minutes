use crate::config::Config;
use crate::markdown::ContentType;
use chrono::SecondsFormat;
use serde::Serialize;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const TRACE_FILE: &str = "process-audio-trace.jsonl";

#[derive(Clone, Debug, Default)]
pub struct ProcessTraceContext {
    input_path: String,
    content_type: String,
    language: Option<String>,
    model_path: Option<String>,
    vad_enabled: Option<bool>,
    audio_samples: Option<usize>,
    audio_duration_secs: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ProcessTraceEvent<'a> {
    ts: String,
    stage: &'a str,
    input_path: &'a str,
    content_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vad_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_samples: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_duration_secs: Option<f64>,
    #[serde(flatten)]
    extra: serde_json::Value,
}

thread_local! {
    static PROCESS_TRACE_CONTEXT: RefCell<Option<ProcessTraceContext>> = const { RefCell::new(None) };
}

impl ProcessTraceContext {
    pub fn new(input_path: &Path, content_type: ContentType, config: &Config) -> Self {
        let model_path = configured_model_path(config).map(|path| path.display().to_string());
        let vad_enabled = Some(!config.transcription.vad_model.trim().is_empty());
        Self {
            input_path: input_path.display().to_string(),
            content_type: content_type_label(content_type).to_string(),
            language: config.transcription.language.clone(),
            model_path,
            vad_enabled,
            audio_samples: None,
            audio_duration_secs: None,
        }
    }
}

pub struct ProcessTraceGuard;

impl Drop for ProcessTraceGuard {
    fn drop(&mut self) {
        PROCESS_TRACE_CONTEXT.with(|current| {
            current.borrow_mut().take();
        });
    }
}

pub fn start_process_trace(
    input_path: &Path,
    content_type: ContentType,
    config: &Config,
) -> Option<ProcessTraceGuard> {
    if !enabled() {
        return None;
    }
    let context = ProcessTraceContext::new(input_path, content_type, config);
    PROCESS_TRACE_CONTEXT.with(|current| {
        *current.borrow_mut() = Some(context);
    });
    stage("process.start");
    Some(ProcessTraceGuard)
}

pub fn is_active() -> bool {
    PROCESS_TRACE_CONTEXT.with(|current| current.borrow().is_some())
}

pub fn stage(stage: &'static str) {
    stage_with_extra(stage, serde_json::Value::Null);
}

pub fn stage_with_extra(stage: &'static str, extra: serde_json::Value) {
    if !enabled() {
        return;
    }
    let context = PROCESS_TRACE_CONTEXT.with(|current| current.borrow().clone());
    let Some(context) = context else {
        return;
    };
    write_event(stage, &context, extra);
}

pub fn update_model_path(path: &Path) {
    if !enabled() {
        return;
    }
    PROCESS_TRACE_CONTEXT.with(|current| {
        if let Some(context) = current.borrow_mut().as_mut() {
            context.model_path = Some(path.display().to_string());
        }
    });
}

pub fn update_vad_enabled(is_enabled: bool) {
    if !enabled() {
        return;
    }
    PROCESS_TRACE_CONTEXT.with(|current| {
        if let Some(context) = current.borrow_mut().as_mut() {
            context.vad_enabled = Some(is_enabled);
        }
    });
}

pub fn update_audio(samples: usize) {
    if !enabled() {
        return;
    }
    PROCESS_TRACE_CONTEXT.with(|current| {
        if let Some(context) = current.borrow_mut().as_mut() {
            context.audio_samples = Some(samples);
            context.audio_duration_secs = Some(samples as f64 / 16_000.0);
        }
    });
}

fn write_event(stage: &'static str, context: &ProcessTraceContext, extra: serde_json::Value) {
    let event = ProcessTraceEvent {
        ts: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        stage,
        input_path: &context.input_path,
        content_type: &context.content_type,
        language: context.language.as_deref(),
        model_path: context.model_path.as_deref(),
        vad_enabled: context.vad_enabled,
        audio_samples: context.audio_samples,
        audio_duration_secs: context.audio_duration_secs,
        extra,
    };

    let Ok(line) = serde_json::to_string(&event) else {
        return;
    };
    let Some(path) = trace_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    if writeln!(file, "{line}").is_err() {
        return;
    }
    if file.flush().is_err() {
        return;
    }
    let _ = file.sync_all();
}

fn trace_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("MINUTES_HOME") {
        return Some(PathBuf::from(home).join("logs").join(TRACE_FILE));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|home| home.join(".minutes").join("logs").join(TRACE_FILE))
}

fn enabled() -> bool {
    !matches!(
        std::env::var("MINUTES_TRACE").ok().as_deref(),
        Some("0" | "false" | "FALSE" | "off" | "OFF")
    )
}

fn configured_model_path(config: &Config) -> Option<PathBuf> {
    let model = config.transcription.model.trim();
    if model.is_empty() {
        return None;
    }
    let direct = PathBuf::from(model);
    if direct.is_absolute() {
        Some(direct)
    } else {
        Some(
            config
                .transcription
                .model_path
                .join(format!("ggml-{model}.bin")),
        )
    }
}

fn content_type_label(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
        ContentType::Dictation => "dictation",
    }
}

#[cfg(test)]
pub fn trace_file_path_for_tests() -> Option<PathBuf> {
    trace_path()
}
