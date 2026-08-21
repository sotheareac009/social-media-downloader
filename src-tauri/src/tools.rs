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
    /// Bytes received so far for the current download.
    pub downloaded_bytes: u64,
    /// Total size when the server declares Content-Length. Absent for a
    /// chunked response, in which case the UI shows bytes without a bar.
    pub total_bytes: Option<u64>,
    /// Recent throughput. Measured over the gap between emits rather than the
    /// whole download, so it reflects the connection now instead of an average
    /// dragged down by a slow start.
    pub bytes_per_sec: Option<u64>,
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
/// Throttles progress reporting and works out throughput.
///
/// A 163 MB download emits thousands of chunks; forwarding every one would
/// flood the IPC channel and make the UI worse, not better. Emitting on a
/// fixed interval keeps it readable and cheap.
struct Reporter<F: FnMut(u64, Option<u64>, Option<u64>)> {
    emit: F,
    last_emit: std::time::Instant,
    last_bytes: u64,
}

impl<F: FnMut(u64, Option<u64>, Option<u64>)> Reporter<F> {
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

    fn new(emit: F) -> Self {
        Self {
            emit,
            last_emit: std::time::Instant::now(),
            last_bytes: 0,
        }
    }

    fn tick(&mut self, downloaded: u64, total: Option<u64>, force: bool) {
        let elapsed = self.last_emit.elapsed();
        if !force && elapsed < Self::INTERVAL {
            return;
        }
        // Speed over this window only. `saturating_sub` guards the reset that
        // happens when a retry restarts the download from zero.
        let speed = if elapsed.as_secs_f64() > 0.0 {
            Some((downloaded.saturating_sub(self.last_bytes) as f64 / elapsed.as_secs_f64()) as u64)
        } else {
            None
        };
        (self.emit)(downloaded, total, speed);
        self.last_emit = std::time::Instant::now();
        self.last_bytes = downloaded;
    }
}

/// Download `url`, reporting progress as the body arrives.
///
/// Streams rather than buffering the whole body: `bytes()` returns nothing
/// until the transfer finishes, so there is no way to show a user how far
/// through a 163 MB archive they are.
async fn fetch_bytes(
    url: &str,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, Option<u64>) + Send),
) -> Result<Vec<u8>> {
    use futures_util::StreamExt;

    const ATTEMPTS: usize = 5;
    let mut last = String::from("no attempt made");

    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(700 * attempt as u64)).await;
        }

        let resp = match reqwest::get(url).await {
            Ok(r) => match r.error_for_status() {
                Ok(ok) => ok,
                Err(e) => {
                    last = format!("server said {e}");
                    continue;
                }
            },
            Err(e) => {
                last = format!("request failed: {e}");
                continue;
            }
        };

        let total = resp.content_length();
        let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut reporter = Reporter::new(|d, t, s| on_progress(d, t, s));
        let mut stream = resp.bytes_stream();
        let mut failed = None;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    reporter.tick(buf.len() as u64, total, false);
                }
                Err(e) => {
                    failed = Some(format!("interrupted: {e}"));
                    break;
                }
            }
        }

        if let Some(e) = failed {
            last = e;
            continue;
        }
        if buf.is_empty() {
            last = "empty response".into();
            continue;
        }

        reporter.tick(buf.len() as u64, total, true);
        return Ok(buf);
    }

    Err(AppError::Internal(format!(
        "download failed after {ATTEMPTS} tries ({last})"
    )))
}

/// Download `url` and write it to `dest`, then mark it executable. Whole-body
/// (not streamed) because these are tens of MB and it keeps the code simple.
async fn fetch_to(
    url: &str,
    dest: &Path,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, Option<u64>) + Send),
) -> Result<()> {
    let bytes = fetch_bytes(url, on_progress).await?;
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
async fn fetch_zip_binaries(
    url: &str,
    members: &[&str],
    dir: &Path,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, Option<u64>) + Send),
) -> Result<()> {
    let bytes = fetch_bytes(url, on_progress).await?;
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
                downloaded_bytes: 0,
                total_bytes: None,
                bytes_per_sec: None,
            },
        );
    };

    // A per-tool byte reporter. Built fresh for each download so the numbers
    // always describe the file currently in flight.
    let progress = |tool: &'static str, step: u32| {
        let app = app.clone();
        move |downloaded: u64, total_bytes: Option<u64>, bytes_per_sec: Option<u64>| {
            let _ = app.emit(
                "tools://progress",
                ToolsProgress {
                    tool: tool.into(),
                    state: "downloading".into(),
                    step,
                    total,
                    error: None,
                    downloaded_bytes: downloaded,
                    total_bytes,
                    bytes_per_sec,
                },
            );
        }
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
        match fetch_to(url, &dir.join(exe_name("yt-dlp")), &mut progress("yt-dlp", 1)).await {
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
        match fetch_zip_binaries(url, &["ffmpeg", "ffprobe"], &dir, &mut progress("ffmpeg", 2)).await {
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
            match fetch_zip_binaries(&url, &[member], &dir, &mut progress(member, i)).await {
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
