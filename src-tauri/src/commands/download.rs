//! Tauri commands for the downloader.
//!
//! As in `commands::auth`, nothing returned here can carry a secret: a
//! `JobView` holds a title, a size and a local path. The signed CDN URL the
//! engine fetches from never leaves Rust.

use std::sync::Arc;

use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::download::manager::{run_job, Destination, DownloadManager, EngineStatus, JobView};
use crate::download::quality::Quality;
use crate::download::session::{self, InstagramSession, SessionStatus, StoredCookie};
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

const IG_LOGIN_LABEL: &str = "instagram-login";
const IG_LOGIN_URL: &str = "https://www.instagram.com/accounts/login/";

/// Open a dedicated Instagram login window and capture its session.
///
/// A separate window rather than `--cookies-from-browser`: that flag would
/// hand yt-dlp the user's entire browser profile - every site they are signed
/// into. This reads cookies from one window, scoped to one URL, that the user
/// opened deliberately for this purpose.
///
/// Resolves when a `sessionid` cookie appears, which is the moment login
/// actually succeeded. Closing the window first cancels the flow.
#[tauri::command]
pub async fn download_instagram_connect(app: AppHandle) -> Result<SessionStatus> {
    // Reusing a stale window would show a page from a previous attempt.
    if let Some(existing) = app.get_webview_window(IG_LOGIN_LABEL) {
        let _ = existing.close();
    }

    let url = IG_LOGIN_URL
        .parse()
        .map_err(|_| AppError::Internal("bad login url".into()))?;

    let window = WebviewWindowBuilder::new(&app, IG_LOGIN_LABEL, WebviewUrl::External(url))
        .title("Sign in to Instagram")
        .inner_size(480.0, 760.0)
        .resizable(true)
        .focused(true)
        .build()
        .map_err(|e| AppError::Internal(format!("could not open the login window: {e}")))?;

    // Poll rather than hook navigation: Instagram's login is a single-page app
    // that finishes without a page load we could listen for, and the cookie
    // appearing is the only reliable signal that it worked.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    // Kept so a timeout can say what went wrong instead of just giving up.
    let mut last_error: Option<String> = None;
    let mut seen_cookie_names: Vec<String> = Vec::new();

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        // Closed by the user - a cancelled sign-in, not a failure.
        if app.get_webview_window(IG_LOGIN_LABEL).is_none() {
            return Err(AppError::Cancelled);
        }
        if std::time::Instant::now() > deadline {
            let _ = window.close();
            // Names only - never values - so a failure is diagnosable without
            // a session cookie ending up in a log or an error message.
            return Err(AppError::Internal(format!(
                "signed-in session not detected after 5 minutes. Instagram cookies seen: [{}].{}",
                seen_cookie_names.join(", "),
                last_error
                    .map(|e| format!(" Last cookie-read error: {e}"))
                    .unwrap_or_default()
            )));
        }

        // Deliberately NOT `cookies_for_url`: on macOS that compares the
        // cookie's domain to the URL's host by exact string equality, so a
        // `.instagram.com` cookie never matches `www.instagram.com` and the
        // jar comes back empty every single time. Ask for everything and do
        // the domain match here, where a leading dot is handled.
        let jar = match window.cookies() {
            Ok(jar) => jar,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        };

        let cookies: Vec<StoredCookie> = jar
            .iter()
            .filter(|c| c.domain().map(session::is_instagram_domain).unwrap_or(false))
            .map(|c| StoredCookie {
                name: c.name().to_string(),
                value: c.value().to_string(),
                domain: c
                    .domain()
                    .map(|d| {
                        // Netscape files want the leading dot for domain cookies.
                        if d.starts_with('.') { d.to_string() } else { format!(".{d}") }
                    })
                    .unwrap_or_else(|| ".instagram.com".to_string()),
                path: c.path().unwrap_or("/").to_string(),
                secure: c.secure().unwrap_or(true),
                expires: c
                    .expires()
                    .and_then(|e| e.datetime())
                    .map(|d| d.unix_timestamp())
                    .unwrap_or(0),
            })
            .collect();

        seen_cookie_names = cookies.iter().map(|c| c.name.clone()).collect();

        let candidate = InstagramSession {
            cookies,
            captured_at: crate::auth::now_unix(),
        };
        if candidate.is_usable() {
            session::save(&candidate)?;
            let _ = window.close();
            return Ok(session::status());
        }
    }
}

#[tauri::command]
pub async fn download_instagram_status() -> Result<SessionStatus> {
    Ok(session::status())
}

/// Forget the stored session. The window's own cookies are not this app's to
/// clear, so the user is told to sign out in the browser too if they want that.
#[tauri::command]
pub async fn download_instagram_disconnect() -> Result<SessionStatus> {
    session::clear()?;
    Ok(session::status())
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
