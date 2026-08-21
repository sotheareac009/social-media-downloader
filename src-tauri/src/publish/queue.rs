//! The publishing queue: one job per (video, account) pair, run with a bounded
//! number of workers.
//!
//! Shape follows [`crate::download::manager::DownloadManager`] — commands
//! return the current view synchronously, events keep it fresh — with one
//! deliberate difference: jobs are persisted. A download that vanishes when the
//! app closes is an inconvenience; a publish that vanishes leaves you unable to
//! answer "did that go out or not?", so every transition is written to SQLite
//! and interrupted jobs are failed honestly at the next startup.
//!
//! LAYERING. The queue orchestrates. It knows the *shape* of publishing —
//! wake the device, copy the file, index it, hand it to the app — and nothing
//! about any particular app. The last step is delegated to a connector, and
//! that delegation is the only place platform knowledge enters.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;

use crate::errors::{AppError, Result};
use crate::ldplayer::manager::{now_unix, LdPlayerManager};
use crate::publish::connector::{self, Outcome, PublishContext};
use crate::publish::model::{
    Account, AccountStatus, AccountView, JobStatus, MediaItem, Platform, PublishJob,
};
use crate::publish::store::{JobRow, PublishStore};

pub mod events {
    /// A job was created and is now in the queue.
    pub const CREATED: &str = "publish://created";
    /// A job changed — status, progress or step. The workhorse event.
    pub const UPDATED: &str = "publish://updated";
    /// A job reached a terminal state, so the UI can raise a toast once.
    pub const FINISHED: &str = "publish://finished";
}

/// How many jobs a `list` call returns. More than anyone scrolls, small enough
/// that the query stays instant on a year-old database.
const JOB_PAGE: usize = 200;

/// Summary counts for the dashboard.
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueSummary {
    pub pending: i64,
    pub active: i64,
    pub published: i64,
    pub needs_attention: i64,
    pub failed: i64,
}

pub struct PublishQueue {
    store: Arc<PublishStore>,
    devices: Arc<LdPlayerManager>,
    /// Worker slots. Replaced (not resized — a semaphore cannot shrink) when
    /// the concurrency setting changes; jobs already holding a permit finish
    /// against the old one, which is correct: they are mid-publish.
    slots: Mutex<(usize, Arc<Semaphore>)>,
    /// Jobs the user asked to cancel. Checked at every step boundary rather
    /// than mid-step: killing an `adb push` halfway leaves a truncated file on
    /// the device, and killing a connector mid-composer leaves the app in a
    /// state the user did not choose.
    cancelled: Mutex<HashSet<String>>,
}

impl PublishQueue {
    pub fn new(store: Arc<PublishStore>, devices: Arc<LdPlayerManager>) -> Self {
        let max = devices.settings().max_concurrent;
        Self {
            store,
            devices,
            slots: Mutex::new((max, Arc::new(Semaphore::new(max)))),
            cancelled: Mutex::new(HashSet::new()),
        }
    }

    pub fn store(&self) -> &Arc<PublishStore> {
        &self.store
    }

    /// Current worker pool, rebuilt if the setting changed since last time.
    fn permits(&self) -> Arc<Semaphore> {
        let want = self.devices.settings().max_concurrent;
        let mut guard = self.slots.lock().expect("slots lock");
        if guard.0 != want {
            *guard = (want, Arc::new(Semaphore::new(want)));
        }
        guard.1.clone()
    }

    // ----------------------------------------------------------- accounts

    /// Accounts joined with live device state.
    ///
    /// Status is computed on every call rather than stored: a saved
    /// "Connected" is wrong the moment somebody closes the emulator, and a
    /// stale green dot is worse than no dot.
    pub async fn accounts(&self, app: Option<&AppHandle>) -> Result<Vec<AccountView>> {
        let accounts = self.store.accounts()?;
        if accounts.is_empty() {
            return Ok(Vec::new());
        }
        let devices = self.devices.list_devices(app).await.unwrap_or_default();

        let mut out = Vec::with_capacity(accounts.len());
        for account in accounts {
            let device = devices
                .iter()
                .find(|d| d.id == account.ldplayer_instance_id);

            let (status, detail) = match device {
                None => (
                    AccountStatus::DeviceMissing,
                    Some(format!(
                        "{} is no longer available",
                        account.ldplayer_instance_id
                    )),
                ),
                Some(d) if !d.is_online() => (
                    AccountStatus::DeviceOffline,
                    Some(format!("{} is not running", d.name)),
                ),
                Some(d) => {
                    // Only ask the device about packages when it can answer.
                    let installed = self
                        .devices
                        .packages(&account.ldplayer_instance_id)
                        .await
                        .unwrap_or_default();
                    if installed.iter().any(|p| p == &account.package_name) {
                        (AccountStatus::Connected, None)
                    } else {
                        (
                            AccountStatus::AppMissing,
                            Some(format!(
                                "{} is not installed on {}",
                                account.package_name, d.name
                            )),
                        )
                    }
                }
            };

            out.push(AccountView {
                device_name: device.map(|d| d.name.clone()),
                device_online: device.is_some_and(|d| d.is_online()),
                status,
                detail,
                account,
            });
        }
        Ok(out)
    }

    /// Every social app this app recognises, found on a device.
    ///
    /// The "add account" flow is built on this: rather than making a person
    /// type a package name, we look at what is installed and offer it.
    pub async fn discover_accounts(&self, device_id: &str) -> Result<Vec<(Platform, String)>> {
        let installed = self.devices.packages(device_id).await?;
        Ok(installed
            .into_iter()
            .filter_map(|pkg| Platform::for_package(&pkg).map(|p| (p, pkg)))
            .collect())
    }

    pub fn add_account(
        &self,
        name: &str,
        platform: Platform,
        device_id: &str,
        package: &str,
    ) -> Result<Account> {
        // Refuse a package that doesn't belong to the platform it claims:
        // it would produce an account that opens the wrong app and fails in a
        // way nobody could diagnose from the UI.
        if !platform.packages().contains(&package) {
            return Err(AppError::PackagePlatformMismatch(format!(
                "{package} is not a {} app",
                platform.label()
            )));
        }
        self.store.add_account(name, platform, device_id, package)
    }

    // -------------------------------------------------------------- submit

    /// Queue one video to every selected account. Returns the created jobs so
    /// the UI can render the queue immediately, before any work starts.
    pub async fn submit(
        &self,
        app: &AppHandle,
        queue: Arc<PublishQueue>,
        video_path: &str,
        caption: &str,
        account_ids: &[String],
    ) -> Result<Vec<PublishJob>> {
        if account_ids.is_empty() {
            return Err(AppError::NoAccountsSelected);
        }

        let path = Path::new(video_path);
        let meta = std::fs::metadata(path).map_err(|_| {
            AppError::MediaFileMissing(
                path.file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| video_path.to_string()),
            )
        })?;
        if !meta.is_file() {
            return Err(AppError::MediaFileMissing(video_path.to_string()));
        }

        let media = MediaItem {
            id: uuid::Uuid::new_v4().to_string(),
            path: video_path.to_string(),
            file_name: path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "video.mp4".into()),
            size_bytes: meta.len(),
            duration_seconds: None,
            added_at: now_unix(),
        };
        self.store.add_media(&media)?;

        let mut created = Vec::with_capacity(account_ids.len());
        for account_id in account_ids {
            // Validate up front: a job pointing at a deleted account would
            // fail one worker-slot later, for no reason.
            self.store.account(account_id)?;

            let row = JobRow {
                id: uuid::Uuid::new_v4().to_string(),
                media_id: media.id.clone(),
                account_id: account_id.clone(),
                caption: caption.to_string(),
                status: JobStatus::Pending,
                progress: 0.0,
                step: Some("Waiting for a free slot".into()),
                error_code: None,
                error: None,
                screenshot_path: None,
                created_at: now_unix(),
                started_at: None,
                completed_at: None,
            };
            self.store.add_job(&row)?;

            let view = self.store.view(row.clone())?;
            let _ = app.emit(events::CREATED, &view);
            created.push(view);

            self.spawn(app.clone(), queue.clone(), row.id);
        }
        Ok(created)
    }

    /// Put a stopped job back in the queue.
    pub fn retry(&self, app: &AppHandle, queue: Arc<PublishQueue>, job_id: &str) -> Result<PublishJob> {
        let row = self.store.reset_job(job_id)?;
        self.cancelled.lock().expect("cancel lock").remove(job_id);
        let view = self.store.view(row.clone())?;
        let _ = app.emit(events::UPDATED, &view);
        self.spawn(app.clone(), queue, row.id);
        Ok(view)
    }

    /// Ask a running job to stop at its next step boundary, or cancel a
    /// pending one outright.
    pub fn cancel(&self, app: &AppHandle, job_id: &str) -> Result<PublishJob> {
        let row = self.store.job(job_id)?;
        if row.status.is_terminal() {
            return self.store.view(row);
        }
        self.cancelled
            .lock()
            .expect("cancel lock")
            .insert(job_id.to_string());
        if row.status == JobStatus::Pending {
            // Nothing is running it, so nothing will notice the flag — finish
            // it here rather than leaving a "Pending" row nobody will move.
            self.store.update_job(
                job_id,
                JobStatus::Cancelled,
                0.0,
                Some("Cancelled"),
                None,
                None,
            )?;
        }
        let view = self.store.view(self.store.job(job_id)?)?;
        let _ = app.emit(events::UPDATED, &view);
        Ok(view)
    }

    fn is_cancelled(&self, job_id: &str) -> bool {
        self.cancelled.lock().expect("cancel lock").contains(job_id)
    }

    // --------------------------------------------------------------- reads

    pub fn list(&self) -> Result<Vec<PublishJob>> {
        self.store
            .jobs(JOB_PAGE)?
            .into_iter()
            .map(|r| self.store.view(r))
            .collect()
    }

    pub fn summary(&self) -> Result<QueueSummary> {
        let mut s = QueueSummary::default();
        for (status, n) in self.store.counts()? {
            match status.as_str() {
                "pending" => s.pending += n,
                "uploading" | "publishing" => s.active += n,
                "published" => s.published += n,
                "needs_attention" => s.needs_attention += n,
                "failed" => s.failed += n,
                _ => {}
            }
        }
        Ok(s)
    }

    // ------------------------------------------------------------- workers

    fn spawn(&self, app: AppHandle, queue: Arc<PublishQueue>, job_id: String) {
        let permits = self.permits();
        tauri::async_runtime::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return; // Only on shutdown.
            };
            queue.run(&app, &job_id).await;
        });
    }

    /// Record a transition and tell the UI. Every status change in this file
    /// goes through here, so "the database and the screen disagree" has one
    /// place to be wrong rather than a dozen.
    fn set(
        &self,
        app: &AppHandle,
        job_id: &str,
        status: JobStatus,
        progress: f64,
        step: Option<&str>,
        error_code: Option<&str>,
        error: Option<&str>,
    ) {
        let _ = self
            .store
            .update_job(job_id, status, progress, step, error_code, error);
        if let Ok(view) = self.store.job(job_id).and_then(|r| self.store.view(r)) {
            let _ = app.emit(events::UPDATED, &view);
            if status.is_terminal() {
                let _ = app.emit(events::FINISHED, &view);
            }
        }
    }

    /// Run one job end to end.
    ///
    /// The stages are fixed and generic — wake, copy, index, hand over — and
    /// only the last is delegated. Cancellation is checked between stages; see
    /// the note on [`Self::cancelled`] for why not inside them.
    async fn run(&self, app: &AppHandle, job_id: &str) {
        if self.is_cancelled(job_id) {
            self.set(app, job_id, JobStatus::Cancelled, 0.0, Some("Cancelled"), None, None);
            return;
        }
        match self.run_inner(app, job_id).await {
            Ok(Outcome::Published) => {
                self.set(app, job_id, JobStatus::Published, 1.0, Some("Published"), None, None);
            }
            Ok(Outcome::NeedsUser(message)) => {
                // Deliberately not "Failed": the video is on the device and the
                // app is open on it. Calling this a failure would teach people
                // to ignore the failures that matter.
                self.set(
                    app,
                    job_id,
                    JobStatus::NeedsAttention,
                    1.0,
                    Some("Ready for you to post"),
                    Some("needs_user"),
                    Some(&message),
                );
            }
            Err(e) => {
                if self.is_cancelled(job_id) {
                    self.set(app, job_id, JobStatus::Cancelled, 0.0, Some("Cancelled"), None, None);
                    return;
                }
                let code = e.code().to_string();
                let message = e.to_string();
                self.devices
                    .log(Some(app), "error", Some(job_id), format!("{code}: {message}"));
                self.set(
                    app,
                    job_id,
                    JobStatus::Failed,
                    0.0,
                    Some("Failed"),
                    Some(&code),
                    Some(&message),
                );
            }
        }
    }

    async fn run_inner(&self, app: &AppHandle, job_id: &str) -> Result<Outcome> {
        let row = self.store.job(job_id)?;
        let account = self.store.account(&row.account_id)?;
        let media = self.store.media(&row.media_id)?;
        let settings = self.devices.settings();

        // 1. The device. Booting a cold instance is the slowest step by far,
        //    so it gets its own progress band and its own step text.
        self.set(
            app,
            job_id,
            JobStatus::Uploading,
            0.05,
            Some("Waiting for the emulator to be ready"),
            None,
            None,
        );
        let serial = self
            .devices
            .ensure_online(Some(app), &account.ldplayer_instance_id)
            .await?;

        if self.is_cancelled(job_id) {
            return Err(AppError::Cancelled);
        }

        // 2. The app has to exist before we spend a minute copying a file for
        //    it. Checking here turns a late, confusing failure into an early,
        //    obvious one.
        let adb = self.devices.adb()?;
        if !adb.is_installed(&serial, &account.package_name).await? {
            return Err(AppError::AppNotInstalled(account.package_name.clone()));
        }

        // 3. Copy and index. This is the step that replaces dragging a file
        //    into LDPlayer by hand.
        self.set(
            app,
            job_id,
            JobStatus::Uploading,
            0.2,
            Some(&format!("Copying {} to the device", media.file_name)),
            None,
            None,
        );
        let remote_path = self
            .devices
            .transfer_media(Some(app), &account.ldplayer_instance_id, Path::new(&media.path))
            .await?;

        if self.is_cancelled(job_id) {
            self.devices
                .remove_media(&account.ldplayer_instance_id, &remote_path)
                .await;
            return Err(AppError::Cancelled);
        }

        self.set(
            app,
            job_id,
            JobStatus::Publishing,
            0.65,
            Some("Video is in the device gallery"),
            None,
            None,
        );

        // 4. Hand off to the platform connector — the one platform-aware step.
        let content_uri = adb.media_store_uri(&serial, &remote_path).await;
        let connector = connector::for_platform(account.platform);

        let ctx = PublishContext {
            manager: self.devices.clone(),
            device_id: account.ldplayer_instance_id.clone(),
            serial: serial.clone(),
            package: account.package_name.clone(),
            remote_path: remote_path.clone(),
            content_uri,
            caption: row.caption.clone(),
            report: {
                let store = self.store.clone();
                let app = app.clone();
                let job_id = job_id.to_string();
                Box::new(move |progress, message| {
                    let _ = store.update_job(
                        &job_id,
                        JobStatus::Publishing,
                        progress,
                        Some(message),
                        None,
                        None,
                    );
                    if let Ok(view) = store.job(&job_id).and_then(|r| store.view(r)) {
                        let _ = app.emit(events::UPDATED, &view);
                    }
                })
            },
        };

        self.devices.log(
            Some(app),
            "info",
            Some(job_id),
            format!(
                "publishing to {} via {}",
                account.platform.label(),
                connector.strategy()
            ),
        );

        let outcome = connector.publish(&ctx).await;

        // A screenshot of wherever we ended up, success or failure — it is the
        // one artefact that answers "what is it stuck on?" without alt-tabbing
        // to LDPlayer.
        if settings.verbose_logging || outcome.is_err() {
            if let Ok(path) = self
                .devices
                .screenshot(&account.ldplayer_instance_id, Some("result"))
                .await
            {
                let _ = self.store.set_job_screenshot(job_id, &path);
            }
        }

        let outcome = outcome?;

        // Only tidy up after a clean finish. A handed-off job still needs its
        // file: the person has not tapped Post yet.
        if settings.cleanup_after_publish && matches!(outcome, Outcome::Published) {
            self.devices
                .remove_media(&account.ldplayer_instance_id, &remote_path)
                .await;
        }

        Ok(outcome)
    }
}
