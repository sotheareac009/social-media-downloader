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
        can_install: cfg!(any(target_os = "macos", target_os = "windows")),
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

/// Pull one or more binaries out of a zip, flattened into `dir`.
///
/// Extracts in-process rather than shelling out to `unzip`: that binary lives
/// at `/usr/bin/unzip` on macOS and does not exist on Windows at all, so the
/// old approach could never have worked there.
///
/// Members are matched on FILE NAME, not full path. Windows ffmpeg builds nest
/// their binaries (`ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe`) while the
/// macOS ones sit at the root, and matching the leaf handles both.
async fn fetch_zip_binaries(url: &str, members: &[&str], dir: &Path) -> Result<()> {
    let bytes = fetch_bytes(url).await?;
    tokio::fs::create_dir_all(dir).await.ok();

    let wanted: Vec<String> = members.iter().map(|m| exe_name(m)).collect();
    // The closure needs its own copy; the original is checked afterwards to
    // confirm every requested binary actually turned up.
    let looking_for = wanted.clone();
    let dir = dir.to_path_buf();

    // `zip` is synchronous and these archives run to tens of MB, so unpack off
    // the async runtime.
    let found = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| AppError::Internal(format!("the archive could not be opened: {e}")))?;

        let mut found = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| AppError::Internal(format!("archive entry unreadable: {e}")))?;
            if entry.is_dir() {
                continue;
            }
            // `enclosed_name` rejects paths that escape the destination, so a
            // hostile archive cannot write outside `dir`.
            let Some(path) = entry.enclosed_name() else {
                continue;
            };
            let Some(leaf) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            if !looking_for.contains(&leaf) {
                continue;
            }

            let dest = dir.join(&leaf);
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| AppError::Internal(format!("could not write {leaf}: {e}")))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| AppError::Internal(format!("could not extract {leaf}: {e}")))?;
            drop(out);

            make_executable(&dest)?;
            found.push(leaf);
        }
        Ok(found)
    })
    .await
    .map_err(|e| AppError::Internal(format!("extraction task failed: {e}")))??;

    let missing: Vec<&String> = wanted.iter().filter(|w| !found.contains(w)).collect();
    if !missing.is_empty() {
        return Err(AppError::Internal(format!(
            "the download did not contain {missing:?}"
        )));
    }
    Ok(())
}

/// Platform-correct executable name.
fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Download whatever is missing into `bin_dir`, emitting progress as it goes.
/// Already-present tools are skipped, so re-running is cheap and safe.
pub async fn install(app: &AppHandle) -> Result<ToolsStatus> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return Err(AppError::Internal(
            "automatic setup is available on macOS and Windows; on Linux install \
             yt-dlp and ffmpeg with your package manager"
                .into(),
        ));
    }

    let dir = bin_dir(app)?;
    tokio::fs::create_dir_all(&dir).await.ok();
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

    // 1. yt-dlp - a single binary published by the project, no archive.
    if before.ytdlp {
        emit("yt-dlp", "skipped", 1, None);
    } else {
        emit("yt-dlp", "downloading", 1, None);
        let url = if cfg!(target_os = "windows") {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        } else {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
        };
        match fetch_to(url, &dir.join(exe_name("yt-dlp"))).await {
            Ok(()) => emit("yt-dlp", "installed", 1, None),
            Err(e) => {
                emit("yt-dlp", "failed", 1, Some(e.to_string()));
                return Err(e);
            }
        }
    }

    // 2 & 3. ffmpeg + ffprobe.
    if before.ffmpeg {
        emit("ffmpeg", "skipped", 2, None);
        emit("ffprobe", "skipped", 3, None);
    } else if cfg!(target_os = "windows") {
        // One archive carries both binaries, so download it once rather than
        // pulling ~100 MB twice. The macOS source has no Windows builds at all
        // - every path there 404s - so this uses BtbN's release instead.
        emit("ffmpeg", "downloading", 2, None);
        emit("ffprobe", "downloading", 3, None);
        let url = "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";
        match fetch_zip_binaries(url, &["ffmpeg", "ffprobe"], &dir).await {
            Ok(()) => {
                emit("ffmpeg", "installed", 2, None);
                emit("ffprobe", "installed", 3, None);
            }
            Err(e) => {
                emit("ffmpeg", "failed", 2, Some(e.to_string()));
                emit("ffprobe", "failed", 3, Some(e.to_string()));
                return Err(e);
            }
        }
    } else {
        let arch = mac_arch();
        for (i, member) in [(2u32, "ffmpeg"), (3u32, "ffprobe")] {
            emit(member, "downloading", i, None);
            let url = format!(
                "https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/{member}.zip"
            );
            match fetch_zip_binaries(&url, &[member], &dir).await {
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
