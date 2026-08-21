//! IPC for accounts and the publishing queue.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::errors::{AppError, Result};
use crate::publish::model::{Account, AccountView, Platform, PublishJob};
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
    Ok(Platform::ALL
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

#[tauri::command]
pub async fn publish_remove_account(queue: Queue<'_>, id: String) -> Result<()> {
    queue.store().remove_account(&id)
}

// -------------------------------------------------------------------- jobs

/// Queue one video to every selected account. Returns immediately with the
/// created jobs; progress arrives on `publish://updated`.
#[tauri::command]
pub async fn publish_submit(
    app: AppHandle,
    queue: Queue<'_>,
    video_path: String,
    caption: String,
    account_ids: Vec<String>,
) -> Result<Vec<PublishJob>> {
    let handle = queue.inner().clone();
    handle
        .submit(&app, handle.clone(), &video_path, &caption, &account_ids)
        .await
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
