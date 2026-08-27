//! Tauri commands for the downloader.
//!
//! As in `commands::auth`, nothing returned here can carry a secret: a
//! `JobView` holds a title, a size and a local path. The signed CDN URL the
//! engine fetches from never leaves Rust.

use std::sync::Arc;

use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::download::manager::{run_job, Destination, DownloadManager, EngineStatus, JobView};
use crate::download::quality::Quality;
use crate::download::session::{SessionKind, SessionStatus, StoredCookie, WebSession};
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
    /// Prefer H.264 so files open in QuickTime and Photos.
    pub prefer_compatible: bool,
}

const IG_LOGIN_URL: &str = "https://www.instagram.com/accounts/login/";
const FB_LOGIN_URL: &str = "https://www.facebook.com/login/";
const X_LOGIN_URL: &str = "https://x.com/login";

/// Static per-platform login details.
struct LoginTarget {
    kind: SessionKind,
    label: &'static str,
    url: &'static str,
    title: &'static str,
    /// Fallback cookie domain when the webview omits one.
    default_domain: &'static str,
    /// For the timeout diagnostic.
    platform: &'static str,
}

const INSTAGRAM_TARGET: LoginTarget = LoginTarget {
    kind: SessionKind::Instagram,
    label: "instagram-login",
    url: IG_LOGIN_URL,
    title: "Sign in to Instagram",
    default_domain: ".instagram.com",
    platform: "Instagram",
};

const FACEBOOK_TARGET: LoginTarget = LoginTarget {
    kind: SessionKind::Facebook,
    label: "facebook-login",
    url: FB_LOGIN_URL,
    title: "Sign in to Facebook",
    default_domain: ".facebook.com",
    platform: "Facebook",
};

const X_TARGET: LoginTarget = LoginTarget {
    kind: SessionKind::X,
    label: "x-login",
    url: X_LOGIN_URL,
    title: "Sign in to X",
    default_domain: ".x.com",
    platform: "X",
};

/// Open a dedicated login window for `target` and capture its session.
///
/// A separate window rather than `--cookies-from-browser`: that flag would
/// hand the engine the user's entire browser profile - every site they are
/// signed into. This reads cookies from one window, that the user opened
/// deliberately, and keeps only this platform's.
///
/// Resolves when the platform's login cookie(s) appear. Closing the window
/// first cancels the flow.
async fn capture_session(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    target: LoginTarget,
) -> Result<SessionStatus> {
    // Reusing a stale window would show a page from a previous attempt.
    if let Some(existing) = app.get_webview_window(target.label) {
        let _ = existing.close();
    }

    let url = target
        .url
        .parse()
        .map_err(|_| AppError::Internal("bad login url".into()))?;

    let window = WebviewWindowBuilder::new(app, target.label, WebviewUrl::External(url))
        .title(target.title)
        .inner_size(480.0, 760.0)
        .resizable(true)
        .focused(true)
        .build()
        .map_err(|e| AppError::Internal(format!("could not open the login window: {e}")))?;

    // Poll rather than hook navigation: these logins are single-page apps that
    // finish without a page load we could listen for, and the cookie appearing
    // is the only reliable signal that it worked.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut last_error: Option<String> = None;
    let mut seen_cookie_names: Vec<String> = Vec::new();

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        if app.get_webview_window(target.label).is_none() {
            return Err(AppError::Cancelled);
        }
        if std::time::Instant::now() > deadline {
            let _ = window.close();
            // Names only - never values - so a failure is diagnosable without
            // a session cookie ending up in a log or an error message.
            return Err(AppError::Internal(format!(
                "signed-in session not detected after 5 minutes. {} cookies seen: [{}].{}",
                target.platform,
                seen_cookie_names.join(", "),
                last_error
                    .map(|e| format!(" Last cookie-read error: {e}"))
                    .unwrap_or_default()
            )));
        }

        // Deliberately NOT `cookies_for_url`: on macOS wry compares the
        // cookie's domain to the URL host by exact equality, so a `.x.com`
        // cookie never matches `www.x.com` and the jar comes back empty. Ask
        // for everything and match the domain here, where a leading dot is
        // handled.
        let jar = match window.cookies() {
            Ok(jar) => jar,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        };

        let cookies: Vec<StoredCookie> = jar
            .iter()
            .filter(|c| c.domain().map(|d| target.kind.domain_matches(d)).unwrap_or(false))
            .map(|c| StoredCookie {
                name: c.name().to_string(),
                value: c.value().to_string(),
                domain: c
                    .domain()
                    .map(|d| if d.starts_with('.') { d.to_string() } else { format!(".{d}") })
                    .unwrap_or_else(|| target.default_domain.to_string()),
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

        let candidate = WebSession {
            cookies,
            captured_at: crate::auth::now_unix(),
        };
        if candidate.is_usable_for(target.kind) {
            manager.session_remember(target.kind, &candidate)?;
            // Best-effort: fetch the display name/avatar. A failure here leaves
            // the account connected but nameless, never blocks the login.
            if let Some(profile) =
                crate::download::profile::fetch(target.kind, &candidate.cookies).await
            {
                let _ = manager.session_set_profile(target.kind, &profile);
            }
            let status = manager.session_status(target.kind);
            let _ = window.close();
            return Ok(status);
        }
    }
}

/// What a liveness check concluded.
#[derive(serde::Serialize)]
pub struct CookieCheck {
    /// True only when the platform answered as a signed-in user.
    pub alive: bool,
    /// Plain-language outcome, safe to show. Never quotes a cookie.
    pub message: String,
    /// The account the cookies belong to, when the platform said.
    pub display_name: Option<String>,
    /// When the login cookies stop working, if they state it.
    pub expires_at: Option<i64>,
}

fn session_kind(name: &str) -> Result<SessionKind> {
    match name {
        "instagram" => Ok(SessionKind::Instagram),
        "facebook" => Ok(SessionKind::Facebook),
        "tiktok" => Ok(SessionKind::TikTok),
        "x" => Ok(SessionKind::X),
        other => Err(AppError::UnknownProvider(other.to_string())),
    }
}

/// Store cookies pasted from a browser export.
///
/// The alternative to the login window: some accounts cannot complete a login
/// inside an embedded webview at all - a checkpoint, two-factor, or a
/// "suspicious device" wall - and for those, cookies already obtained in a
/// real browser are the only way through.
#[tauri::command]
pub async fn download_session_import_cookies(
    manager: State<'_, Arc<DownloadManager>>,
    platform: String,
    text: String,
) -> Result<SessionStatus> {
    let kind = session_kind(&platform)?;
    // Parsing rejects a paste that is for another site, or that carries only
    // the cookies an anonymous visitor has.
    let session = crate::download::session::parse_netscape(&text, kind)?;
    manager.session_remember(kind, &session)?;

    // Best-effort, exactly as the login window does it: a name makes the card
    // readable, and a failure here never invalidates working cookies.
    if let Some(profile) = crate::download::profile::fetch(kind, &session.cookies).await {
        let _ = manager.session_set_profile(kind, &profile);
    }
    Ok(manager.session_status(kind))
}

/// Ask the platform whether the stored cookies still work.
///
/// Two steps, cheapest first: a stated expiry in the past is a definite answer
/// with no request at all. Otherwise the platform is asked, and only a reply
/// that names the account counts as alive - a page that loads for logged-out
/// visitors proves nothing.
#[tauri::command]
pub async fn download_session_check(
    manager: State<'_, Arc<DownloadManager>>,
    platform: String,
) -> Result<CookieCheck> {
    let kind = session_kind(&platform)?;
    let Some(session) = manager.session_for(kind) else {
        return Ok(CookieCheck {
            alive: false,
            message: format!("No {} cookies saved yet.", kind.display_name()),
            display_name: None,
            expires_at: None,
        });
    };

    let expires_at = kind.soonest_required_expiry(&session);
    let now = crate::auth::now_unix();
    if let Some(expiry) = expires_at {
        if expiry <= now {
            return Ok(CookieCheck {
                alive: false,
                message: "These cookies have expired — export them again.".into(),
                display_name: None,
                expires_at,
            });
        }
    }

    use crate::download::profile::SessionCheck;
    match crate::download::profile::check(kind, &session.cookies).await {
        SessionCheck::SignedIn(profile) => {
            // Worth keeping: a check is also the cheapest chance to put a name
            // on a card that was saved without one.
            let _ = manager.session_set_profile(kind, &profile);
            Ok(CookieCheck {
                alive: true,
                message: match &profile.display_name {
                    Some(name) => format!("Signed in as {name}."),
                    None => "Cookies still work.".into(),
                },
                display_name: profile.display_name,
                expires_at,
            })
        }
        SessionCheck::Rejected => Ok(CookieCheck {
            alive: false,
            message: format!(
                "{} rejected these cookies — export them again while signed in.",
                kind.display_name()
            ),
            display_name: None,
            expires_at,
        }),
        // Reported as its own outcome rather than as a failure. Saying
        // "invalid" here sends people off to re-export cookies that were fine,
        // which is worse than admitting the check could not tell.
        SessionCheck::Unknown => Ok(CookieCheck {
            alive: true,
            message: format!(
                "Couldn't confirm with {} — the cookies are stored and will be used for downloads. If a download says it needs a login, export them again.",
                kind.display_name()
            ),
            display_name: None,
            expires_at,
        }),
    }
}

/// The stored session for a platform, for a card that renders itself.
#[tauri::command]
pub async fn download_session_status(
    manager: State<'_, Arc<DownloadManager>>,
    platform: String,
) -> Result<SessionStatus> {
    Ok(manager.session_status(session_kind(&platform)?))
}

/// Forget the stored cookies for a platform.
///
/// Deletes the session file rather than blanking it: a jar that is "cleared"
/// but still on disk is the kind of thing that turns up in a backup later.
#[tauri::command]
pub async fn download_session_clear(
    manager: State<'_, Arc<DownloadManager>>,
    platform: String,
) -> Result<SessionStatus> {
    manager.session_forget(session_kind(&platform)?)
}

#[tauri::command]
pub async fn download_instagram_connect(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    capture_session(&app, &manager, INSTAGRAM_TARGET).await
}

#[tauri::command]
pub async fn download_facebook_connect(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    capture_session(&app, &manager, FACEBOOK_TARGET).await
}

/// Cheap by design: reads a non-secret marker, so rendering a page never
/// touches the session file.
#[tauri::command]
pub async fn download_instagram_status(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    Ok(manager.instagram_status())
}

#[tauri::command]
pub async fn download_facebook_status(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    Ok(manager.facebook_status())
}

#[tauri::command]
pub async fn download_facebook_disconnect(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    manager.facebook_forget()
}

/// Forget the stored session. The window's own cookies are not this app's to
/// clear, so the user is told to sign out in the browser too if they want that.
#[tauri::command]
pub async fn download_instagram_disconnect(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    manager.instagram_forget()
}

#[tauri::command]
pub async fn download_x_connect(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    capture_session(&app, &manager, X_TARGET).await
}

#[tauri::command]
pub async fn download_x_status(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    Ok(manager.x_status())
}

#[tauri::command]
pub async fn download_x_disconnect(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<SessionStatus> {
    manager.x_forget()
}

#[tauri::command]
pub async fn download_get_quality(
    manager: State<'_, Arc<DownloadManager>>,
) -> Result<QualitySettings> {
    let has_ffmpeg = crate::download::ytdlp::locate_ffmpeg().is_some();
    Ok(QualitySettings {
        selected: manager.quality(),
        has_ffmpeg,
        prefer_compatible: manager.prefer_compatible(),
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
pub async fn download_set_compatible(
    manager: State<'_, Arc<DownloadManager>>,
    on: bool,
) -> Result<bool> {
    manager.set_prefer_compatible(on)
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
