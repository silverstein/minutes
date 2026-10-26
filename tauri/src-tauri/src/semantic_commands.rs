//! Desktop wiring for local semantic search ("Search by meaning").
//!
//! One background thread keeps the index in step: when the setting is on it
//! installs the model if needed and runs an incremental sync now and every
//! few minutes (a no-op when nothing changed); when it is turned off it
//! stops the sync between meetings and deletes the index. Searches use the
//! process-wide warm worker, so they interleave with a running sync.
use minutes_core::config::Config;
use minutes_core::search::SearchFilters;
use minutes_core::{semantic, semantic_index, semantic_search};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

const RESYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const RESULT_LIMIT: usize = 12;
/// Cosine similarity below which a "related" meeting is noise. Calibrated on
/// Mat's corpus: meant meetings scored at or above it, unrelated queries'
/// best matches below it.
const SCORE_FLOOR: f32 = 0.0;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncProgress {
    /// "idle", "downloading", "indexing" or "error".
    phase: &'static str,
    done: usize,
    total: usize,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatus {
    enabled: bool,
    phase: &'static str,
    done: usize,
    total: usize,
    error: Option<String>,
    model_installed: bool,
    worker_available: bool,
    indexed_meetings: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticHit {
    path: String,
    title: String,
    date: String,
    content_type: String,
    snippet: String,
    score: f32,
}

static PROGRESS: Mutex<SyncProgress> = Mutex::new(SyncProgress {
    phase: "idle",
    done: 0,
    total: 0,
    error: None,
});
static ENABLED: AtomicBool = AtomicBool::new(false);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn set_progress(update: impl FnOnce(&mut SyncProgress)) {
    if let Ok(mut progress) = PROGRESS.lock() {
        update(&mut progress);
    }
}

fn wake() {
    if let Ok(mut flag) = WAKE.0.lock() {
        *flag = true;
        WAKE.1.notify_all();
    }
}

fn ensure_background() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        ENABLED.store(Config::load().search.semantic, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("semantic-sync".into())
            .spawn(background_loop)
            .ok();
    });
}

fn background_loop() {
    loop {
        if ENABLED.load(Ordering::SeqCst) {
            run_sync(&Config::load());
        } else if semantic_index::index_dir(&Config::minutes_dir()).exists() {
            let result = semantic_index::clear(&Config::minutes_dir());
            set_progress(|p| {
                *p = SyncProgress {
                    phase: if result.is_ok() { "idle" } else { "error" },
                    error: result.err().map(|e| e.to_string()),
                    ..SyncProgress::default()
                }
            });
        }
        let Ok(flag) = WAKE.0.lock() else { return };
        let Ok((mut flag, _)) = WAKE
            .1
            .wait_timeout_while(flag, RESYNC_EVERY, |woken| !*woken)
        else {
            return;
        };
        *flag = false;
    }
}

fn run_sync(config: &Config) {
    let model_root = &config.transcription.model_path;
    if !semantic::model_installed(model_root) {
        set_progress(|p| {
            *p = SyncProgress {
                phase: "downloading",
                ..SyncProgress::default()
            }
        });
        if let Err(error) = semantic::install_model(model_root) {
            set_progress(|p| {
                p.phase = "error";
                p.error = Some(error);
            });
            return;
        }
    }
    set_progress(|p| {
        *p = SyncProgress {
            phase: "indexing",
            ..SyncProgress::default()
        }
    });
    let mut embedder = semantic::SharedEmbedder::new(model_root);
    let result = semantic_search::sync(config, &mut embedder, |done, total| {
        set_progress(|p| {
            p.done = done;
            p.total = total;
        });
        ENABLED.load(Ordering::SeqCst)
    });
    set_progress(|p| match result {
        Ok(_) => {
            p.phase = "idle";
            p.error = None;
        }
        Err(error) => {
            p.phase = "error";
            p.error = Some(error.to_string());
        }
    });
}

#[tauri::command]
pub fn cmd_semantic_status() -> SemanticStatus {
    ensure_background();
    let config = Config::load();
    let progress = PROGRESS.lock().map(|p| p.clone()).unwrap_or_default();
    let indexed_meetings = if config.search.semantic {
        semantic_index::SemanticIndex::open(
            &Config::minutes_dir(),
            semantic_search::MODEL_ID,
            semantic::DIMENSIONS,
        )
        .and_then(|index| index.counts())
        .map(|(meetings, _)| meetings)
        .unwrap_or(0)
    } else {
        0
    };
    SemanticStatus {
        enabled: config.search.semantic,
        phase: progress.phase,
        done: progress.done,
        total: progress.total,
        error: progress.error,
        model_installed: semantic::model_installed(&config.transcription.model_path),
        worker_available: semantic::worker_path().is_some(),
        indexed_meetings,
    }
}

/// Turn semantic search on (install the model, index in the background) or
/// off (stop indexing and delete the index).
#[tauri::command]
pub fn cmd_semantic_set_enabled(enabled: bool) -> Result<SemanticStatus, String> {
    if enabled && semantic::worker_path().is_none() {
        return Err("This build of Minutes does not include the semantic search worker.".into());
    }
    let mut config = Config::load();
    config.search.semantic = enabled;
    config.save().map_err(|e| e.to_string())?;
    ensure_background();
    ENABLED.store(enabled, Ordering::SeqCst);
    wake();
    Ok(cmd_semantic_status())
}

/// Meetings related to `query` by meaning, above the noise floor. Empty when
/// semantic search is off or the index is not ready.
#[tauri::command]
pub async fn cmd_semantic_search(query: String) -> Result<Vec<SemanticHit>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = Config::load();
        if !config.search.semantic || !semantic::model_installed(&config.transcription.model_path) {
            return Ok(Vec::new());
        }
        let mut embedder = semantic::SharedEmbedder::new(&config.transcription.model_path);
        let hits = semantic_search::search(
            &config,
            &mut embedder,
            &query,
            &SearchFilters::default(),
            RESULT_LIMIT,
        )
        .map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .filter(|(_, score)| *score >= SCORE_FLOOR)
            .map(|(result, score)| SemanticHit {
                path: result.path.to_string_lossy().into_owned(),
                title: result.title,
                date: result.date,
                content_type: result.content_type,
                snippet: result.snippet,
                score,
            })
            .collect())
    })
    .await
    .map_err(|e| format!("semantic search worker failed: {e}"))?
}
