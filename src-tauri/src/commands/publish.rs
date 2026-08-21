//! IPC for accounts and the publishing queue.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::errors::{AppError, Result};
use crate::publish::model::{
    Account, AccountView, Platform, PostMode, PublishJob, PublishTarget, VideoFormat,
};
use crate::publish::queue::{PublishQueue, QueueSummary};

type Queue<'a> = State<'a, Arc<PublishQueue>>;

/// A social app found installed on a device, offered as an account to add.
#[derive(Debug, Serialize)]
pub struct DiscoveredApp {
    pub platform: Platform,
    pub package: String,
    pub label: String,
}

/// Platform metadata the UI renders (names, packages), so the list of
/// supported platforms lives in Rust only.
#[derive(Debug, Serialize)]
pub struct PlatformInfo {
    pub id: Platform,
    pub label: String,
    pub packages: Vec<String>,
}

#[tauri::command]
pub async fn publish_platforms() -> Result<Vec<PlatformInfo>> {
    // AVAILABLE, not ALL: the picker must not offer a platform a job cannot
    // be run against.
    Ok(Platform::AVAILABLE
        .iter()
        .map(|p| PlatformInfo {
            id: *p,
            label: p.label().to_string(),
            packages: p.packages().iter().map(|s| s.to_string()).collect(),
        })
        .collect())
}

// ---------------------------------------------------------------- accounts

#[tauri::command]
pub async fn publish_accounts(app: AppHandle, queue: Queue<'_>) -> Result<Vec<AccountView>> {
    queue.accounts(Some(&app)).await
}

/// Which recognised social apps are installed on a device — the basis of the
/// "add account" flow, so nobody has to type a package name.
#[tauri::command]
pub async fn publish_discover_accounts(
    queue: Queue<'_>,
    device_id: String,
) -> Result<Vec<DiscoveredApp>> {
    Ok(queue
        .discover_accounts(&device_id)
        .await?
        .into_iter()
        .map(|(platform, package)| DiscoveredApp {
            platform,
            label: platform.label().to_string(),
            package,
        })
        .collect())
}

#[tauri::command]
pub async fn publish_add_account(
    queue: Queue<'_>,
    name: String,
    platform: String,
    device_id: String,
    package: String,
) -> Result<Account> {
    let platform = Platform::parse(&platform)
        .ok_or_else(|| AppError::UnknownProvider(platform.clone()))?;
    queue.add_account(name.trim(), platform, &device_id, &package)
}

#[tauri::command]
pub async fn publish_rename_account(queue: Queue<'_>, id: String, name: String) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Internal("an account needs a name".into()));
    }
    queue.store().rename_account(&id, name)
}

/// Record the name this account posts under, so every post can be checked
/// against it first. An empty string clears it and switches the check off.
#[tauri::command]
pub async fn publish_set_profile_name(
    queue: Queue<'_>,
    id: String,
    profile_name: String,
) -> Result<()> {
    queue.store().set_account_profile_name(&id, &profile_name)
}

#[tauri::command]
pub async fn publish_remove_account(queue: Queue<'_>, id: String) -> Result<()> {
    queue.store().remove_account(&id)
}

// -------------------------------------------------------------------- jobs

/// Queue the selected media to every selected account. Returns immediately
/// with the created jobs; progress arrives on `publish://updated`.
///
/// `mode` decides how several files become posts: `album` makes one post per
/// account carrying all of them, `single` makes one post per file per account.
/// It is required rather than defaulted — guessing wrong publishes three posts
/// where the user wanted one, and that is not undoable.
#[tauri::command]
pub async fn publish_submit(
    app: AppHandle,
    queue: Queue<'_>,
    paths: Vec<String>,
    caption: String,
    targets: Vec<PublishTarget>,
    mode: String,
    video_format: Option<String>,
) -> Result<Vec<PublishJob>> {
    let mode = PostMode::parse(&mode)
        .ok_or_else(|| AppError::Internal(format!("unknown post mode `{mode}`")))?;
    // Absent means the default. An unrecognised value does not: publishing a
    // Reel when a feed post was asked for is not a rounding error.
    let video_format = match video_format.as_deref() {
        None => VideoFormat::default(),
        Some(v) => VideoFormat::parse(v)
            .ok_or_else(|| AppError::Internal(format!("unknown video format `{v}`")))?,
    };
    let handle = queue.inner().clone();
    handle
        .submit(&app, handle.clone(), &paths, &caption, &targets, mode, video_format)
        .await
}

// ------------------------------------------------------------------- pages

/// The Pages an account can post as, as the dashboard lists them.
/// Read this account's Pages out of the Facebook app and store them.
#[tauri::command]
pub async fn publish_discover_pages(queue: Queue<'_>, id: String) -> Result<Vec<String>> {
    queue.inner().discover_pages(&id).await
}

#[tauri::command]
pub async fn publish_add_page(queue: Queue<'_>, id: String, page_name: String) -> Result<()> {
    queue.store().add_account_page(&id, &page_name)
}

#[tauri::command]
pub async fn publish_remove_page(queue: Queue<'_>, id: String, page_name: String) -> Result<()> {
    queue.store().remove_account_page(&id, &page_name)
}

#[tauri::command]
pub async fn publish_jobs(queue: Queue<'_>) -> Result<Vec<PublishJob>> {
    queue.list()
}

#[tauri::command]
pub async fn publish_summary(queue: Queue<'_>) -> Result<QueueSummary> {
    queue.summary()
}

#[tauri::command]
pub async fn publish_retry(app: AppHandle, queue: Queue<'_>, id: String) -> Result<PublishJob> {
    let handle = queue.inner().clone();
    handle.retry(&app, handle.clone(), &id)
}

#[tauri::command]
pub async fn publish_cancel(app: AppHandle, queue: Queue<'_>, id: String) -> Result<PublishJob> {
    queue.cancel(&app, &id)
}

#[tauri::command]
pub async fn publish_remove_job(queue: Queue<'_>, id: String) -> Result<()> {
    queue.store().remove_job(&id)
}

#[tauri::command]
pub async fn publish_clear_finished(queue: Queue<'_>) -> Result<usize> {
    queue.store().clear_finished()
}
