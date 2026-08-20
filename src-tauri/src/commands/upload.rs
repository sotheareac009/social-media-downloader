//! Unified upload: one command surface behind a single "Upload" screen.
//!
//! The frontend picks a file, fills a form, and chooses a target platform; the
//! matching upload runs in Rust with that platform's stored credential. Only
//! YouTube is implemented so far; the others report why they aren't ready, so
//! the selector can show the whole set honestly.

use tauri::{AppHandle, Manager, State};

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
pub async fn upload_targets(
    app: AppHandle,
    manager: State<'_, AuthManager>,
) -> Result<Vec<UploadTarget>> {
    // Telegram is ready when a session exists (non-secret file check).
    let telegram_ready = app
        .path()
        .app_data_dir()
        .map(|d| crate::telegram::status(&d).connected)
        .unwrap_or(false);

    // YouTube is ready when at least one uploader account has been added on the
    // Upload page. These are separate from the single-account slot used by the
    // Accounts page — a creator can push one video to several channels.
    let youtube_ready = app
        .path()
        .app_data_dir()
        .map(|d| !crate::youtube_accounts::list(&d).is_empty())
        .unwrap_or(false);

    // TikTok needs more than a connection. Login grants `user.info.basic`,
    // which reads a profile and nothing else; posting goes through the Content
    // Posting API and needs `video.publish`, or `video.upload` to send the
    // video to the creator's TikTok inbox for them to finish.
    // Checking the granted scopes rather than just "is connected" keeps the
    // card from promising an upload that would fail at the first API call.
    let tiktok = manager.access_token(ProviderId::TikTok).await.ok();
    let tiktok_connected = tiktok.is_some();
    let tiktok_can_post = tiktok
        .as_ref()
        .is_some_and(|c| {
            c.scopes
                .iter()
                .any(|s| s == "video.publish" || s == "video.upload")
        });

    Ok(vec![
        UploadTarget {
            id: "youtube".into(),
            name: "YouTube".into(),
            accepts: "video".into(),
            ready: youtube_ready,
            reason: if youtube_ready {
                None
            } else {
                Some("Add a YouTube account below to upload.".into())
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
            id: "tiktok".into(),
            name: "TikTok".into(),
            accepts: "video".into(),
            ready: tiktok_can_post,
            reason: if tiktok_can_post {
                None
            } else if tiktok_connected {
                Some(
                    "Connected, but this login only grants profile access. Posting needs the \
                     video.publish scope, which TikTok grants after app review."
                        .into(),
                )
            } else {
                Some("Connect TikTok on the Accounts page.".into())
            },
        },
        UploadTarget {
            id: "telegram".into(),
            name: "Telegram".into(),
            accepts: "video,photo".into(),
            ready: telegram_ready,
            reason: if telegram_ready {
                None
            } else {
                Some("Connect Telegram first (Telegram page → sign in).".into())
            },
        },
    ])
}

/// Pick one or more files to upload. `kind` is "video", "photo", or "any".
/// Returns every selected path (empty when the picker is dismissed).
#[tauri::command]
pub async fn upload_pick_files(app: AppHandle, kind: String) -> Result<Vec<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Choose files to upload");
    dialog = match kind.as_str() {
        "video" => dialog.add_filter("Videos", &["mp4", "mov", "webm", "mkv", "avi", "m4v"]),
        "photo" => dialog.add_filter("Images", &["jpg", "jpeg", "png", "gif", "webp"]),
        _ => dialog.add_filter("Media", &["mp4", "mov", "webm", "jpg", "jpeg", "png", "gif", "webp"]),
    };
    dialog.pick_files(move |f| {
        let _ = tx.send(f);
    });
    let picked = rx.await.map_err(|_| AppError::Internal("picker closed".into()))?;
    let paths = picked.unwrap_or_default();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(p.into_path().map_err(|e| AppError::DownloadPath(e.to_string()))?.display().to_string());
    }
    Ok(out)
}

/// Which YouTube channel(s) the connected Google account will upload to.
#[tauri::command]
pub async fn upload_youtube_channels(manager: State<'_, AuthManager>) -> Result<Vec<Channel>> {
    let cred = manager.access_token(ProviderId::Google).await?;
    youtube::my_channels(&reqwest::Client::new(), &cred.access_token).await
}

/// A video's dimensions and duration, for correct Telegram video attributes.
#[derive(serde::Serialize)]
pub struct VideoMeta {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
}

/// Probe a video with ffprobe. `None` when ffmpeg/ffprobe is missing or the
/// probe fails - the caller then sends without video attributes.
#[tauri::command]
pub async fn upload_video_meta(path: String) -> Result<Option<VideoMeta>> {
    let Some(ffmpeg) = crate::download::ytdlp::locate_ffmpeg() else {
        return Ok(None);
    };
    let Some(ffprobe) = crate::download::compat::ffprobe_beside(&ffmpeg) else {
        return Ok(None);
    };

    let out = crate::process::command(ffprobe)
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height:format=duration",
            "-of", "json",
        ])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|_| AppError::EngineMissing)?;

    if !out.status.success() {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

    let stream = v.get("streams").and_then(|s| s.get(0));
    let width = stream.and_then(|s| s.get("width")).and_then(|x| x.as_u64());
    let height = stream.and_then(|s| s.get("height")).and_then(|x| x.as_u64());
    let duration = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Ok(Some(VideoMeta {
            width: w as u32,
            height: h as u32,
            duration: duration.unwrap_or(0.0),
        })),
        _ => Ok(None),
    }
}

/// Extract a poster frame from a video, as a base64 `data:` URL.
///
/// A `<video>` element in the webview shows a black frame until it plays, so a
/// real thumbnail needs a decoded frame. ffmpeg (already required for the
/// download pipeline) grabs one cheaply. Returns `None` when ffmpeg is missing
/// or the grab fails - the UI then falls back to a plain placeholder.
#[tauri::command]
pub async fn upload_video_thumbnail(path: String) -> Result<Option<String>> {
    use base64::Engine;

    let Some(ffmpeg) = crate::download::ytdlp::locate_ffmpeg() else {
        return Ok(None);
    };

    // Seek ~1s in (past black intro frames), grab one frame, scale to a
    // thumbnail, and write JPEG to stdout.
    let out = crate::process::command(ffmpeg)
        .args(["-ss", "1", "-i"])
        .arg(&path)
        .args([
            "-frames:v", "1",
            "-vf", "scale=320:-2",
            "-f", "image2pipe",
            "-vcodec", "mjpeg",
            "-",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map_err(|_| AppError::EngineMissing)?;

    if !out.status.success() || out.stdout.is_empty() {
        return Ok(None);
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&out.stdout);
    Ok(Some(format!("data:image/jpeg;base64,{b64}")))
}

/// Upload a video to the user's YouTube channel. Returns the video id.
/// Send a video to the creator's TikTok inbox.
///
/// Returns TikTok's publish id. The video does NOT appear on the profile, and
/// it is NOT in Drafts either: TikTok sends the creator an inbox NOTIFICATION,
/// and they must tap it to finish editing and post. Saying "drafts" sends people
/// looking somewhere the video will not be.
#[tauri::command]
pub async fn upload_tiktok(
    manager: State<'_, AuthManager>,
    file_path: String,
) -> Result<String> {
    let cred = manager.access_token(ProviderId::TikTok).await?;

    // Fail with something actionable rather than letting TikTok reject the
    // first API call: the connection can be live while the scope is absent.
    if !cred
        .scopes
        .iter()
        .any(|s| s == "video.upload" || s == "video.publish")
    {
        return Err(AppError::ProviderDenied(
            "this TikTok login has no upload permission - reconnect TikTok on the Accounts page"
                .into(),
        ));
    }

    crate::tiktok::upload_to_inbox(
        &reqwest::Client::new(),
        &cred.access_token,
        std::path::Path::new(&file_path),
    )
    .await
}

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
