//! Job registry: owns every download's lifecycle and is the only thing that
//! emits `download://*` events.
//!
//! Mirrors the shape of [`crate::auth::manager::AuthManager`] deliberately -
//! the frontend already knows this pattern: commands return the current view
//! synchronously, and events keep it fresh afterwards.
//!
//! Layering: this module knows about yt-dlp and URLs. It does not know about
//! accounts, and holds no reference to the credential store. Public media is
//! fetched anonymously or not at all.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Semaphore};

use crate::download::settings::Settings;
use crate::download::url::{classify, Source};
use crate::download::ytdlp::{self, MediaInfo, Progress};
use crate::errors::{AppError, Result};

pub mod events {
    pub const CREATED: &str = "download://created";
    pub const UPDATED: &str = "download://updated";
    pub const PROGRESS: &str = "download://progress";
    pub const FINISHED: &str = "download://finished";
    pub const FAILED: &str = "download://failed";
}

/// How many downloads run at once. Two platforms, modest files, and a shared
/// uplink: more parallelism mostly makes every job slower and looks like
/// scraping from the other end.
const MAX_CONCURRENT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Waiting for a concurrency slot.
    Queued,
    /// Reading metadata, no bytes yet.
    Probing,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

/// Everything the UI renders for one job. Contains no credential and no
/// signed CDN URL - only what a person needs to recognise their own download.
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: String,
    pub source: Source,
    /// The link as pasted, so a failed job can be retried or copied.
    pub url: String,
    pub status: JobStatus,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub fraction: Option<f64>,
    /// Set once the file lands. Absolute path, for "Show in folder".
    pub output_path: Option<String>,
    /// Present only in `Failed`; a user-facing sentence, never raw stderr.
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
}

/// Per-job progress payload. Split from `JobView` so a fast-ticking event
/// doesn't re-send static metadata sixty times a second.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub fraction: Option<f64>,
}

struct Job {
    view: JobView,
}

/// State of the engine on this machine, for the UI's setup notice.
#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

/// The destination as the UI needs to describe it.
#[derive(Debug, Clone, Serialize)]
pub struct Destination {
    pub path: String,
    /// False when this is the built-in default, which is what the "Reset"
    /// affordance keys off - there is nothing to reset to otherwise.
    pub is_custom: bool,
    /// Shown so a person can tell "I chose this" from "it fell back".
    pub default_path: String,
}

pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Job>>,
    /// Insertion order, newest last - `HashMap` alone cannot render a list.
    order: Mutex<Vec<String>>,
    dest_dir: Mutex<PathBuf>,
    /// Where the app would save with no preference set.
    default_dir: PathBuf,
    /// Where `downloader-settings.json` lives.
    config_dir: PathBuf,
    slots: Arc<Semaphore>,
}

impl DownloadManager {
    /// `default_dir` is the built-in location; `config_dir` is the app data
    /// directory the chosen folder is remembered in. The saved preference wins
    /// when it still points at a real folder.
    pub fn new(default_dir: PathBuf, config_dir: PathBuf) -> Self {
        let saved = Settings::load(&config_dir);
        let active = crate::download::settings::resolve_destination(&saved, default_dir.clone());
        Self {
            jobs: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            dest_dir: Mutex::new(active),
            default_dir,
            config_dir,
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }

    pub fn destination(&self) -> PathBuf {
        self.dest_dir.lock().expect("dest lock").clone()
    }

    /// The destination plus the context the UI needs to render its controls.
    pub fn destination_view(&self) -> Destination {
        let path = self.destination();
        Destination {
            is_custom: path != self.default_dir,
            path: path.display().to_string(),
            default_path: self.default_dir.display().to_string(),
        }
    }

    /// Adopt a folder and remember it across restarts.
    ///
    /// The folder is *created* if it doesn't exist, because a person picking
    /// "Videos/Reels" in a dialog has usually just made it, and refusing on a
    /// technicality would be baffling. Writability is then proved immediately
    /// rather than at the end of a long download.
    pub fn set_destination(&self, dir: PathBuf) -> Result<Destination> {
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir).map_err(|e| {
                AppError::DownloadPath(format!("{} could not be created: {e}", dir.display()))
            })?;
        }

        let probe = dir.join(".media-downloader-write-test");
        std::fs::write(&probe, b"").map_err(|e| {
            AppError::DownloadPath(format!("{} is not writable: {e}", dir.display()))
        })?;
        let _ = std::fs::remove_file(&probe);

        *self.dest_dir.lock().expect("dest lock") = dir.clone();
        self.persist(Some(dir))?;
        Ok(self.destination_view())
    }

    /// Forget the custom folder and go back to the built-in default.
    pub fn reset_destination(&self) -> Result<Destination> {
        *self.dest_dir.lock().expect("dest lock") = self.default_dir.clone();
        self.persist(None)?;
        Ok(self.destination_view())
    }

    /// A preference that cannot be written is reported rather than swallowed:
    /// silently forgetting the choice on every restart is worse than an error.
    fn persist(&self, destination: Option<PathBuf>) -> Result<()> {
        Settings { destination }
            .save(&self.config_dir)
            .map_err(|e| AppError::DownloadPath(format!("could not save your choice: {e}")))
    }

    pub async fn engine_status(&self) -> EngineStatus {
        match ytdlp::locate() {
            None => EngineStatus {
                available: false,
                path: None,
                version: None,
            },
            Some(path) => EngineStatus {
                available: true,
                path: Some(path.display().to_string()),
                version: ytdlp::version().await.ok().filter(|v| !v.is_empty()),
            },
        }
    }

    /// Check a link without committing to a download. Powers the paste preview.
    pub async fn inspect(&self, raw: &str) -> Result<MediaInfo> {
        let (_, url) = classify(raw)?;
        ytdlp::probe(&url).await
    }

    pub fn list(&self) -> Vec<JobView> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let order = self.order.lock().expect("order lock");
        order
            .iter()
            .rev() // newest first, which is what the page shows
            .filter_map(|id| jobs.get(id).map(|j| j.view.clone()))
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<JobView> {
        self.jobs
            .lock()
            .expect("jobs lock")
            .get(id)
            .map(|j| j.view.clone())
            .ok_or(AppError::JobNotFound)
    }

    /// Validate a link and queue it. Returns as soon as the job exists, so the
    /// UI can render a row immediately; the work happens on a spawned task.
    pub fn enqueue(&self, app: &AppHandle, raw: &str) -> Result<JobView> {
        let (source, url) = classify(raw)?;

        // Fail fast rather than queueing something that cannot possibly run.
        if ytdlp::locate().is_none() {
            return Err(AppError::EngineMissing);
        }

        let id = uuid::Uuid::new_v4().to_string();
        let view = JobView {
            id: id.clone(),
            source,
            url: url.to_string(),
            status: JobStatus::Queued,
            title: None,
            uploader: None,
            duration_seconds: None,
            thumbnail_url: None,
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bps: None,
            eta_seconds: None,
            fraction: None,
            output_path: None,
            error_code: None,
            error_message: None,
            created_at: crate::auth::now_unix(),
        };

        {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            jobs.insert(id.clone(), Job { view: view.clone() });
            self.order.lock().expect("order lock").push(id.clone());
        }
        let _ = app.emit(events::CREATED, view.clone());

        Ok(view)
    }

    /// Mark a job cancelled and stop its engine process.
    ///
    /// Idempotent: cancelling an already-finished job is a no-op rather than an
    /// error, because the user's click and the job's completion race freely.
    pub fn cancel(&self, app: &AppHandle, id: &str) -> Result<JobView> {
        let view = {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            let job = jobs.get_mut(id).ok_or(AppError::JobNotFound)?;
            if job.view.status.is_terminal() {
                return Ok(job.view.clone());
            }
            // The status *is* the signal: `run_job` polls it and kills the
            // engine. A separate channel would add a second source of truth
            // that could disagree with what the UI has already been told.
            job.view.status = JobStatus::Cancelled;
            job.view.speed_bps = None;
            job.view.eta_seconds = None;
            job.view.clone()
        };
        let _ = app.emit(events::UPDATED, view.clone());
        Ok(view)
    }

    /// Drop a terminal job from the list. Running jobs must be cancelled first.
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        match jobs.get(id) {
            None => return Err(AppError::JobNotFound),
            Some(j) if !j.view.status.is_terminal() => {
                return Err(AppError::Internal(
                    "cancel this download before removing it".into(),
                ))
            }
            Some(_) => {}
        }
        jobs.remove(id);
        self.order.lock().expect("order lock").retain(|x| x != id);
        Ok(())
    }

    pub fn clear_finished(&self) -> usize {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let mut order = self.order.lock().expect("order lock");
        let doomed: Vec<String> = order
            .iter()
            .filter(|id| jobs.get(*id).is_some_and(|j| j.view.status.is_terminal()))
            .cloned()
            .collect();
        for id in &doomed {
            jobs.remove(id);
        }
        order.retain(|id| !doomed.contains(id));
        doomed.len()
    }

    // ------------------------------------------------------------- internals

    fn mutate<F: FnOnce(&mut JobView)>(&self, id: &str, f: F) -> Option<JobView> {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs.get_mut(id)?;
        // A cancelled job must never be dragged back into a running state by a
        // late update from its own task.
        if job.view.status == JobStatus::Cancelled {
            return None;
        }
        f(&mut job.view);
        Some(job.view.clone())
    }

    fn is_cancelled(&self, id: &str) -> bool {
        self.jobs
            .lock()
            .expect("jobs lock")
            .get(id)
            .map(|j| j.view.status == JobStatus::Cancelled)
            .unwrap_or(true)
    }
}

/// Drive one job to completion. Spawned by the command layer, which owns the
/// `Arc` this needs.
pub async fn run_job(manager: Arc<DownloadManager>, app: AppHandle, id: String) {
    let (raw_url, _source) = match manager.get(&id) {
        Ok(v) => (v.url, v.source),
        Err(_) => return,
    };
    let url = match classify(&raw_url) {
        Ok((_, u)) => u,
        Err(e) => return finish_failed(&manager, &app, &id, e),
    };

    // Queued until a slot frees. Holding the permit for the whole job is what
    // bounds concurrency.
    let permit = match manager.slots.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => return finish_failed(&manager, &app, &id, AppError::Internal("shutting down".into())),
    };
    if manager.is_cancelled(&id) {
        return;
    }

    if let Some(v) = manager.mutate(&id, |v| v.status = JobStatus::Probing) {
        let _ = app.emit(events::UPDATED, v);
    }

    let info = match ytdlp::probe(&url).await {
        Ok(i) => i,
        Err(e) => return finish_failed(&manager, &app, &id, e),
    };
    if manager.is_cancelled(&id) {
        return;
    }

    if let Some(v) = manager.mutate(&id, |v| {
        v.title = Some(info.title.clone());
        v.uploader = info.uploader.clone();
        v.duration_seconds = info.duration_seconds;
        v.thumbnail_url = info.thumbnail_url.clone();
        v.total_bytes = info.estimated_bytes;
        v.status = JobStatus::Downloading;
    }) {
        let _ = app.emit(events::UPDATED, v);
    }

    let dest = manager.destination();
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return finish_failed(&manager, &app, &id, AppError::DownloadPath(e.to_string()));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Progress>();
    let mut running = match ytdlp::start(&url, &dest, tx) {
        Ok(r) => r,
        Err(e) => return finish_failed(&manager, &app, &id, e),
    };

    // Relay progress to the UI while the engine works.
    let relay = {
        let manager = manager.clone();
        let app = app.clone();
        let id = id.clone();
        tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                let changed = manager.mutate(&id, |v| {
                    v.downloaded_bytes = p.downloaded_bytes;
                    if p.total_bytes.is_some() {
                        v.total_bytes = p.total_bytes;
                    }
                    v.speed_bps = p.speed_bps;
                    v.eta_seconds = p.eta_seconds;
                    v.fraction = p.fraction;
                });
                if changed.is_none() {
                    break; // cancelled or removed
                }
                let _ = app.emit(
                    events::PROGRESS,
                    ProgressEvent {
                        id: id.clone(),
                        downloaded_bytes: p.downloaded_bytes,
                        total_bytes: p.total_bytes,
                        speed_bps: p.speed_bps,
                        eta_seconds: p.eta_seconds,
                        fraction: p.fraction,
                    },
                );
            }
        })
    };

    let outcome = tokio::select! {
        res = ytdlp::wait(&mut running) => res,
        _ = wait_for_cancel(&manager, &id) => {
            running.kill().await;
            relay.abort();
            drop(permit);
            return;
        }
    };

    relay.abort();
    drop(permit);

    match outcome {
        Ok(()) => {
            let path = newest_file_in(&dest);
            if let Some(v) = manager.mutate(&id, |v| {
                v.status = JobStatus::Completed;
                v.speed_bps = None;
                v.eta_seconds = None;
                v.fraction = Some(1.0);
                if let Some(t) = v.total_bytes {
                    v.downloaded_bytes = v.downloaded_bytes.max(t);
                }
                v.output_path = path.clone();
            }) {
                let _ = app.emit(events::FINISHED, v);
            }
        }
        Err(e) => finish_failed(&manager, &app, &id, e),
    }
}

/// Resolves once [`DownloadManager::cancel`] has flipped the job's status.
///
/// A quarter-second poll rather than a channel: cancellation is a human action
/// on a network-bound job, so the latency is invisible, and this keeps the
/// job's status as the single source of truth.
async fn wait_for_cancel(manager: &Arc<DownloadManager>, id: &str) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if manager.is_cancelled(id) {
            return;
        }
    }
}

fn finish_failed(manager: &Arc<DownloadManager>, app: &AppHandle, id: &str, e: AppError) {
    let code = e.code().to_string();
    let message = e.to_string();
    if let Some(v) = manager.mutate(id, |v| {
        v.status = JobStatus::Failed;
        v.speed_bps = None;
        v.eta_seconds = None;
        v.error_code = Some(code.clone());
        v.error_message = Some(message.clone());
    }) {
        let _ = app.emit(events::FAILED, v);
    }
}

/// yt-dlp's final path is not reported on stdout in a form we parse, so the
/// most recently written media file in the destination is used instead.
fn newest_file_in(dir: &std::path::Path) -> Option<String> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        // Skip `.part` files, which are the in-progress form of another job.
        if path.extension().is_some_and(|e| e == "part") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else { continue };
        if best.as_ref().map_or(true, |(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> DownloadManager {
        let cfg = std::env::temp_dir().join("md-manager-test-config");
        let _ = std::fs::remove_dir_all(&cfg);
        DownloadManager::new(std::env::temp_dir().join("md-default-dl"), cfg)
    }

    #[test]
    fn terminal_states_are_the_removable_ones() {
        assert!(JobStatus::Completed.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Cancelled.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Downloading.is_terminal());
    }

    #[test]
    fn unknown_job_ids_are_rejected_everywhere() {
        let m = manager();
        assert!(matches!(m.get("nope"), Err(AppError::JobNotFound)));
        assert!(matches!(m.remove("nope"), Err(AppError::JobNotFound)));
    }

    #[test]
    fn an_uncreatable_destination_is_refused_before_any_download() {
        let m = manager();
        // Root is not writable, so neither the folder nor the probe can be made.
        let err = m.set_destination(PathBuf::from("/definitely/not/here")).unwrap_err();
        assert!(matches!(err, AppError::DownloadPath(_)), "{err}");
    }

    #[test]
    fn choosing_a_folder_survives_a_restart() {
        let cfg = std::env::temp_dir().join("md-persist-test-config");
        let _ = std::fs::remove_dir_all(&cfg);
        let default = std::env::temp_dir().join("md-persist-default");
        let chosen = std::env::temp_dir().join("md-persist-chosen");
        std::fs::create_dir_all(&chosen).unwrap();

        let first = DownloadManager::new(default.clone(), cfg.clone());
        assert!(!first.destination_view().is_custom);
        first.set_destination(chosen.clone()).unwrap();

        // A fresh manager is what a relaunch produces.
        let restarted = DownloadManager::new(default.clone(), cfg.clone());
        assert_eq!(restarted.destination(), chosen);
        assert!(restarted.destination_view().is_custom);

        restarted.reset_destination().unwrap();
        let again = DownloadManager::new(default.clone(), cfg.clone());
        assert_eq!(again.destination(), default);
        assert!(!again.destination_view().is_custom);

        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&chosen);
    }
}
