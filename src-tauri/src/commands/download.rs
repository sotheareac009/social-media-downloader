//! Tauri commands for the downloader.
//!
//! As in `commands::auth`, nothing returned here can carry a secret: a
//! `JobView` holds a title, a size and a local path. The signed CDN URL the
//! engine fetches from never leaves Rust.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::download::manager::{run_job, Destination, DownloadManager, EngineStatus, JobView};
use crate::download::quality::Quality;
use crate::download::url::{classify_target, TargetKind};
use crate::download::ytdlp::{FormatReport, MediaInfo, ProfileListing};
use crate::errors::{AppError, Result};

/// Whether yt-dlp is installed, and which one we found. Drives the setup
/// notice on the Downloads page.
#[tauri::command]
pub async fn download_engine_status(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<EngineStatus> {
    Ok(manager.engine_status().await)
}

/// Read a link's metadata without downloading it. Used for the paste preview,
/// so a person can confirm they pasted the right thing.
#[tauri::command]
pub async fn download_inspect(
    manager: State<'_, Arc<DownloadManager>>,
    url: String,
) -> Result<MediaInfo> {
    manager.inspect(&url).await
}

/// Read a link's metadata and the quality tiers it offers, without downloading.
///
/// Powers the inspection panel: a video that has 8K should offer 8K, and one
/// that tops out at 720p should not pretend otherwise.
#[tauri::command]
pub async fn download_inspect_formats(
    manager: State<'_, Arc<DownloadManager>>,
    url: String,
) -> Result<FormatReport> {
    manager.inspect_formats(&url).await
}

/// Queue a link. Returns immediately with the new job; progress arrives on the
/// `download://*` event stream.
#[tauri::command]
pub async fn download_start(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    url: String,
    quality: Option<Quality>,
) -> Result<JobView> {
    let view = manager.enqueue(&app, &url, quality)?;
    let manager = Arc::clone(&manager);
    let id = view.id.clone();
    tokio::spawn(run_job(manager, app, id));
    Ok(view)
}

/// A link that couldn't be used, and why - kept alongside the successes so a
/// mixed paste reports precisely instead of failing as a whole.
#[derive(serde::Serialize)]
pub struct RejectedLink {
    pub url: String,
    pub code: String,
    pub message: String,
}

/// The result of submitting a paste.
///
/// Single videos are queued straight away. Profiles are *not*: enumerating one
/// can turn a single line into 133 downloads, so the listing comes back for
/// the user to confirm. Deciding that on their behalf is not this layer's call.
#[derive(serde::Serialize)]
pub struct Submission {
    pub queued: Vec<JobView>,
    pub profiles: Vec<ProfileListing>,
    pub rejected: Vec<RejectedLink>,
}

/// Handle a whole paste: any mix of video links and profile links.
#[tauri::command]
pub async fn download_submit(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    urls: Vec<String>,
    quality: Option<Quality>,
) -> Result<Submission> {
    let mut queued = Vec::new();
    let mut profiles = Vec::new();
    let mut rejected = Vec::new();

    for raw in urls {
        match classify_target(&raw) {
            Err(e) => rejected.push(RejectedLink {
                url: raw,
                code: e.code().to_string(),
                message: e.to_string(),
            }),
            Ok((_, _, TargetKind::Profile)) => match manager.inspect_profile(&raw).await {
                Ok(listing) => profiles.push(listing),
                Err(e) => rejected.push(RejectedLink {
                    url: raw,
                    code: e.code().to_string(),
                    message: e.to_string(),
                }),
            },
            Ok((_, _, TargetKind::Single)) => match manager.enqueue(&app, &raw, quality) {
                Ok(view) => {
                    let id = view.id.clone();
                    queued.push(view);
                    tokio::spawn(run_job(Arc::clone(&manager), app.clone(), id));
                }
                Err(e) => rejected.push(RejectedLink {
                    url: raw,
                    code: e.code().to_string(),
                    message: e.to_string(),
                }),
            },
        }
    }

    Ok(Submission {
        queued,
        profiles,
        rejected,
    })
}

/// Queue every video from a profile the user has confirmed.
#[tauri::command]
pub async fn download_start_many(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    urls: Vec<String>,
    quality: Option<Quality>,
) -> Result<Vec<JobView>> {
    let (queued, _failed) = manager.enqueue_all(&app, &urls, quality);
    for view in &queued {
        tokio::spawn(run_job(Arc::clone(&manager), app.clone(), view.id.clone()));
    }
    Ok(queued)
}

#[tauri::command]
pub async fn download_list(manager: State<'_, Arc<DownloadManager>>) -> Result<Vec<JobView>> {
    Ok(manager.list())
}

#[tauri::command]
pub async fn download_cancel(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    id: String,
) -> Result<JobView> {
    manager.cancel(&app, &id)
}

#[tauri::command]
pub async fn download_remove(
    manager: State<'_, Arc<DownloadManager>>,
    id: String,
) -> Result<()> {
    manager.remove(&id)
}

#[tauri::command]
pub async fn download_clear_finished(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<usize> {
    Ok(manager.clear_finished())
}

#[tauri::command]
pub async fn download_get_destination(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<Destination> {
    Ok(manager.destination_view())
}

#[tauri::command]
pub async fn download_set_destination(
    manager: State<'_, Arc<DownloadManager>>,
    path: String,
) -> Result<Destination> {
    manager.set_destination(std::path::PathBuf::from(path))
}

/// One quality option as the picker renders it.
#[derive(serde::Serialize)]
pub struct QualityOption {
    pub id: Quality,
    pub label: String,
    /// True when this option needs FFmpeg to mean anything - every capped
    /// option above 360p does, because YouTube serves those as split streams.
    pub needs_ffmpeg: bool,
}

/// The quality menu plus the current choice.
#[derive(serde::Serialize)]
pub struct QualitySettings {
    pub selected: Quality,
    pub options: Vec<QualityOption>,
    /// Repeated here so the picker can warn without a second round trip.
    pub has_ffmpeg: bool,
}

#[tauri::command]
pub async fn download_get_quality(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<QualitySettings> {
    let has_ffmpeg = crate::download::ytdlp::locate_ffmpeg().is_some();
    Ok(QualitySettings {
        selected: manager.quality(),
        has_ffmpeg,
        options: Quality::ALL
            .iter()
            .map(|q| QualityOption {
                id: *q,
                label: q.label().to_string(),
                // 360p is reachable as a single progressive file; anything
                // above it needs a merge on YouTube.
                needs_ffmpeg: q.max_height().map_or(true, |h| h > 360),
            })
            .collect(),
    })
}

#[tauri::command]
pub async fn download_set_quality(
    manager: State<'_, Arc<DownloadManager>>,
    quality: Quality,
) -> Result<Quality> {
    manager.set_quality(quality)
}

#[tauri::command]
pub async fn download_reset_destination(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<Destination> {
    manager.reset_destination()
}

/// Open the OS folder picker and return what was chosen, changing nothing.
///
/// Browsing is deliberately separate from saving: the returned path is a
/// *proposal* the UI shows until the user confirms it. Picking a folder by
/// accident should not silently redirect every future download.
///
/// The dialog is opened from Rust rather than JavaScript so the frontend never
/// needs filesystem capabilities: the webview gets a path string back, and no
/// ability to read or write anything itself.
///
/// `Ok(None)` means the user dismissed the dialog - a normal outcome, not an
/// error, and the UI must not show a failure toast for it.
#[tauri::command]
pub async fn download_browse_destination(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let current = manager.destination();
    let (tx, rx) = tokio::sync::oneshot::channel();

    app.dialog()
        .file()
        .set_title("Choose where downloads are saved")
        // Start where they already save, so "somewhere near here" is one click.
        .set_directory(if current.is_dir() {
            current
        } else {
            std::env::temp_dir()
        })
        .pick_folder(move |picked| {
            let _ = tx.send(picked);
        });

    let picked = rx
        .await
        .map_err(|_| AppError::Internal("the folder picker closed unexpectedly".into()))?;

    let Some(path) = picked else { return Ok(None) };
    let path = path
        .into_path()
        .map_err(|e| AppError::DownloadPath(e.to_string()))?;

    Ok(Some(path.display().to_string()))
}

/// Open a folder in Finder / Explorer / the desktop file manager.
///
/// Accepts either a file - in which case its containing folder is opened, the
/// "show me this download" case - or a folder, which is opened directly.
#[tauri::command]
pub async fn download_reveal(app: AppHandle, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let p = std::path::PathBuf::from(&path);
    let target = if p.is_dir() {
        p.as_path()
    } else {
        p.parent().unwrap_or(p.as_path())
    };
    app.opener()
        .open_path(target.display().to_string(), None::<&str>)
        .map_err(|e| crate::errors::AppError::Internal(e.to_string()))
}
