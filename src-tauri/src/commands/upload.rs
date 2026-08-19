//! Unified upload: one command surface behind a single "Upload" screen.
//!
//! The frontend picks a file, fills a form, and chooses a target platform; the
//! matching upload runs in Rust with that platform's stored credential. Only
//! YouTube is implemented so far; the others report why they aren't ready, so
//! the selector can show the whole set honestly.

use tauri::{AppHandle, State};

use crate::auth::manager::AuthManager;
use crate::auth::ProviderId;
use crate::errors::{AppError, Result};
use crate::youtube::{self, Channel, Privacy};

/// One selectable destination and whether it can be used right now.
#[derive(serde::Serialize)]
pub struct UploadTarget {
    pub id: String,
    pub name: String,
    /// "video", "photo", or "video,photo" — what this target accepts.
    pub accepts: String,
    pub ready: bool,
    /// Shown when not ready, so the selector explains itself.
    pub reason: Option<String>,
}

/// Which platforms the Upload screen can offer, and their readiness.
#[tauri::command]
pub async fn upload_targets(manager: State<'_, AuthManager>) -> Result<Vec<UploadTarget>> {
    // YouTube is ready when Google is connected (the token carries the upload
    // scope once the account is reconnected after enabling it).
    let youtube_ready = manager.access_token(ProviderId::Google).await.is_ok();

    Ok(vec![
        UploadTarget {
            id: "youtube".into(),
            name: "YouTube".into(),
            accepts: "video".into(),
            ready: youtube_ready,
            reason: if youtube_ready {
                None
            } else {
                Some("Connect Google on the Accounts page (with YouTube upload enabled).".into())
            },
        },
        UploadTarget {
            id: "facebook".into(),
            name: "Facebook Page".into(),
            accepts: "photo".into(),
            ready: false,
            reason: Some("Needs Facebook Business Verification before publishing.".into()),
        },
        UploadTarget {
            id: "telegram".into(),
            name: "Telegram".into(),
            accepts: "video,photo".into(),
            ready: false,
            reason: Some("Telegram upload isn't built yet.".into()),
        },
    ])
}

/// Pick a file to upload. `kind` is "video", "photo", or "any".
#[tauri::command]
pub async fn upload_pick_file(app: AppHandle, kind: String) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Choose a file to upload");
    dialog = match kind.as_str() {
        "video" => dialog.add_filter("Videos", &["mp4", "mov", "webm", "mkv", "avi", "m4v"]),
        "photo" => dialog.add_filter("Images", &["jpg", "jpeg", "png", "gif", "webp"]),
        _ => dialog.add_filter("Media", &["mp4", "mov", "webm", "jpg", "jpeg", "png", "gif", "webp"]),
    };
    dialog.pick_file(move |f| {
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

/// Which YouTube channel(s) the connected Google account will upload to.
#[tauri::command]
pub async fn upload_youtube_channels(manager: State<'_, AuthManager>) -> Result<Vec<Channel>> {
    let cred = manager.access_token(ProviderId::Google).await?;
    youtube::my_channels(&reqwest::Client::new(), &cred.access_token).await
}

/// Upload a video to the user's YouTube channel. Returns the video id.
#[tauri::command]
pub async fn upload_youtube(
    manager: State<'_, AuthManager>,
    file_path: String,
    title: String,
    description: String,
    privacy: String,
) -> Result<String> {
    let cred = manager.access_token(ProviderId::Google).await?;
    youtube::upload_video(
        &reqwest::Client::new(),
        &cred.access_token,
        std::path::Path::new(&file_path),
        &title,
        &description,
        Privacy::parse(&privacy),
    )
    .await
}
