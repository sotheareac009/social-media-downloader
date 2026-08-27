//! Cutting one long video into equal parts.
//!
//! WHY THIS EXISTS. A one-hour recording is not publishable as it stands: every
//! short-form platform caps a post well below that, so the first thing anyone
//! does with a long file is chop it up. Doing that by hand means learning
//! ffmpeg's seek flags, and getting them subtly wrong produces clips that start
//! a few seconds late or carry a frozen first frame.
//!
//! HOW IT CUTS. Two modes, and the difference matters:
//!
//!   * **Fast** copies the existing streams (`-c copy`). No re-encoding, so a
//!     one-hour file splits in seconds with no quality loss at all - but a copy
//!     can only start on a keyframe, so a cut lands on the nearest keyframe at
//!     or before the mark. Drift is typically under a couple of seconds.
//!   * **Exact** re-encodes, so every part starts on the requested frame. It
//!     costs minutes rather than seconds and re-compresses the video once.
//!
//! Fast is the default because the drift is invisible for the thing people
//! actually do with these clips, and an hour of waiting is not.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::errors::{AppError, Result};

/// The most parts one file can be cut into.
///
/// Not a technical limit - it is the point where a typo ("600" for "6") stops
/// being recoverable by clicking cancel, since each part is a separate ffmpeg
/// run writing a separate file.
const MAX_PARTS: u32 = 200;

/// The shortest part worth producing. Below this a "clip" is a glitch.
const MIN_PART_SECONDS: f64 = 1.0;

/// Progress event name. One tick per part, plus a final one when the last part
/// lands, so a UI can show "3 of 6" without polling.
pub const PROGRESS_EVENT: &str = "convert://progress";

/// What ffprobe could tell us about a file the user dropped in.
#[derive(Debug, Clone, Serialize)]
pub struct VideoProbe {
    pub path: String,
    /// The file's own name, so the UI never has to parse a path.
    pub file_name: String,
    pub duration_seconds: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: u64,
    /// False for an audio-only file, which cannot be split into video clips.
    pub has_video: bool,
}

/// One finished part.
#[derive(Debug, Clone, Serialize)]
pub struct Clip {
    /// 1-based, as the filename spells it.
    pub index: u32,
    pub path: String,
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SplitResult {
    pub output_dir: String,
    pub clips: Vec<Clip>,
}

/// Emitted as each part starts and finishes.
#[derive(Debug, Clone, Serialize)]
pub struct SplitProgress {
    /// 1-based index of the part this tick is about.
    pub index: u32,
    pub total: u32,
    /// "cutting" | "done"
    pub state: String,
    /// Present once the part is written.
    pub path: Option<String>,
}

/// Locate ffmpeg, or say plainly that it is missing.
///
/// Shares the downloader's search, so "installed" means the same thing on both
/// screens - including the copy the app fetches into its own data directory on
/// first launch, which is not on any PATH.
fn ffmpeg_path() -> Result<PathBuf> {
    crate::download::ytdlp::locate_ffmpeg().ok_or(AppError::FfmpegMissing)
}

/// What the Convert tab needs to know before it offers a GPU toggle.
#[derive(Debug, Clone, Serialize)]
pub struct ConvertCapabilities {
    pub ffmpeg: bool,
    /// "Apple VideoToolbox", "NVIDIA NVENC", "CPU (x264)" - the encoder that
    /// would actually run, so the toggle never promises hardware the machine
    /// does not have.
    pub encoder_label: String,
    pub has_hardware: bool,
    /// Logical cores, to size the thread picker sensibly.
    pub cpu_threads: usize,
    pub default_threads: usize,
    pub max_threads: usize,
    /// Output formats this FFmpeg build can write, as extensions. Anything
    /// missing is left out of the picker rather than failing at conversion.
    pub video_formats: Vec<String>,
    pub photo_formats: Vec<String>,
}

/// Report FFmpeg and hardware-encoder availability.
#[tauri::command]
pub async fn convert_capabilities() -> Result<ConvertCapabilities> {
    let ffmpeg = crate::download::ytdlp::locate_ffmpeg();
    let encoder = match &ffmpeg {
        Some(path) => crate::convert::available_encoder(path).await,
        None => crate::convert::HardwareEncoder::None,
    };
    let (video_formats, photo_formats) = match &ffmpeg {
        Some(path) => crate::convert::encoders::writable_formats(path).await,
        None => (Vec::new(), Vec::new()),
    };
    Ok(ConvertCapabilities {
        ffmpeg: ffmpeg.is_some(),
        encoder_label: encoder.label().to_string(),
        has_hardware: encoder != crate::convert::HardwareEncoder::None,
        cpu_threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        default_threads: crate::convert::DEFAULT_THREADS,
        max_threads: crate::convert::MAX_THREADS,
        video_formats,
        photo_formats,
    })
}

/// Choose a folder of media to convert.
#[tauri::command]
pub async fn convert_pick_folder(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a folder of media")
        .pick_folder(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx
        .await
        .map_err(|_| AppError::Internal("picker closed".into()))?;
    let Some(dir) = picked else {
        return Ok(None);
    };
    let path = dir
        .into_path()
        .map_err(|e| AppError::DownloadPath(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Choose several video files at once.
///
/// The merge list is ordered, so this returns the picker's own order and does
/// not sort: the arrows in the list are what decides sequence, and quietly
/// alphabetising a selection would fight them.
#[tauri::command]
pub async fn convert_pick_videos(app: AppHandle) -> Result<Vec<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose videos")
        .add_filter(
            "Videos",
            &[
                "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "flv", "wmv",
                "3gp", "mts", "m2ts", "vob", "ogv", "asf", "divx", "f4v", "rm", "rmvb", "mxf",
            ],
        )
        .pick_files(move |f| {
            let _ = tx.send(f);
        });

    let picked = rx
        .await
        .map_err(|_| AppError::Internal("picker closed".into()))?;
    let mut out = Vec::new();
    for file in picked.unwrap_or_default() {
        out.push(
            file.into_path()
                .map_err(|e| AppError::DownloadPath(e.to_string()))?
                .display()
                .to_string(),
        );
    }
    Ok(out)
}

/// Choose where converted or split files should be written.
///
/// Separate from [`convert_pick_folder`] only for its title: picking media to
/// work on and picking where the results go are different questions, and a
/// dialog that says "Choose a folder of media" for the second one is how a
/// source folder ends up as the destination.
#[tauri::command]
pub async fn convert_pick_output_dir(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose where to save the results")
        .pick_folder(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx
        .await
        .map_err(|_| AppError::Internal("picker closed".into()))?;
    let Some(dir) = picked else {
        return Ok(None);
    };
    let path = dir
        .into_path()
        .map_err(|e| AppError::DownloadPath(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Walk dropped files and folders into table rows, probing each one.
#[tauri::command]
pub async fn convert_scan(paths: Vec<String>) -> Result<Vec<crate::convert::MediaItem>> {
    let ffmpeg = crate::download::ytdlp::locate_ffmpeg();
    Ok(crate::convert::scan_paths(paths, ffmpeg).await)
}

/// Join clips into one file, in the order given.
///
/// Shares the batch queue: a merge and a batch conversion would compete for
/// the same cores, and one progress bar cannot describe both.
#[tauri::command]
pub async fn convert_merge(
    app: AppHandle,
    queue: State<'_, std::sync::Arc<crate::convert::ConvertQueue>>,
    items: Vec<crate::convert::MediaItem>,
    output_path: String,
    format: Option<crate::convert::VideoFormat>,
    height: Option<u32>,
    // How to shape a mix of portrait and landscape clips, and what to do with
    // the space left over. Both default to the least surprising choice: the
    // first clip's shape, with nothing cropped.
    shape: Option<crate::convert::merge::Shape>,
    fit: Option<crate::convert::Fit>,
) -> Result<crate::convert::merge::MergeResult> {
    let format = format.unwrap_or_default();
    // Its own lane: merging is heavy, but it should not refuse a photo batch
    // running beside it, nor be refused by one.
    let lane = queue.lane("merge");
    lane.begin()?;

    let ffmpeg = crate::download::ytdlp::locate_ffmpeg();
    let encoder = match (&ffmpeg, format.takes_h264()) {
        (Some(path), true) => crate::convert::available_encoder(path).await,
        _ => crate::convert::HardwareEncoder::None,
    };

    let result = crate::convert::merge::merge(
        &app,
        &lane,
        &items,
        std::path::Path::new(&output_path),
        format,
        height,
        shape.unwrap_or_default(),
        fit.unwrap_or(crate::convert::Fit::Pad),
        encoder,
    )
    .await;
    lane.finish();
    result
}

/// Convert every item in `items`, `settings.threads` at a time.
///
/// Resolves when the batch finishes; the table updates from the per-file
/// events rather than from this return value.
#[tauri::command]
pub async fn convert_start(
    app: AppHandle,
    queue: State<'_, std::sync::Arc<crate::convert::ConvertQueue>>,
    items: Vec<crate::convert::MediaItem>,
    settings: crate::convert::ConvertSettings,
    // "video" or "photo" — the screen that started it. Each runs independently,
    // so a photo batch neither waits for nor cancels a video one.
    lane: Option<String>,
) -> Result<crate::convert::BatchDone> {
    let lane = queue.lane(lane.as_deref().unwrap_or("video"));
    crate::convert::run_batch(app, lane, items, settings).await
}

/// Stop after the files already running finish being killed.
#[tauri::command]
pub async fn convert_cancel(
    queue: State<'_, std::sync::Arc<crate::convert::ConvertQueue>>,
    lane: Option<String>,
) -> Result<()> {
    queue.lane(lane.as_deref().unwrap_or("video")).cancel();
    Ok(())
}

/// Pick one video file to split.
#[tauri::command]
pub async fn convert_pick_file(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a video to split")
        .add_filter(
            "Videos",
            &["mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "flv"],
        )
        .pick_file(move |f| {
            let _ = tx.send(f);
        });

    let picked = rx
        .await
        .map_err(|_| AppError::Internal("picker closed".into()))?;
    let Some(file) = picked else {
        return Ok(None);
    };
    let path = file
        .into_path()
        .map_err(|e| AppError::DownloadPath(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Read a file's duration and dimensions.
///
/// The duration is what makes the whole screen work: without it there is no
/// way to say what "6 parts" means, so a file ffprobe cannot read is refused
/// here rather than half-accepted.
#[tauri::command]
pub async fn convert_probe(path: String) -> Result<VideoProbe> {
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(AppError::DownloadPath(format!("no file at {path}")));
    }

    let ffmpeg = ffmpeg_path()?;
    let ffprobe =
        crate::download::compat::ffprobe_beside(&ffmpeg).ok_or(AppError::FfmpegMissing)?;

    let out = crate::process::command(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height:format=duration",
            "-of",
            "json",
        ])
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| AppError::FfmpegMissing)?;

    if !out.status.success() {
        return Err(AppError::NotAVideo);
    }

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::NotAVideo)?;

    let duration = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or(AppError::NotAVideo)?;

    let stream = v.get("streams").and_then(|s| s.get(0));
    let dim = |key: &str| {
        stream
            .and_then(|s| s.get(key))
            .and_then(|n| n.as_u64())
            .map(|n| n as u32)
    };

    Ok(VideoProbe {
        file_name: file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone()),
        duration_seconds: duration,
        width: dim("width"),
        height: dim("height"),
        size_bytes: std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0),
        has_video: stream.is_some(),
        path,
    })
}

/// Cut `path` into `parts` equal pieces.
///
/// Every part is one ffmpeg run rather than one pass of the segment muxer: the
/// muxer splits *at* keyframes, so asking it for six parts of a file whose
/// keyframes fall awkwardly can yield five or seven. Being told six and getting
/// seven is the kind of surprise this screen exists to avoid.
#[tauri::command]
pub async fn convert_split(
    app: AppHandle,
    path: String,
    // Exactly one of these decides the cut: `parts` for "give me 6 clips",
    // `part_seconds` for "give me clips of 30 seconds". Naming both is a
    // contradiction, so it is refused rather than silently resolved.
    parts: Option<u32>,
    part_seconds: Option<f64>,
    // `exact` re-encodes so each cut lands on the exact second, at the cost of
    // time; `output_dir` defaults to a folder beside the source file.
    exact: bool,
    output_dir: Option<String>,
) -> Result<SplitResult> {
    let probe = convert_probe(path.clone()).await?;
    if !probe.has_video {
        return Err(AppError::NotAVideo);
    }

    let plan = match (parts, part_seconds) {
        (Some(_), Some(_)) => {
            return Err(AppError::SplitCount(
                "Choose either a number of parts or a length per part, not both.".into(),
            ))
        }
        (Some(n), None) => plan_parts(probe.duration_seconds, n)?,
        (None, Some(seconds)) => plan_by_length(probe.duration_seconds, seconds)?,
        (None, None) => {
            return Err(AppError::SplitCount(
                "Say how many parts you want, or how long each one should be.".into(),
            ))
        }
    };
    // However the plan was reached, the count is what names the files and the
    // folder from here on.
    let parts = plan.len() as u32;
    let source = PathBuf::from(&path);
    let out_dir = match output_dir {
        Some(dir) => PathBuf::from(dir),
        None => default_output_dir(&source, parts),
    };
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| AppError::DownloadPath(format!("could not create {}: {e}", out_dir.display())))?;

    let ffmpeg = ffmpeg_path()?;
    // Keep the source container unless it is one that cannot hold a copied
    // stream cleanly; mp4 is the safe landing place for anything else.
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "mp4" | "mov" | "mkv" | "webm"))
        .unwrap_or_else(|| "mp4".to_string());
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".to_string());

    let mut clips = Vec::with_capacity(plan.len());
    for (i, (start, duration)) in plan.iter().enumerate() {
        let index = i as u32 + 1;
        let _ = app.emit(
            PROGRESS_EVENT,
            SplitProgress {
                index,
                total: parts,
                state: "cutting".into(),
                path: None,
            },
        );

        let out = out_dir.join(format!(
            "{} - part {} of {}.{}",
            sanitise(&stem),
            index,
            parts,
            ext
        ));
        cut_one(&ffmpeg, &source, &out, *start, *duration, exact).await?;

        let clip = Clip {
            index,
            path: out.display().to_string(),
            start_seconds: *start,
            duration_seconds: *duration,
            size_bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        };
        let _ = app.emit(
            PROGRESS_EVENT,
            SplitProgress {
                index,
                total: parts,
                state: "done".into(),
                path: Some(clip.path.clone()),
            },
        );
        clips.push(clip);
    }

    Ok(SplitResult {
        output_dir: out_dir.display().to_string(),
        clips,
    })
}

/// Run one cut.
async fn cut_one(
    ffmpeg: &Path,
    source: &Path,
    out: &Path,
    start: f64,
    duration: f64,
    exact: bool,
) -> Result<()> {
    let mut cmd = crate::process::command(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);

    if exact {
        // Output seeking: ffmpeg decodes up to the mark and cuts on the exact
        // frame. Slower, and only meaningful because the streams are re-encoded
        // immediately afterwards.
        cmd.arg("-i").arg(source).arg("-ss").arg(fmt(start));
    } else {
        // Input seeking: ffmpeg jumps straight to the nearest keyframe, which
        // is what makes a copy near-instant.
        cmd.arg("-ss").arg(fmt(start)).arg("-i").arg(source);
    }

    cmd.arg("-t").arg(fmt(duration));

    if exact {
        cmd.args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-c:a", "aac", "-b:a", "192k",
        ]);
    } else {
        cmd.args(["-c", "copy"]);
    }

    // Without this a copied part can open with negative timestamps, which some
    // players show as a frozen first frame.
    cmd.args(["-avoid_negative_ts", "make_zero", "-map_metadata", "0"]);
    cmd.arg(out).stdin(Stdio::null());

    let result = cmd.output().await.map_err(|_| AppError::FfmpegMissing)?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail = stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ffmpeg failed");
        return Err(AppError::SplitFailed(detail.chars().take(200).collect()));
    }
    Ok(())
}

/// Where the parts go when the user hasn't chosen: a folder beside the source,
/// named after it. Six new files loose in the same directory as the original is
/// how a Downloads folder becomes unusable.
fn default_output_dir(source: &Path, parts: u32) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".to_string());
    let parent = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    parent.join(format!("{} ({} parts)", sanitise(&stem), parts))
}

/// Start and length of every part, in seconds.
///
/// The last part runs to the end of the file rather than to its computed
/// length: an hour never divides evenly, and rounding each part down loses the
/// final fraction of a second - which is exactly where a video's last frame is.
fn plan_parts(duration: f64, parts: u32) -> Result<Vec<(f64, f64)>> {
    if parts < 2 {
        return Err(AppError::SplitCount(
            "Choose at least 2 parts — one part is the file you already have.".into(),
        ));
    }
    if parts > MAX_PARTS {
        return Err(AppError::SplitCount(format!(
            "That's more than {MAX_PARTS} parts. Pick a smaller number."
        )));
    }

    let each = duration / parts as f64;
    if each < MIN_PART_SECONDS {
        return Err(AppError::SplitCount(format!(
            "{parts} parts would be under a second each. This video is {} long.",
            human_duration(duration)
        )));
    }

    Ok((0..parts)
        .map(|i| {
            let start = each * i as f64;
            // Overshooting the end is harmless - ffmpeg stops at the last frame
            // - and it is what keeps the final part whole.
            let len = if i + 1 == parts {
                duration - start + 1.0
            } else {
                each
            };
            (start, len)
        })
        .collect())
}

/// Start and length of every part, when the user names a clip length instead
/// of a number of clips.
///
/// "30 seconds each" of a 30-minute video is 60 clips, and the count is worked
/// out here rather than asked for. The awkward case is the remainder: 30
/// minutes divides evenly, 31 does not, and the leftover 60 seconds is a real
/// clip that must not be dropped.
///
/// A leftover shorter than [`MIN_PART_SECONDS`] is folded into the previous
/// part instead of shipped as its own file - a half-second clip is a glitch,
/// not a video, and it is better to have one part slightly long than one part
/// nobody can watch.
fn plan_by_length(duration: f64, each: f64) -> Result<Vec<(f64, f64)>> {
    if each < MIN_PART_SECONDS {
        return Err(AppError::SplitCount(
            "Each part has to be at least a second long.".into(),
        ));
    }
    if each >= duration {
        return Err(AppError::SplitCount(format!(
            "That's as long as the video itself ({}). Choose a shorter length.",
            human_duration(duration)
        )));
    }

    let whole = (duration / each).floor() as u32;
    let remainder = duration - (whole as f64 * each);
    let parts = if remainder >= MIN_PART_SECONDS {
        whole + 1
    } else {
        whole
    };

    if parts > MAX_PARTS {
        return Err(AppError::SplitCount(format!(
            "{} parts of {} would be {parts} files — more than the {MAX_PARTS} limit. Choose a longer part.",
            human_duration(duration),
            human_duration(each),
        )));
    }

    Ok((0..parts)
        .map(|i| {
            let start = each * i as f64;
            // The last part runs to the end whatever its computed length: it
            // carries the remainder, and overshooting is harmless because
            // ffmpeg stops at the final frame.
            let len = if i + 1 == parts {
                duration - start + 1.0
            } else {
                each
            };
            (start, len)
        })
        .collect())
}

/// Seconds as ffmpeg wants them: plain decimal, millisecond resolution.
fn fmt(seconds: f64) -> String {
    format!("{seconds:.3}")
}

/// A rough "1 h 3 min" for an error message.
fn human_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min {s} s")
    } else {
        format!("{s} s")
    }
}

/// Strip what a filesystem will not take, so a title with a slash in it cannot
/// silently create a directory.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "video".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_parts_of_an_hour_are_ten_minutes_each() {
        let plan = plan_parts(3600.0, 6).unwrap();
        assert_eq!(plan.len(), 6);
        for (i, (start, len)) in plan.iter().enumerate() {
            assert_eq!(*start, 600.0 * i as f64, "part {i}");
            if i < 5 {
                assert_eq!(*len, 600.0, "part {i}");
            }
        }
    }

    #[test]
    fn the_last_part_runs_past_the_end_so_no_frame_is_lost() {
        // 100s in 3 parts is 33.33 each; stopping the last one at its computed
        // length would drop the final fraction of a second.
        let plan = plan_parts(100.0, 3).unwrap();
        let (start, len) = plan[2];
        assert!(start + len > 100.0, "last part stops short: {start} + {len}");
    }

    #[test]
    fn every_part_starts_where_the_previous_one_was_asked_to_end() {
        let plan = plan_parts(1234.0, 7).unwrap();
        for pair in plan.windows(2) {
            let (start, len) = pair[0];
            let (next_start, _) = pair[1];
            assert!(
                (start + len - next_start).abs() < 0.001,
                "gap between {start}+{len} and {next_start}"
            );
        }
    }

    #[test]
    fn counts_that_cannot_produce_watchable_clips_are_refused() {
        // One part is the file you already have.
        assert!(plan_parts(3600.0, 1).is_err());
        assert!(plan_parts(3600.0, 0).is_err());
        // 30 seconds into 60 parts is half a second each.
        assert!(plan_parts(30.0, 60).is_err());
        assert!(plan_parts(3600.0, MAX_PARTS + 1).is_err());
        // The boundary itself is fine.
        assert!(plan_parts(3600.0, MAX_PARTS).is_ok());
        assert!(plan_parts(3600.0, 2).is_ok());
    }

    #[test]
    fn thirty_second_clips_of_a_thirty_minute_video_are_sixty_files() {
        let plan = plan_by_length(1800.0, 30.0).unwrap();
        assert_eq!(plan.len(), 60);
        assert_eq!(plan[0].0, 0.0);
        assert_eq!(plan[0].1, 30.0);
        assert_eq!(plan[59].0, 1770.0);
    }

    #[test]
    fn a_remainder_worth_watching_becomes_its_own_part() {
        // 31 minutes in 30s clips: 62 whole ones and a 60-second leftover.
        let plan = plan_by_length(1860.0, 30.0).unwrap();
        assert_eq!(plan.len(), 62);
        let (start, len) = plan[61];
        assert_eq!(start, 1830.0);
        assert!(start + len >= 1860.0, "the tail was dropped: {start}+{len}");
    }

    #[test]
    fn a_remainder_too_short_to_watch_is_folded_into_the_part_before_it() {
        // 60.4s in 30s clips: the 0.4s tail is not a video, so 2 parts, and
        // the last one carries it.
        let plan = plan_by_length(60.4, 30.0).unwrap();
        assert_eq!(plan.len(), 2);
        let (start, len) = plan[1];
        assert!(start + len >= 60.4, "the tail was dropped: {start}+{len}");
    }

    #[test]
    fn a_length_that_cannot_produce_a_split_is_refused() {
        // Longer than the video, and equal to it: both mean "no split".
        assert!(plan_by_length(600.0, 900.0).is_err());
        assert!(plan_by_length(600.0, 600.0).is_err());
        // Sub-second clips.
        assert!(plan_by_length(600.0, 0.5).is_err());
        // 10s clips of a two-hour file is 720 files - past the limit, and the
        // message has to say so rather than starting 720 encodes.
        assert!(plan_by_length(7200.0, 10.0).is_err());
    }

    #[test]
    fn both_ways_of_asking_agree_when_they_describe_the_same_split() {
        let by_count = plan_parts(3600.0, 6).unwrap();
        let by_length = plan_by_length(3600.0, 600.0).unwrap();
        assert_eq!(by_count.len(), by_length.len());
        for (a, b) in by_count.iter().zip(by_length.iter()) {
            assert!((a.0 - b.0).abs() < 0.001, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_name_with_a_slash_cannot_become_a_directory() {
        assert_eq!(sanitise("AC/DC live"), "AC-DC live");
        assert_eq!(sanitise("  "), "video");
        assert_eq!(sanitise("normal name"), "normal name");
    }

    #[test]
    fn parts_land_in_their_own_folder_beside_the_source() {
        let dir = default_output_dir(Path::new("/tmp/holiday.mp4"), 6);
        assert_eq!(dir, PathBuf::from("/tmp/holiday (6 parts)"));
    }

    #[test]
    fn seconds_are_formatted_for_ffmpeg_not_for_people() {
        assert_eq!(fmt(600.0), "600.000");
        assert_eq!(fmt(1.5), "1.500");
    }
}
