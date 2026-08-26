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

use crate::download::quality::Quality;
use crate::download::settings::Settings;
use crate::download::compat;
use crate::download::cookies::CookieFile;
use crate::download::session::{self, SessionKind, SessionStatus, WebSession};
use crate::download::slideshow;
use crate::download::url::{classify, classify_target, Source, TargetKind};

/// Which sessions apply to which sources. Only these two use a login.
fn session_kind_for(source: Source) -> Option<SessionKind> {
    match source {
        Source::Instagram => Some(SessionKind::Instagram),
        Source::Facebook => Some(SessionKind::Facebook),
        Source::TikTok => Some(SessionKind::TikTok),
        Source::X => Some(SessionKind::X),
        _ => None,
    }
}
use crate::download::ytdlp::{
    self, FormatReport, MediaInfo, Progress, ProfileListing, YOUTUBE_CLIENTS,
};
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

/// Retry policy for platform throttling.
///
/// These three numbers were measured, not guessed. Replaying a slice of a real
/// TikTok profile through this queue:
///
///   no retry, no stagger      5/8  succeeded
///   3 attempts, 3s/9s, 700ms  7/8  succeeded
///   4 attempts, 5s/15s/30s, 1.2s stagger   12/12 succeeded
///
/// Beyond four attempts the platform is saying no loudly enough that hammering
/// it further is both rude and useless.
const MAX_ATTEMPTS: u32 = 4;

/// Seconds to wait before each retry. Deliberately generous: the failure is a
/// rate limit, so retrying quickly is the one thing guaranteed not to help.
const RETRY_BACKOFF: &[u64] = &[5, 15, 30];

/// Minimum gap between starting one job's engine and the next.
///
/// Downloading a 133-video profile fired requests as fast as two workers could
/// manage, and TikTok answered roughly a third of them with an anti-bot page.
/// Spacing the starts removes most of those failures before any retry is
/// needed, at a cost of about two minutes spread across a 133-video queue.
const START_STAGGER_MS: u64 = 1200;

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
    /// True when the post had no video stream - a TikTok photo/slideshow, so
    /// the saved file is audio only. Surfaced so it reads as "this post has no
    /// video" rather than "the downloader dropped the video".
    pub audio_only: bool,
    /// True when a photo post was rebuilt into a playable video from its cover
    /// image. The distinction matters: the file *is* a video, but it shows one
    /// still frame, not the original slideshow.
    pub still_image_video: bool,
    /// Set when the file was re-encoded for playback, naming the codec it came
    /// from — so "why is this bigger/slower" has a visible answer.
    pub converted_from: Option<String>,
    /// Set once the file lands. Absolute path, for "Show in folder".
    pub output_path: Option<String>,
    /// 1-based, so the UI can say "Retrying 2 of 3" instead of going quiet
    /// during a backoff that looks like a hang.
    pub attempt: u32,
    pub max_attempts: u32,
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
    /// Set when this job was queued at a quality other than the global
    /// preference - a per-link choice made from the inspection panel.
    quality: Option<Quality>,
}

/// State of the tooling on this machine, for the UI's setup notice.
#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// FFmpeg is optional but decides YouTube quality: without it the best
    /// single file YouTube offers is 360p, because anything better is served
    /// as separate video and audio streams that need merging.
    pub has_ffmpeg: bool,
    pub ffmpeg_path: Option<String>,
    /// gallery-dl, needed only to list Instagram profiles. Single Instagram
    /// links work without it.
    pub has_lister: bool,
    pub lister_version: Option<String>,
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
    quality: Mutex<Quality>,
    prefer_compatible: Mutex<bool>,
    /// When each platform's session was captured, or absent. Mirrors the
    /// non-secret markers in `settings`, so the UI never reads a session file
    /// just to answer "connected?".
    session_markers: Mutex<HashMap<SessionKind, i64>>,
    /// Each platform's session, held after its first read from disk so a
    /// large batch does not re-read the file per job.
    session_cache: Mutex<HashMap<SessionKind, Arc<WebSession>>>,
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
            quality: Mutex::new(saved.quality),
            prefer_compatible: Mutex::new(saved.prefer_compatible),
            session_markers: Mutex::new({
                let mut m = HashMap::new();
                if let Some(at) = saved.instagram_connected_at {
                    m.insert(SessionKind::Instagram, at);
                }
                if let Some(at) = saved.facebook_connected_at {
                    m.insert(SessionKind::Facebook, at);
                }
                if let Some(at) = saved.x_connected_at {
                    m.insert(SessionKind::X, at);
                }
                if let Some(at) = saved.tiktok_connected_at {
                    m.insert(SessionKind::TikTok, at);
                }
                m
            }),
            session_cache: Mutex::new(HashMap::new()),
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

    fn marker(&self, kind: SessionKind) -> Option<i64> {
        self.session_markers.lock().expect("marker lock").get(&kind).copied()
    }

    /// Whether a platform is connected, answered without reading the session.
    /// Also carries the display profile (name/avatar) captured at login, which
    /// is non-secret metadata stored separately from the cookies.
    pub fn session_status(&self, kind: SessionKind) -> SessionStatus {
        let at = self.marker(kind);
        let profile = at
            .and(session::load_profile(&self.config_dir, kind))
            .unwrap_or_default();
        SessionStatus {
            connected: at.is_some(),
            captured_at: at,
            display_name: profile.display_name,
            avatar_url: profile.avatar_url,
        }
    }

    /// Store a captured session and record that one exists.
    pub fn session_remember(&self, kind: SessionKind, captured: &WebSession) -> Result<SessionStatus> {
        session::save(&self.config_dir, kind, captured)?;
        self.session_cache
            .lock()
            .expect("session cache lock")
            .insert(kind, Arc::new(captured.clone()));
        self.session_markers
            .lock()
            .expect("marker lock")
            .insert(kind, captured.captured_at);
        self.persist(self.saved_destination())?;
        Ok(self.session_status(kind))
    }

    /// Store the display profile captured at login (best-effort metadata).
    pub fn session_set_profile(
        &self,
        kind: SessionKind,
        profile: &session::SessionProfile,
    ) -> Result<()> {
        session::save_profile(&self.config_dir, kind, profile)
    }

    /// Forget a session. The marker is cleared even if the file delete fails,
    /// so the UI can never claim a connection the app cannot use.
    pub fn session_forget(&self, kind: SessionKind) -> Result<SessionStatus> {
        let cleared = session::clear(&self.config_dir, kind);
        session::clear_profile(&self.config_dir, kind);
        self.session_cache.lock().expect("session cache lock").remove(&kind);
        self.session_markers.lock().expect("marker lock").remove(&kind);
        self.persist(self.saved_destination())?;
        cleared?;
        Ok(self.session_status(kind))
    }

    // Thin per-platform wrappers, so command and frontend names stay stable.
    pub fn instagram_status(&self) -> SessionStatus { self.session_status(SessionKind::Instagram) }
    pub fn instagram_remember(&self, s: &WebSession) -> Result<SessionStatus> {
        self.session_remember(SessionKind::Instagram, s)
    }
    pub fn instagram_forget(&self) -> Result<SessionStatus> {
        self.session_forget(SessionKind::Instagram)
    }
    pub fn facebook_status(&self) -> SessionStatus { self.session_status(SessionKind::Facebook) }
    pub fn facebook_remember(&self, s: &WebSession) -> Result<SessionStatus> {
        self.session_remember(SessionKind::Facebook, s)
    }
    pub fn facebook_forget(&self) -> Result<SessionStatus> {
        self.session_forget(SessionKind::Facebook)
    }
    pub fn x_status(&self) -> SessionStatus { self.session_status(SessionKind::X) }
    pub fn x_remember(&self, s: &WebSession) -> Result<SessionStatus> {
        self.session_remember(SessionKind::X, s)
    }
    pub fn x_forget(&self) -> Result<SessionStatus> {
        self.session_forget(SessionKind::X)
    }

    /// The stored session for a platform, if one is saved and usable.
    ///
    /// Returns the cookies themselves, so callers can ask the platform whether
    /// they still work. Nothing derived from this may reach the frontend -
    /// `SessionStatus` is the type that crosses that boundary.
    pub fn session_for(&self, kind: SessionKind) -> Option<WebSession> {
        self.cached_session(kind).map(|s| (*s).clone())
    }

    pub fn prefer_compatible(&self) -> bool {
        *self.prefer_compatible.lock().expect("compat lock")
    }

    /// Applies to jobs started from now on; one already running keeps the
    /// format it negotiated.
    pub fn set_prefer_compatible(&self, on: bool) -> Result<bool> {
        *self.prefer_compatible.lock().expect("compat lock") = on;
        self.persist(self.saved_destination())?;
        Ok(on)
    }

    pub fn quality(&self) -> Quality {
        *self.quality.lock().expect("quality lock")
    }

    /// Change the quality preference. Applies to jobs started from now on;
    /// one already downloading keeps the format it negotiated.
    pub fn set_quality(&self, quality: Quality) -> Result<Quality> {
        *self.quality.lock().expect("quality lock") = quality;
        self.persist(self.saved_destination())?;
        Ok(quality)
    }

    /// The destination as it should be *stored* - `None` when it's the default,
    /// so a later change to the default is picked up rather than frozen.
    fn saved_destination(&self) -> Option<PathBuf> {
        let current = self.destination();
        (current != self.default_dir).then_some(current)
    }

    /// A preference that cannot be written is reported rather than swallowed:
    /// silently forgetting the choice on every restart is worse than an error.
    fn persist(&self, destination: Option<PathBuf>) -> Result<()> {
        Settings {
            destination,
            quality: self.quality(),
            instagram_connected_at: self.marker(SessionKind::Instagram),
            facebook_connected_at: self.marker(SessionKind::Facebook),
            x_connected_at: self.marker(SessionKind::X),
            tiktok_connected_at: self.marker(SessionKind::TikTok),
            prefer_compatible: self.prefer_compatible(),
        }
        .save(&self.config_dir)
        .map_err(|e| AppError::DownloadPath(format!("could not save your choice: {e}")))
    }

    pub async fn engine_status(&self) -> EngineStatus {
        let ffmpeg = ytdlp::locate_ffmpeg();
        let lister_version = crate::download::gallerydl::version().await;
        let has_lister = crate::download::gallerydl::locate().is_some();
        match ytdlp::locate() {
            None => EngineStatus {
                available: false,
                path: None,
                version: None,
                has_ffmpeg: ffmpeg.is_some(),
                ffmpeg_path: ffmpeg.map(|p| p.display().to_string()),
                has_lister,
                lister_version: lister_version.clone(),
            },
            Some(path) => EngineStatus {
                available: true,
                path: Some(path.display().to_string()),
                version: ytdlp::version().await.ok().filter(|v| !v.is_empty()),
                has_ffmpeg: ffmpeg.is_some(),
                ffmpeg_path: ffmpeg.map(|p| p.display().to_string()),
                has_lister,
                lister_version,
            },
        }
    }

    /// Check a link without committing to a download. Powers the paste preview.
    pub async fn inspect(&self, raw: &str) -> Result<MediaInfo> {
        let (source, url) = classify(raw)?;
        let jar = self.cookie_jar(source);
        ytdlp::probe(&url, None, jar.as_ref().map(|j| j.path())).await
    }

    /// Read a link's metadata and the quality tiers it actually offers.
    pub async fn inspect_formats(&self, raw: &str) -> Result<FormatReport> {
        let (source, url, kind) = classify_target(raw)?;
        if kind != TargetKind::Single {
            return Err(AppError::UnsupportedUrl);
        }
        let clients: &[Option<&str>] = match source {
            Source::YouTube => YOUTUBE_CLIENTS,
            _ => &[None],
        };
        let jar = self.cookie_jar(source);
        ytdlp::inspect_formats(&url, clients, jar.as_ref().map(|j| j.path())).await
    }

    /// A cookie jar for this source, or `None`.
    ///
    /// Only Instagram and Facebook ever get one, and only when the user has
    /// signed in through the app's own login window. Every other source
    /// downloads with no session at all.
    fn cookie_jar(&self, source: Source) -> Option<CookieFile> {
        let kind = session_kind_for(source)?;
        let stored = self.cached_session(kind)?;
        if !stored.is_usable_for(kind) {
            return None;
        }
        CookieFile::write(&stored.cookies).ok()
    }

    /// A platform's session, from memory when possible. The first call per run
    /// reads the file; the marker gate means an unconnected platform never
    /// touches disk.
    fn cached_session(&self, kind: SessionKind) -> Option<Arc<WebSession>> {
        if let Some(cached) = self.session_cache.lock().expect("session cache lock").get(&kind).cloned() {
            return Some(cached);
        }
        // Nothing recorded means nothing to read.
        self.marker(kind)?;

        let loaded = Arc::new(session::load(&self.config_dir, kind).ok().flatten()?);
        self.session_cache
            .lock()
            .expect("session cache lock")
            .insert(kind, loaded.clone());
        Some(loaded)
    }

    /// The quality a job should download at: its own choice, or the default.
    fn quality_for(&self, id: &str) -> Quality {
        self.jobs
            .lock()
            .expect("jobs lock")
            .get(id)
            .and_then(|j| j.quality)
            .unwrap_or_else(|| self.quality())
    }

    /// List a creator's videos without downloading any of them.
    pub async fn inspect_profile(&self, raw: &str) -> Result<ProfileListing> {
        let (source, url, kind) = classify_target(raw)?;
        if kind != TargetKind::Profile {
            return Err(AppError::UnsupportedUrl);
        }

        // Instagram is listed by gallery-dl; yt-dlp's own extractor for it is
        // marked CURRENTLY BROKEN upstream. Every *download* still goes
        // through yt-dlp, so only the enumeration differs.
        if source == Source::Instagram {
            let jar = self.cookie_jar(source);
            return crate::download::gallerydl::list_instagram_profile(
                &url,
                jar.as_ref().map(|j| j.path()),
            )
            .await;
        }

        // X has no yt-dlp timeline extractor, so — like Instagram — gallery-dl
        // enumerates the profile (using the captured session cookies) and
        // yt-dlp downloads each resulting tweet.
        if source == Source::X {
            let jar = self.cookie_jar(source);
            return crate::download::gallerydl::list_x_profile(
                &url,
                jar.as_ref().map(|j| j.path()),
            )
            .await;
        }

        // A channel home page is not one feed but three - Videos, Shorts and
        // past streams - so it is listed as all of them at once. An explicit
        // tab link expands to nothing here and is listed as itself.
        if let Some(feeds) = crate::download::url::youtube_channel_feeds(&url) {
            return ytdlp::list_channel(&feeds, &url, None).await;
        }

        ytdlp::list_profile(&url, None).await
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
    pub fn enqueue(&self, app: &AppHandle, raw: &str, quality: Option<Quality>) -> Result<JobView> {
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
            audio_only: false,
            still_image_video: false,
            converted_from: None,
            output_path: None,
            attempt: 1,
            max_attempts: MAX_ATTEMPTS,
            error_code: None,
            error_message: None,
            created_at: crate::auth::now_unix(),
        };

        {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            jobs.insert(
                id.clone(),
                Job {
                    view: view.clone(),
                    quality,
                },
            );
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

    /// Queue a batch of already-validated links.
    ///
    /// Returns only the jobs that were created: a single bad entry in a
    /// 133-video profile must not discard the other 132.
    pub fn enqueue_all(
        &self,
        app: &AppHandle,
        urls: &[String],
        quality: Option<Quality>,
    ) -> (Vec<JobView>, usize) {
        let mut queued = Vec::with_capacity(urls.len());
        let mut failed = 0usize;
        for url in urls {
            match self.enqueue(app, url, quality) {
                Ok(v) => queued.push(v),
                Err(_) => failed += 1,
            }
        }
        (queued, failed)
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
    let (raw_url, source) = match manager.get(&id) {
        Ok(v) => (v.url, v.source),
        Err(_) => return,
    };
    let url = match classify(&raw_url) {
        Ok((_, u)) => u,
        Err(e) => return finish_failed(&manager, &app, &id, e),
    };

    // Queued until a slot frees. Bound to a name so it lives until this
    // function returns: the permit must cover every retry, or a backing-off
    // job would free its slot and let a third download start alongside.
    let _permit = match manager.slots.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => return finish_failed(&manager, &app, &id, AppError::Internal("shutting down".into())),
    };
    if manager.is_cancelled(&id) {
        return;
    }

    // Space out engine starts so a large queue doesn't look like a scraper.
    tokio::time::sleep(std::time::Duration::from_millis(START_STAGGER_MS)).await;

    // Only YouTube has alternative player clients; everything else has one way
    // of asking, so a refusal there is simply a failure.
    let clients: &[Option<&str>] = match source {
        Source::YouTube => YOUTUBE_CLIENTS,
        _ => &[None],
    };
    let mut client_idx = 0usize;

    // Held for the whole job so retries reuse it; deleted when this returns.
    let jar = manager.cookie_jar(source);
    let cookies = jar.as_ref().map(|j| j.path());

    for attempt in 1..=MAX_ATTEMPTS {
        if manager.is_cancelled(&id) {
            return;
        }
        match attempt_job(&manager, &app, &id, &url, attempt, clients[client_idx], cookies).await {
            Outcome::Done | Outcome::Cancelled => return,
            Outcome::Failed(e) => return finish_failed(&manager, &app, &id, e),
            Outcome::Refused(e) => {
                // Waiting won't help - ask again as a different client. When
                // there isn't one left, report it rather than spinning.
                client_idx += 1;
                if client_idx >= clients.len() {
                    return finish_failed(&manager, &app, &id, e);
                }
                if let Some(v) = manager.mutate(&id, |v| {
                    v.status = JobStatus::Queued;
                    v.attempt = attempt + 1;
                }) {
                    let _ = app.emit(events::UPDATED, v);
                }
            }
            Outcome::Throttled(e) => {
                // Out of attempts: report the throttling honestly rather than
                // as some other kind of failure.
                let Some(wait) = RETRY_BACKOFF.get((attempt - 1) as usize) else {
                    return finish_failed(&manager, &app, &id, e);
                };
                if let Some(v) = manager.mutate(&id, |v| {
                    v.status = JobStatus::Queued;
                    v.attempt = attempt + 1;
                    v.speed_bps = None;
                    v.eta_seconds = None;
                }) {
                    let _ = app.emit(events::UPDATED, v);
                }
                tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
            }
        }
    }
}

/// What one attempt concluded.
enum Outcome {
    Done,
    Cancelled,
    /// Worth another attempt - the platform throttled us.
    Throttled(AppError),
    /// Worth another attempt, but only as a different player client.
    Refused(AppError),
    /// Not worth retrying: private, missing, or genuinely broken.
    Failed(AppError),
}

impl Outcome {
    fn from(e: AppError) -> Self {
        match e {
            AppError::TemporarilyUnavailable => Outcome::Throttled(e),
            AppError::ClientRefused => Outcome::Refused(e),
            other => Outcome::Failed(other),
        }
    }
}

/// One probe-and-download pass. Called up to [`MAX_ATTEMPTS`] times.
async fn attempt_job(
    manager: &Arc<DownloadManager>,
    app: &AppHandle,
    id: &str,
    url: &url::Url,
    attempt: u32,
    client: Option<&str>,
    cookies: Option<&std::path::Path>,
) -> Outcome {
    if let Some(v) = manager.mutate(id, |v| {
        v.status = JobStatus::Probing;
        v.attempt = attempt;
    }) {
        let _ = app.emit(events::UPDATED, v);
    }

    let info = match ytdlp::probe(url, client, cookies).await {
        Ok(i) => i,
        Err(e) => return Outcome::from(e),
    };
    if manager.is_cancelled(id) {
        return Outcome::Cancelled;
    }

    if let Some(v) = manager.mutate(id, |v| {
        v.title = Some(info.title.clone());
        v.uploader = info.uploader.clone();
        v.duration_seconds = info.duration_seconds;
        v.thumbnail_url = info.thumbnail_url.clone();
        v.total_bytes = info.estimated_bytes;
        v.audio_only = !info.has_video;
        v.status = JobStatus::Downloading;
    }) {
        let _ = app.emit(events::UPDATED, v);
    }

    let dest = manager.destination();
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return Outcome::Failed(AppError::DownloadPath(e.to_string()));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Progress>();
    let mut running = match ytdlp::start(
        url,
        &dest,
        tx,
        client,
        manager.quality_for(id),
        cookies,
        manager.prefer_compatible(),
    ) {
        Ok(r) => r,
        Err(e) => return Outcome::from(e),
    };

    // Relay progress to the UI while the engine works.
    let relay = {
        let manager = manager.clone();
        let app = app.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            // A merged download is two downloads: yt-dlp reports the video
            // stream to completion, then restarts the counter at zero for the
            // audio stream. Reported verbatim, the bar would jump backwards
            // and the finished size would be the *audio* track alone - 1.1 MB
            // for an 8 MB file. So finished streams are accumulated into a
            // base that later streams are added to.
            let mut base = 0u64;
            let mut last = 0u64;

            while let Some(p) = rx.recv().await {
                if p.downloaded_bytes < last {
                    base += last;
                }
                last = p.downloaded_bytes;

                let downloaded = base + p.downloaded_bytes;
                // The total only covers streams seen so far, so it grows as
                // each new one starts. Monotonic progress matters more here
                // than a total that is exact before the end.
                let total = p.total_bytes.map(|t| base + t);
                let fraction = total
                    .filter(|t| *t > 0)
                    .map(|t| (downloaded as f64 / t as f64).clamp(0.0, 1.0));

                let changed = manager.mutate(&id, |v| {
                    v.downloaded_bytes = downloaded;
                    if total.is_some() {
                        v.total_bytes = total;
                    }
                    v.speed_bps = p.speed_bps;
                    v.eta_seconds = p.eta_seconds;
                    v.fraction = fraction;
                });
                if changed.is_none() {
                    break; // cancelled or removed
                }
                let _ = app.emit(
                    events::PROGRESS,
                    ProgressEvent {
                        id: id.clone(),
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        speed_bps: p.speed_bps,
                        eta_seconds: p.eta_seconds,
                        fraction,
                    },
                );
            }
        })
    };

    let outcome = tokio::select! {
        res = ytdlp::wait(&mut running) => res,
        _ = wait_for_cancel(manager, id) => {
            running.kill().await;
            relay.abort();
            return Outcome::Cancelled;
        }
    };

    relay.abort();

    match outcome {
        Ok(()) => {
            // The engine's own report, with the folder scan as a fallback for
            // an older yt-dlp that doesn't print it.
            let path = running.output_path().or_else(|| newest_file_in(&dest));
            // The file on disk is the only honest answer for "how big is it".
            // Summed stream counters miss container overhead, and a remux
            // changes the size again - so measure rather than infer.
            // A photo post lands as bare audio. If FFmpeg is available, rebuild
            // it into something playable rather than leaving an mp3 sitting in
            // a folder of videos. Failure here is not job failure: the audio is
            // still a successful download.
            let mut still_image = false;
            let mut path = path;
            if !info.has_video {
                if let (Some(audio), Some(cover), Some(ffmpeg)) = (
                    path.as_deref(),
                    info.thumbnail_url.as_deref(),
                    ytdlp::locate_ffmpeg(),
                ) {
                    // Metadata said "no video"; the file is the authority.
                    // Converting a real video would encode a still image over
                    // its soundtrack and delete the original - so this second
                    // check is what makes the feature safe rather than clever.
                    let audio_path = std::path::Path::new(audio);
                    let really_audio_only =
                        !slideshow::file_has_video(&ffmpeg, audio_path).await;

                    match if really_audio_only {
                        slideshow::build_still_video(cover, audio_path, &ffmpeg).await
                    } else {
                        Err(AppError::Internal(
                            "metadata reported no video but the file has a video stream; left as downloaded".into(),
                        ))
                    } {
                        Ok(built) => {
                            still_image = true;
                            path = Some(built.display().to_string());
                        }
                        Err(e) => {
                            log_conversion_failure(&e);
                        }
                    }
                }
            }

            // Selection prefers H.264, but a platform that offers nothing else
            // still lands a VP9 or AV1 file that QuickTime refuses to open.
            // This is the backstop that makes "playable" actually true.
            let mut converted_from = None;
            if manager.prefer_compatible() {
                if let (Some(file), Some(ffmpeg)) = (path.as_deref(), ytdlp::locate_ffmpeg()) {
                    match compat::ensure_playable(&ffmpeg, std::path::Path::new(file)).await {
                        Ok(compat::Outcome::Converted { from }) => converted_from = Some(from),
                        Ok(compat::Outcome::Skipped(why)) => eprintln!("compatibility: {why}"),
                        Ok(compat::Outcome::Untouched) => {}
                        // The download itself succeeded; keep it.
                        Err(e) => eprintln!("compatibility pass failed, keeping original: {e}"),
                    }
                }
            }

            let actual = path
                .as_deref()
                .and_then(|p| std::fs::metadata(p).ok())
                .filter(|m| m.is_file())
                .map(|m| m.len());

            if let Some(v) = manager.mutate(id, |v| {
                v.still_image_video = still_image;
                v.converted_from = converted_from.clone();
                v.status = JobStatus::Completed;
                v.speed_bps = None;
                v.eta_seconds = None;
                v.fraction = Some(1.0);
                match actual {
                    Some(size) => {
                        v.downloaded_bytes = size;
                        v.total_bytes = Some(size);
                    }
                    // No path reported: fall back to the counters we have.
                    None => {
                        if let Some(t) = v.total_bytes {
                            v.downloaded_bytes = v.downloaded_bytes.max(t);
                        }
                    }
                }
                v.output_path = path.clone();
            }) {
                let _ = app.emit(events::FINISHED, v);
            }
            Outcome::Done
        }
        Err(e) => Outcome::from(e),
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

/// A failed photo-post conversion is worth a line in the log and nothing more:
/// the download itself succeeded, and the audio is still there.
fn log_conversion_failure(e: &AppError) {
    eprintln!("photo post kept as audio: {e}");
}

/// Fallback for when the engine didn't report its output path: the most
/// recently written media file in the destination.
///
/// Approximate by nature - with two downloads finishing together it can name
/// the wrong one - which is why [`Running::output_path`] is preferred.
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
        let blocker = std::env::temp_dir().join(format!(
            "md-manager-blocker-{}",
            std::process::id()
        ));
        std::fs::write(&blocker, b"not a directory").unwrap();

        let err = m.set_destination(blocker.join("child")).unwrap_err();
        assert!(matches!(err, AppError::DownloadPath(_)), "{err}");

        let _ = std::fs::remove_file(&blocker);
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
