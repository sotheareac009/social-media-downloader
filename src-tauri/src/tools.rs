//! First-launch setup: fetch the command-line tools the app needs so a user
//! never has to run `brew install` themselves.
//!
//! WHY THIS EXISTS. yt-dlp and ffmpeg are separate programs this app shells out
//! to. On the developer's Mac they arrive via Homebrew, but a person who just
//! double-clicks the .dmg on another computer has neither. Rather than depend on
//! Homebrew (which many Macs lack, and whose install needs admin + Xcode CLT),
//! we download the standalone binaries once into the app's own data directory,
//! which needs no privileges and no package manager.
//!
//! WHERE THEY GO. `~/Library/Application Support/<bundle-id>/bin`. That folder is
//! prepended to `PATH` at startup (see `lib.rs`), so the existing `locate()`
//! search finds them with no special-casing.
//!
//! SCOPE. macOS only for now: the URLs and unzip step below are mac-specific.
//! On other platforms `install()` reports that auto-setup isn't available and
//! the manual notice stands.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::errors::{AppError, Result};

/// The directory we install downloaded tools into.
pub fn bin_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("no app data dir: {e}")))?
        .join("bin");
    Ok(dir)
}

/// What the frontend needs to decide whether to run setup.
#[derive(Serialize, Clone)]
pub struct ToolsStatus {
    /// yt-dlp is present (anywhere on the resolved search path).
    pub ytdlp: bool,
    /// ffmpeg *and* ffprobe are present — both are needed for merging/thumbnails.
    pub ffmpeg: bool,
    /// Nothing left to install for the core download path.
    pub ready: bool,
    /// True on platforms where `install()` can actually fetch the tools.
    pub can_install: bool,
}

/// Inspect the current tool situation using the same locators the downloader
/// uses, so "ready" here means "downloads will work".
pub fn status() -> ToolsStatus {
    let ytdlp = crate::download::ytdlp::locate().is_some();
    let ffmpeg = crate::download::ytdlp::locate_ffmpeg()
        .as_deref()
        .and_then(crate::download::compat::ffprobe_beside)
        .is_some();
    ToolsStatus {
        ytdlp,
        ffmpeg,
        ready: ytdlp && ffmpeg,
        can_install: cfg!(target_os = "macos"),
    }
}

/// One progress tick, emitted on `tools://progress` as install runs.
#[derive(Serialize, Clone)]
pub struct ToolsProgress {
    /// "yt-dlp", "ffmpeg", "ffprobe".
    pub tool: String,
    /// "downloading" | "installed" | "skipped" | "failed".
    pub state: String,
    /// 1-based position and total, for a simple "2 of 3".
    pub step: u32,
    pub total: u32,
    /// Present when state is "failed".
    pub error: Option<String>,
}

/// The download slice for the running architecture, matching how macOS picks
/// the native slice out of our universal binary.
fn mac_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

/// GET `url` fully into memory, retrying transient failures.
///
/// The static-build mirrors we use sit behind load balancers that occasionally
/// answer a valid URL with a 404 (observed: the same link 404s, then 200s on
/// the next try). A few spaced retries turn that flakiness into a reliable
/// install instead of a coin toss on the user's first launch.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    const ATTEMPTS: usize = 5;
    let mut last = String::from("no attempt made");
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(700 * attempt as u64)).await;
        }
        match reqwest::get(url).await {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok) => match ok.bytes().await {
                    Ok(b) if !b.is_empty() => return Ok(b.to_vec()),
                    Ok(_) => last = "empty response".into(),
                    Err(e) => last = format!("interrupted: {e}"),
                },
                Err(e) => last = format!("server said {e}"),
            },
            Err(e) => last = format!("request failed: {e}"),
        }
    }
    Err(AppError::Internal(format!(
        "download failed after {ATTEMPTS} tries ({last})"
    )))
}

/// Download `url` and write it to `dest`, then mark it executable. Whole-body
/// (not streamed) because these are tens of MB and it keeps the code simple.
async fn fetch_to(url: &str, dest: &Path) -> Result<()> {
    let bytes = fetch_bytes(url).await?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    tokio::fs::write(dest, &bytes)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {e}")))?;
    make_executable(dest)?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| AppError::Internal(format!("stat failed: {e}")))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|e| AppError::Internal(format!("chmod failed: {e}")))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Fetch a zip whose single payload is `member` (ffmpeg or ffprobe), and drop
/// that one file, flattened, into `dir`. Uses the system `unzip`, which every
/// mac has, rather than pulling in a zip crate for one call.
async fn fetch_zip_binary(url: &str, member: &str, dir: &Path) -> Result<()> {
    let bytes = fetch_bytes(url).await?;

    tokio::fs::create_dir_all(dir).await.ok();
    let tmp = dir.join(format!(".{member}.zip"));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {e}")))?;

    // -o overwrite, -j junk paths (flatten), -d destination.
    let out = tokio::process::Command::new("/usr/bin/unzip")
        .args(["-o", "-j"])
        .arg(&tmp)
        .arg("-d")
        .arg(dir)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("unzip failed to start: {e}")))?;
    tokio::fs::remove_file(&tmp).await.ok();
    if !out.status.success() {
        return Err(AppError::Internal(format!(
            "unzip failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let extracted = dir.join(member);
    if !extracted.is_file() {
        return Err(AppError::Internal(format!(
            "{member} not found in the downloaded archive"
        )));
    }
    make_executable(&extracted)
}

/// Download whatever is missing into `bin_dir`, emitting progress as it goes.
/// Already-present tools are skipped, so re-running is cheap and safe.
pub async fn install(app: &AppHandle) -> Result<ToolsStatus> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::Internal(
            "automatic setup is only available on macOS right now".into(),
        ));
    }

    let dir = bin_dir(app)?;
    tokio::fs::create_dir_all(&dir).await.ok();
    let arch = mac_arch();
    let before = status();
    let total = 3u32;

    let emit = |tool: &str, state: &str, step: u32, error: Option<String>| {
        let _ = app.emit(
            "tools://progress",
            ToolsProgress {
                tool: tool.into(),
                state: state.into(),
                step,
                total,
                error,
            },
        );
    };

    // 1. yt-dlp — a single universal binary published by the project.
    if before.ytdlp {
        emit("yt-dlp", "skipped", 1, None);
    } else {
        emit("yt-dlp", "downloading", 1, None);
        let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos";
        match fetch_to(url, &dir.join("yt-dlp")).await {
            Ok(()) => emit("yt-dlp", "installed", 1, None),
            Err(e) => {
                emit("yt-dlp", "failed", 1, Some(e.to_string()));
                return Err(e);
            }
        }
    }

    // 2 & 3. ffmpeg + ffprobe — per-arch static builds, one zip each.
    if before.ffmpeg {
        emit("ffmpeg", "skipped", 2, None);
        emit("ffprobe", "skipped", 3, None);
    } else {
        for (i, member) in [(2u32, "ffmpeg"), (3u32, "ffprobe")] {
            emit(member, "downloading", i, None);
            let url = format!(
                "https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/{member}.zip"
            );
            match fetch_zip_binary(&url, member, &dir).await {
                Ok(()) => emit(member, "installed", i, None),
                Err(e) => {
                    emit(member, "failed", i, Some(e.to_string()));
                    return Err(e);
                }
            }
        }
    }

    Ok(status())
}
