//! Tauri commands for Facebook Page publishing.
//!
//! The frontend gets Pages (no tokens) and requests uploads by page id; the
//! access tokens stay in Rust.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::auth::manager::AuthManager;
use crate::auth::ProviderId;
use crate::download::manager::DownloadManager;
use crate::errors::{AppError, Result};
use crate::facebook::{self, Page};

/// A shared HTTP client would be ideal, but the auth layer owns one privately;
/// a fresh client per call is fine for these low-frequency actions.
fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn user_token(manager: &AuthManager) -> Result<String> {
    let cred = manager.access_token(ProviderId::Facebook).await?;
    Ok(cred.access_token)
}

/// The Pages the connected Facebook account can publish to.
#[tauri::command]
pub async fn facebook_list_pages(manager: State<'_, AuthManager>) -> Result<Vec<Page>> {
    let token = user_token(&manager).await?;
    facebook::list_pages(&http(), &token).await
}

/// Open a picker for an image file. `Ok(None)` when dismissed.
#[tauri::command]
pub async fn facebook_pick_photo(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Images", &["jpg", "jpeg", "png", "gif", "webp"])
        .set_title("Choose a photo to upload")
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx.await.map_err(|_| AppError::Internal("picker closed".into()))?;
    match picked {
        Some(p) => Ok(Some(
            p.into_path().map_err(|e| AppError::DownloadPath(e.to_string()))?.display().to_string(),
        )),
        None => Ok(None),
    }
}

/// Publish a photo to a Page. Returns the new post id.
#[tauri::command]
pub async fn facebook_upload_photo(
    manager: State<'_, AuthManager>,
    page_id: String,
    file_path: String,
    caption: String,
) -> Result<String> {
    let token = user_token(&manager).await?;
    facebook::upload_photo(
        &http(),
        &token,
        &page_id,
        std::path::Path::new(&file_path),
        &caption,
    )
    .await
}

/// The most recently downloaded files, offered as a source to upload from.
#[tauri::command]
pub async fn facebook_recent_downloads(
    downloads: State<'_, Arc<DownloadManager>>,
) -> Result<Vec<String>> {
    Ok(downloads
        .list()
        .into_iter()
        .filter_map(|j| j.output_path)
        .filter(|p| {
            let lower = p.to_lowercase();
            [".jpg", ".jpeg", ".png", ".gif", ".webp"].iter().any(|e| lower.ends_with(e))
        })
        .take(12)
        .collect())
}
