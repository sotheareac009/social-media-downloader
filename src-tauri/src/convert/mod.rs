//! Batch media conversion: many files, a few at a time, through FFmpeg.
//!
//! WHAT THIS IS FOR. A folder of downloads is not a folder of publishable
//! media. Clips arrive at whatever resolution and frame rate their source used,
//! and an emulator or a phone wants one consistent shape. Converting them one
//! at a time by hand is the job this replaces.
//!
//! HOW IT RUNS. Each file is one FFmpeg process, and several run at once - the
//! thread count. FFmpeg already threads its own encoding, so this is not "more
//! threads is faster": past a handful of concurrent files they compete for the
//! same cores and everything slows down together. The default is deliberately
//! low.
//!
//! WHAT IT WILL NOT DO. It never upscales. Asking for 1080p from a 720p source
//! produces a bigger file that carries exactly the same detail, which is worse
//! than leaving it alone, so the height is treated as a ceiling rather than a
//! target. The same rule applies to frame rate.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::errors::{AppError, Result};

pub mod encoders;
pub mod formats;
pub mod merge;
pub mod plan;
pub mod scan;

pub use encoders::{available_encoder, HardwareEncoder};
pub use formats::{PhotoFormat, VideoFormat};
pub use plan::{Aspect, Fit, Work};
pub use scan::{scan_paths, MediaItem, MediaKind};

/// Emitted for every state change of every file in a batch.
pub const JOB_EVENT: &str = "convert://job";
/// Emitted once when a batch finishes, cancelled or not.
pub const DONE_EVENT: &str = "convert://done";

/// How many files may convert at once by default.
///
/// Two, not "one per core": FFmpeg saturates several cores on its own, so a
/// higher number mostly means every file finishes later rather than the batch
/// finishing sooner. It is exposed in the UI for the machines where that trade
/// is worth making.
pub const DEFAULT_THREADS: usize = 2;

/// The ceiling on concurrent conversions, whatever the UI asks for.
pub const MAX_THREADS: usize = 16;

/// What the user chose in the settings bar.
#[derive(Debug, Clone, Deserialize)]
pub struct ConvertSettings {
    /// The container to write. Any source format converts to any of these -
    /// a `.ts` recording to `.mp4`, an `.avi` to `.webm`.
    #[serde(default)]
    pub video_format: VideoFormat,
    #[serde(default)]
    pub photo_format: PhotoFormat,
    /// Tallest side for video output. `None` keeps the source resolution.
    pub video_height: Option<u32>,
    /// Frames per second cap. `None` keeps the source rate.
    pub fps: Option<u32>,
    /// Tallest side for photo output. `None` keeps the source resolution.
    pub photo_height: Option<u32>,
    pub threads: usize,
    /// Use the platform's hardware encoder when one is available.
    pub gpu: bool,
    /// A target shape, from a platform preset. `None` keeps the source's.
    #[serde(default)]
    pub aspect: Option<Aspect>,
    /// Leave a file alone when it already matches the request instead of
    /// re-encoding it into something very slightly worse.
    #[serde(default = "yes")]
    pub skip_conforming: bool,
    /// Delete each source file once its conversion succeeds.
    pub delete_original: bool,
    /// Where converted files land. `None` keeps the default: a folder beside
    /// each source folder, which is what makes a mixed drop stay organised.
    /// A chosen folder collects everything in one place instead.
    pub output_dir: Option<String>,
}

fn yes() -> bool {
    true
}

/// One file's progress, as the table shows it.
#[derive(Debug, Clone, Serialize)]
pub struct JobUpdate {
    /// Matches `MediaItem::id`, so the row can be found without a path compare.
    pub id: String,
    /// "converting" | "done" | "skipped" | "failed" | "cancelled"
    pub status: String,
    /// "copy" when the streams were moved without re-encoding, "encode" when
    /// they were not. Shown in the row, because the difference is the whole
    /// reason a batch finishes in seconds rather than an hour.
    pub how: Option<String>,
    /// 0-100 while converting, for the row's bar. Absent for photos, which
    /// finish too quickly to be worth reporting.
    pub percent: Option<f64>,
    pub output_path: Option<String>,
    pub output_bytes: Option<u64>,
    pub error: Option<String>,
}

/// The summary emitted when the batch stops.
#[derive(Debug, Clone, Serialize)]
pub struct BatchDone {
    pub converted: usize,
    pub failed: usize,
    /// Files that were already exactly what was asked for.
    pub skipped: usize,
    pub cancelled: bool,
}

/// Shared cancel flag for the running batch.
///
/// One batch at a time, so a single flag is enough: starting a second batch
/// while one runs is refused rather than queued, because two batches writing
/// into the same output folders is not something a progress table can explain.
#[derive(Default)]
pub struct ConvertQueue {
    /// One entry per independent line of work, created on first use.
    lanes: std::sync::Mutex<std::collections::HashMap<String, Arc<Lane>>>,
}

/// A single line of work that can run, be cancelled, and refuse to be started
/// twice.
///
/// Lanes exist because the screens are separate: converting a folder of photos
/// while a folder of videos encodes is a perfectly ordinary thing to want, and
/// one global flag turned that into "a conversion is already running". They
/// stay separate rather than becoming one pool because each has its own
/// progress, its own cancel button, and its own idea of how many files to run
/// at once.
#[derive(Default)]
pub struct Lane {
    running: AtomicBool,
    cancelled: AtomicBool,
}

impl Lane {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Claim this lane. Fails if it is already busy.
    pub fn begin(&self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::ConvertBusy);
        }
        self.cancelled.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Release it. Always called, including on failure.
    pub fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl ConvertQueue {
    /// The lane by that name, created on first use.
    ///
    /// Names come from the caller - "video", "photo", "merge" - so a screen
    /// added later gets its own lane without touching this type.
    pub fn lane(&self, name: &str) -> Arc<Lane> {
        let mut lanes = self.lanes.lock().expect("convert lanes");
        Arc::clone(
            lanes
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(Lane::default())),
        )
    }
}

/// Convert every item, `settings.threads` at a time.
///
/// Returns when the whole batch is done. Progress arrives as events rather
/// than in the return value, so a table can update a row the moment its file
/// finishes instead of when the slowest one does.
pub async fn run_batch(
    app: AppHandle,
    lane: Arc<Lane>,
    items: Vec<MediaItem>,
    settings: ConvertSettings,
) -> Result<BatchDone> {
    lane.begin()?;

    let ffmpeg = crate::download::ytdlp::locate_ffmpeg().ok_or_else(|| {
        lane.finish();
        AppError::FfmpegMissing
    })?;

    // Hardware encoding is an H.264 offer: WebM and MP3 cannot use it at all,
    // so asking for it there would write a stream the container rejects.
    let encoder = if settings.gpu && settings.video_format.takes_h264() {
        encoders::available_encoder(&ffmpeg).await
    } else {
        HardwareEncoder::None
    };

    let threads = settings.threads.clamp(1, MAX_THREADS);
    let permits = Arc::new(tokio::sync::Semaphore::new(threads));
    let settings = Arc::new(settings);
    let ffmpeg = Arc::new(ffmpeg);

    let mut tasks = tokio::task::JoinSet::new();
    for item in items {
        let permits = Arc::clone(&permits);
        let lane = Arc::clone(&lane);
        let settings = Arc::clone(&settings);
        let ffmpeg = Arc::clone(&ffmpeg);
        let app = app.clone();

        tasks.spawn(async move {
            // Acquiring inside the task is what bounds concurrency; spawning
            // every job up front is fine because a waiting task costs nothing.
            let _permit = permits.acquire().await;
            if lane.is_cancelled() {
                emit(
                    &app,
                    JobUpdate {
                        id: item.id.clone(),
                        status: "cancelled".into(),
                        how: None,
                        percent: None,
                        output_path: None,
                        output_bytes: None,
                        error: None,
                    },
                );
                return Outcome::Cancelled;
            }

            // How much work this file needs is decided before a process
            // starts: most files need far less than a full re-encode, and
            // some need none at all.
            let work = plan::plan_work(
                &item,
                &plan::Request {
                    format: settings.video_format,
                    height_cap: settings.video_height,
                    fps_cap: settings.fps,
                    aspect: settings.aspect,
                    skip_conforming: settings.skip_conforming,
                },
            );

            let how = match work {
                Work::CopyFile => "clone",
                Work::Remux => "copy",
                Work::Encode => "encode",
            };
            emit(
                &app,
                JobUpdate {
                    id: item.id.clone(),
                    status: "converting".into(),
                    how: Some(how.into()),
                    percent: Some(0.0),
                    output_path: None,
                    output_bytes: None,
                    error: None,
                },
            );

            let outcome = if work == Work::CopyFile {
                // Already exactly what was asked for, so the bytes are the
                // answer: copied across rather than decoded and rebuilt, which
                // would cost minutes and hand back a slightly worse file.
                copy_across(&item, &settings)
            } else {
                convert_one(&app, &ffmpeg, &item, &settings, encoder, &lane, work).await
            };

            match outcome {
                Ok(output) => {
                    // Only after a successful write, and only if asked: losing
                    // the source to a conversion that failed is unrecoverable.
                    if settings.delete_original {
                        let _ = std::fs::remove_file(&item.path);
                    }
                    let bytes = std::fs::metadata(&output).map(|m| m.len()).ok();
                    emit(
                        &app,
                        JobUpdate {
                            id: item.id.clone(),
                            status: "done".into(),
                            how: Some(how.into()),
                            percent: Some(100.0),
                            output_path: Some(output.display().to_string()),
                            output_bytes: bytes,
                            error: None,
                        },
                    );
                    Outcome::Converted
                }
                Err(AppError::Cancelled) => {
                    emit(
                        &app,
                        JobUpdate {
                            id: item.id.clone(),
                            status: "cancelled".into(),
                            how: None,
                            percent: None,
                            output_path: None,
                            output_bytes: None,
                            error: None,
                        },
                    );
                    Outcome::Cancelled
                }
                Err(e) => {
                    emit(
                        &app,
                        JobUpdate {
                            id: item.id.clone(),
                            status: "failed".into(),
                            how: None,
                            percent: None,
                            output_path: None,
                            output_bytes: None,
                            error: Some(e.to_string()),
                        },
                    );
                    Outcome::Failed
                }
            }
        });
    }

    let mut converted = 0;
    let mut failed = 0;
    // Kept in the summary for the UI's sake; nothing is skipped outright any
    // more, so it stays zero.
    let skipped = 0;
    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(Outcome::Converted) => converted += 1,
            Ok(Outcome::Failed) => failed += 1,
            Ok(Outcome::Cancelled) => {}
            // A panicking task must not take the batch with it.
            Err(_) => failed += 1,
        }
    }

    let done = BatchDone {
        converted,
        failed,
        skipped,
        cancelled: lane.is_cancelled(),
    };
    lane.finish();
    let _ = app.emit(DONE_EVENT, done.clone());
    Ok(done)
}

enum Outcome {
    Converted,
    Failed,
    Cancelled,
}

fn emit(app: &AppHandle, update: JobUpdate) {
    let _ = app.emit(JOB_EVENT, update);
}

/// Copy a file that already matches the request into the output folder.
///
/// The alternative was to skip it, which is what this used to do - and it left
/// the chosen folder missing the very files that needed no work, with
/// "Already OK" as the only clue. An output folder is a deliverable: everything
/// asked for belongs in it.
fn copy_across(item: &MediaItem, settings: &ConvertSettings) -> Result<PathBuf> {
    let source = PathBuf::from(&item.path);
    let out_dir = match settings.output_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => output_dir_for(&source),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| {
        AppError::DownloadPath(format!("could not create {}: {e}", out_dir.display()))
    })?;

    let out = unique_path(&out_dir.join(output_name(
        &source,
        item.kind,
        settings.video_format,
        settings.photo_format,
    )));

    // Guards the one case where copying would destroy the file: a source that
    // already sits at the destination path.
    if out == source {
        return Ok(source);
    }

    std::fs::copy(&source, &out)
        .map_err(|e| AppError::ConvertFailed(format!("could not copy: {e}")))?;
    Ok(out)
}

/// Convert one file and return where it landed.
async fn convert_one(
    app: &AppHandle,
    ffmpeg: &Path,
    item: &MediaItem,
    settings: &ConvertSettings,
    encoder: HardwareEncoder,
    lane: &Lane,
    // Decided by the caller, before any process starts: a copy, or an encode.
    work: Work,
) -> Result<PathBuf> {
    let source = PathBuf::from(&item.path);
    let out_dir = match settings.output_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => output_dir_for(&source),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| {
        AppError::DownloadPath(format!("could not create {}: {e}", out_dir.display()))
    })?;

    let out = unique_path(&out_dir.join(output_name(
        &source,
        item.kind,
        settings.video_format,
        settings.photo_format,
    )));

    let mut cmd = crate::process::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    cmd.arg("-i").arg(&source);

    match item.kind {
        MediaKind::Video if work == Work::Remux => {
            // The streams already suit the target container, so this is a
            // copy: no decode, no quality change, and an hour of video in
            // about a second.
            cmd.args(["-c", "copy"]);
            if settings.video_format.is_audio_only() {
                cmd.arg("-vn");
            }
            for arg in settings.video_format.container_args() {
                cmd.arg(arg);
            }
            cmd.args(["-progress", "pipe:1", "-nostats"]);
        }
        MediaKind::Video => {
            let format = settings.video_format;
            if !format.is_audio_only() {
                // An aspect preset composites, so it owns the filter chain; a
                // plain height cap is a single scale.
                match settings.aspect {
                    Some(aspect) => {
                        let canvas =
                            plan::canvas_for(&aspect, item.height, settings.video_height);
                        match plan::aspect_filter(&aspect, canvas) {
                            Some(filter) => {
                                cmd.arg("-vf").arg(filter);
                            }
                            // A blurred backdrop composites the input with
                            // itself, which only filter_complex can express.
                            None => {
                                cmd.arg("-filter_complex").arg(plan::blur_backdrop(canvas));
                                cmd.args(["-map", "[v]"]);
                                // `?` so a clip with no sound is not an error.
                                cmd.args(["-map", "0:a?"]);
                            }
                        }
                    }
                    None => {
                        if let Some(filter) = plan::scale_filter(settings.video_height) {
                            cmd.arg("-vf").arg(filter);
                        }
                    }
                }
                if let Some(fps) = target_fps(settings.fps, item.fps) {
                    cmd.arg("-fps_mode").arg("cfr");
                    cmd.arg("-r").arg(fps.to_string());
                }
                // Hardware where it is both available and valid for this
                // container; the container's own software encoder otherwise.
                let video_args = match encoder {
                    HardwareEncoder::None => format.software_video_args(),
                    hw => hw.video_args(),
                };
                for arg in video_args {
                    cmd.arg(arg);
                }
            }
            for arg in format.audio_args() {
                cmd.arg(arg);
            }
            for arg in format.container_args() {
                cmd.arg(arg);
            }
            // Machine-readable progress on stdout, so the row can show a bar.
            cmd.args(["-progress", "pipe:1", "-nostats"]);
        }
        MediaKind::Photo => {
            if let Some(filter) = plan::scale_filter(settings.photo_height) {
                cmd.arg("-vf").arg(filter);
            }
            for arg in settings.photo_format.quality_args() {
                cmd.arg(arg);
            }
        }
    }

    cmd.arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|_| AppError::FfmpegMissing)?;

    // Stream the progress lines for videos; photos finish before a bar would
    // paint, so their stdout is simply drained.
    if let (Some(stdout), Some(duration)) = (child.stdout.take(), item.duration_seconds) {
        let app = app.clone();
        let id = item.id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // `out_time_us=12340000`, emitted several times a second.
                let Some(us) = line.strip_prefix("out_time_us=") else {
                    continue;
                };
                let Ok(us) = us.trim().parse::<f64>() else {
                    continue;
                };
                if duration <= 0.0 {
                    continue;
                }
                let percent = ((us / 1_000_000.0) / duration * 100.0).clamp(0.0, 99.0);
                emit(
                    &app,
                    JobUpdate {
                        id: id.clone(),
                        status: "converting".into(),
                        how: None,
                        percent: Some(percent),
                        output_path: None,
                        output_bytes: None,
                        error: None,
                    },
                );
            }
        });
    }

    // Poll rather than await the child directly, so a cancel takes effect
    // during a long encode instead of after it.
    loop {
        if lane.is_cancelled() {
            let _ = child.kill().await;
            // A half-written file is not a conversion; leaving it behind would
            // look like a successful one to anyone browsing the folder.
            let _ = std::fs::remove_file(&out);
            return Err(AppError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(out);
                }
                let mut stderr = String::new();
                if let Some(err) = child.stderr.take() {
                    let mut lines = BufReader::new(err).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if stderr.len() < 500 {
                            stderr.push_str(line.trim());
                            stderr.push(' ');
                        }
                    }
                }
                let _ = std::fs::remove_file(&out);
                let detail = stderr.trim();
                return Err(AppError::ConvertFailed(if detail.is_empty() {
                    "FFmpeg refused the file".to_string()
                } else {
                    detail.chars().take(200).collect()
                }));
            }
            Ok(None) => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(e) => return Err(AppError::ConvertFailed(e.to_string())),
        }
    }
}

/// The frame rate to ask for, or `None` to leave the source alone.
///
/// A cap, exactly like the height: asking 60 fps of a 30 fps source makes
/// FFmpeg duplicate every frame, which doubles the file size and adds no
/// smoothness at all, because the motion that would fill those frames was
/// never filmed. A source whose rate is unknown is left untouched for the same
/// reason - guessing could only make it worse.
///
/// The half-frame slack matters: a 29.97 fps source asked for 30 is already
/// there, and re-timing it to exactly 30 would drift the audio sync.
fn target_fps(requested: Option<u32>, source: Option<f64>) -> Option<u32> {
    let requested = requested?;
    match source {
        Some(actual) if actual <= requested as f64 + 0.5 => None,
        Some(_) => Some(requested),
        None => None,
    }
}

/// Where a converted file goes: a folder beside the one it came from.
///
/// Beside rather than inside, so a second run does not pick up the output of
/// the first and convert it again.
pub fn output_dir_for(source: &Path) -> PathBuf {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".to_string());
    parent.with_file_name(format!("{name} (converted)"))
}

/// The output filename: the source's own name, with the chosen format's
/// extension. Keeping the stem is what lets a converted folder still be read
/// alongside the original one.
fn output_name(
    source: &Path,
    kind: MediaKind,
    video: VideoFormat,
    photo: PhotoFormat,
) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    match kind {
        MediaKind::Video => format!("{stem}.{}", video.extension()),
        MediaKind::Photo => format!("{stem}.{}", photo.extension()),
    }
}

/// Never overwrite: two source folders can hold the same filename, and the
/// second one silently replacing the first is data loss with no error.
fn unique_path(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let stem = desired
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let ext = desired
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".into());
    let dir = desired.parent().unwrap_or_else(|| Path::new("."));
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    desired.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_lands_beside_the_source_folder_not_inside_it() {
        // Inside would mean a second run converts the first run's output.
        let dir = output_dir_for(Path::new("/media/clips/one.mp4"));
        assert_eq!(dir, PathBuf::from("/media/clips (converted)"));
    }

    #[test]
    fn a_height_is_a_ceiling_and_never_an_upscale() {
        let f = plan::scale_filter(Some(1080)).unwrap();
        assert!(f.contains("min(1080,ih)"), "{f}");
        // `-2` keeps the width even, which H.264 requires.
        assert!(f.starts_with("scale=-2:"), "{f}");
        assert!(
            plan::scale_filter(None).is_none(),
            "no cap means no filter at all"
        );
    }

    #[test]
    fn a_frame_rate_is_a_cap_and_never_invents_frames() {
        // 60 fps down to 30: a real change.
        assert_eq!(target_fps(Some(30), Some(60.0)), Some(30));
        // 24 fps asked for 30 - already below, so leave it alone rather than
        // duplicating frames into a bigger file that looks identical.
        assert_eq!(target_fps(Some(30), Some(24.0)), None);
        // 29.97 asked for 30 is already there; re-timing would drift audio.
        assert_eq!(target_fps(Some(30), Some(29.97)), None);
        // Unknown source rate: change nothing.
        assert_eq!(target_fps(Some(30), None), None);
        // "Keep original" never sets a rate at all.
        assert_eq!(target_fps(None, Some(60.0)), None);
    }

    #[test]
    fn the_output_keeps_its_name_and_takes_the_chosen_extension() {
        // Any source container to any target one: this is what makes a `.ts`
        // recording come out as `.mp4`.
        let name = |src, kind, v, p| output_name(Path::new(src), kind, v, p);
        assert_eq!(
            name("/a/clip.ts", MediaKind::Video, VideoFormat::Mp4, PhotoFormat::Jpg),
            "clip.mp4"
        );
        assert_eq!(
            name("/a/clip.mp4", MediaKind::Video, VideoFormat::Webm, PhotoFormat::Jpg),
            "clip.webm"
        );
        assert_eq!(
            name("/a/clip.avi", MediaKind::Video, VideoFormat::Mp3, PhotoFormat::Jpg),
            "clip.mp3"
        );
        assert_eq!(
            name("/a/shot.png", MediaKind::Photo, VideoFormat::Mp4, PhotoFormat::Webp),
            "shot.webp"
        );
    }

    #[test]
    fn threads_are_clamped_to_something_a_machine_can_run() {
        assert_eq!(DEFAULT_THREADS.clamp(1, MAX_THREADS), DEFAULT_THREADS);
        assert_eq!(0usize.clamp(1, MAX_THREADS), 1);
        assert_eq!(999usize.clamp(1, MAX_THREADS), MAX_THREADS);
    }

    #[test]
    fn a_second_batch_is_refused_while_that_lane_is_running() {
        let q = ConvertQueue::default();
        let video = q.lane("video");
        video.begin().unwrap();
        assert!(video.is_running());
        assert!(matches!(video.begin(), Err(AppError::ConvertBusy)));
        video.finish();
        // And starting again afterwards clears the previous cancel.
        video.cancel();
        video.begin().unwrap();
        assert!(!video.is_cancelled());
    }

    #[test]
    fn photos_and_videos_convert_at_the_same_time() {
        // The whole point of lanes: a busy video batch must not refuse a photo
        // one, and cancelling either must leave the other running.
        let q = ConvertQueue::default();
        let video = q.lane("video");
        let photo = q.lane("photo");
        video.begin().unwrap();
        photo.begin().unwrap();
        assert!(video.is_running() && photo.is_running());

        video.cancel();
        assert!(video.is_cancelled());
        assert!(!photo.is_cancelled(), "cancelling one lane stopped the other");
    }

    #[test]
    fn the_same_name_always_means_the_same_lane() {
        // Otherwise "cancel" would create a fresh lane and cancel nothing.
        let q = ConvertQueue::default();
        q.lane("video").begin().unwrap();
        assert!(matches!(q.lane("video").begin(), Err(AppError::ConvertBusy)));
    }
}
