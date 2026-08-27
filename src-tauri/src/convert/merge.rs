//! Joining several clips into one file.
//!
//! TWO WAYS TO DO IT, and the difference is large. When every input already
//! shares a codec, size and frame rate - the usual case for clips downloaded
//! from one place, or produced by the Split tab - FFmpeg's concat *demuxer*
//! stitches them by copying streams: no decode, no quality loss, seconds for
//! an hour of footage.
//!
//! When they differ, the streams cannot simply be appended: a player reaching
//! the join would meet a new resolution mid-file. Those are re-encoded onto one
//! canvas through the concat *filter* instead, which costs real time and is why
//! the tab says which path it is taking before it starts.
//!
//! ORDER IS THE FEATURE. The caller passes items in the order they should play,
//! and that order is preserved exactly - no sorting, no dedupe. Two copies of
//! the same clip back to back is a legitimate thing to ask for.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::formats::VideoFormat;
use super::plan::Fit;
use super::scan::MediaItem;
use super::{HardwareEncoder, Lane};
use crate::errors::{AppError, Result};

/// Emitted as a merge runs.
pub const MERGE_EVENT: &str = "convert://merge";

/// Where a merge has got to.
#[derive(Debug, Clone, Serialize)]
pub struct MergeProgress {
    /// 0-100 across the whole output, not per input.
    pub percent: f64,
    /// "copy" when the streams are being appended untouched, "encode" when the
    /// inputs had to be normalised onto one canvas first.
    pub how: String,
}

/// What a merge produced.
#[derive(Debug, Clone, Serialize)]
pub struct MergeResult {
    pub path: String,
    pub size_bytes: u64,
    pub duration_seconds: f64,
    pub how: String,
}

/// Whether these inputs can be appended without re-encoding.
///
/// Every property that would change mid-file has to match: a player cannot
/// switch resolution, codec or frame rate at a join. Missing information counts
/// as a mismatch - a file FFmpeg could not measure is not one to gamble on.
pub fn can_copy(items: &[MediaItem], format: VideoFormat, canvas: (u32, u32)) -> bool {
    let Some(first) = items.first() else {
        return false;
    };
    let Some(vcodec) = first.vcodec.as_deref() else {
        return false;
    };
    if !format.accepts_video(vcodec) {
        return false;
    }
    if let Some(acodec) = first.acodec.as_deref() {
        if !format.accepts_audio(acodec) {
            return false;
        }
    }

    // A shape the clips do not already have is a re-encode however well they
    // match each other.
    if first.width != Some(canvas.0) || first.height != Some(canvas.1) {
        return false;
    }

    items.iter().all(|i| {
        i.vcodec.as_deref() == first.vcodec.as_deref()
            && i.acodec.as_deref() == first.acodec.as_deref()
            && i.width.is_some()
            && i.width == first.width
            && i.height == first.height
            && match (i.fps, first.fps) {
                // Frame rates are floats from a fraction; a hundredth apart is
                // the same rate reported differently.
                (Some(a), Some(b)) => (a - b).abs() < 0.01,
                _ => false,
            }
    })
}

/// The shape the merged file should have.
///
/// This exists because mixing orientations has no right answer the code can
/// pick for you: a portrait clip and a landscape clip cannot both fill the same
/// frame. Deriving a canvas from the inputs produced a square whenever they
/// disagreed - bars on all four sides across the whole video, which is nobody's
/// intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Take the first clip's shape. The order already says which clip leads,
    /// so this is the least surprising default.
    #[default]
    First,
    Landscape,
    Portrait,
    Square,
}

impl Shape {
    fn ratio(self, items: &[MediaItem]) -> f64 {
        match self {
            Self::Landscape => 16.0 / 9.0,
            Self::Portrait => 9.0 / 16.0,
            Self::Square => 1.0,
            Self::First => items
                .first()
                .and_then(|i| match (i.width, i.height) {
                    (Some(w), Some(h)) if h > 0 => Some(w as f64 / h as f64),
                    _ => None,
                })
                .unwrap_or(16.0 / 9.0),
        }
    }
}

/// The canvas every clip is fitted onto.
///
/// Sized from the longest side any input actually has, so a merge never
/// enlarges footage past its own resolution, then capped by the caller's
/// ceiling when one is set.
pub fn common_canvas(items: &[MediaItem], shape: Shape, cap: Option<u32>) -> (u32, u32) {
    let longest = items
        .iter()
        .filter_map(|i| match (i.width, i.height) {
            (Some(w), Some(h)) => Some(w.max(h)),
            _ => None,
        })
        .max()
        .unwrap_or(1920);

    let ratio = shape.ratio(items);
    let (mut w, mut h) = if ratio >= 1.0 {
        (longest, (longest as f64 / ratio).round() as u32)
    } else {
        ((longest as f64 * ratio).round() as u32, longest)
    };

    if let Some(cap) = cap {
        if h > cap {
            w = ((cap as f64) * ratio).round() as u32;
            h = cap;
        }
    }
    (even(w.max(2)), even(h.max(2)))
}

fn even(n: u32) -> u32 {
    if n % 2 == 0 {
        n
    } else {
        n - 1
    }
}

/// Join `items`, in the order given, into `output`.
pub async fn merge(
    app: &AppHandle,
    lane: &Lane,
    items: &[MediaItem],
    output: &Path,
    format: VideoFormat,
    height_cap: Option<u32>,
    shape: Shape,
    fit: Fit,
    encoder: HardwareEncoder,
) -> Result<MergeResult> {
    if items.len() < 2 {
        return Err(AppError::MergeInput(
            "Add at least two clips — merging one is the file you already have.".into(),
        ));
    }

    let ffmpeg = crate::download::ytdlp::locate_ffmpeg().ok_or(AppError::FfmpegMissing)?;
    let total: f64 = items.iter().filter_map(|i| i.duration_seconds).sum();
    let canvas = common_canvas(items, shape, height_cap);
    let copying = can_copy(items, format, canvas);
    let how = if copying { "copy" } else { "encode" };

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::DownloadPath(format!("could not create {}: {e}", parent.display()))
        })?;
    }

    let mut cmd = crate::process::command(&ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);

    // Held until the process exits: the demuxer reads this file as it goes.
    let list_file = if copying {
        let list = concat_list(items);
        let path = std::env::temp_dir().join(format!("socialsync-concat-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, list)
            .map_err(|e| AppError::MergeInput(format!("could not stage the clip list: {e}")))?;
        cmd.args(["-f", "concat", "-safe", "0"]).arg("-i").arg(&path);
        cmd.args(["-c", "copy"]);
        Some(path)
    } else {
        for item in items {
            cmd.arg("-i").arg(&item.path);
        }
        // Audio only survives if every input has some: concat's `a=1` requires
        // one audio stream per segment, and a silent gap would need a
        // generated track the user never asked for.
        let with_audio = items.iter().all(|i| i.acodec.is_some());
        cmd.arg("-filter_complex")
            .arg(concat_filter(items.len(), canvas, with_audio, fit));
        cmd.args(["-map", "[v]"]);
        if with_audio {
            cmd.args(["-map", "[a]"]);
        }
        for arg in match encoder {
            HardwareEncoder::None => format.software_video_args(),
            hw => hw.video_args(),
        } {
            cmd.arg(arg);
        }
        if with_audio {
            for arg in format.audio_args() {
                cmd.arg(arg);
            }
        }
        None
    };

    for arg in format.container_args() {
        cmd.arg(arg);
    }
    cmd.args(["-progress", "pipe:1", "-nostats"]);
    cmd.arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|_| AppError::FfmpegMissing)?;

    if let Some(stdout) = child.stdout.take() {
        let app = app.clone();
        let how = how.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(us) = line.strip_prefix("out_time_us=") else {
                    continue;
                };
                let Ok(us) = us.trim().parse::<f64>() else {
                    continue;
                };
                if total <= 0.0 {
                    continue;
                }
                let _ = app.emit(
                    MERGE_EVENT,
                    MergeProgress {
                        percent: ((us / 1_000_000.0) / total * 100.0).clamp(0.0, 99.0),
                        how: how.clone(),
                    },
                );
            }
        });
    }

    let outcome = loop {
        if lane.is_cancelled() {
            let _ = child.kill().await;
            let _ = std::fs::remove_file(output);
            break Err(AppError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => {
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
                let _ = std::fs::remove_file(output);
                let detail = stderr.trim();
                break Err(AppError::ConvertFailed(if detail.is_empty() {
                    "FFmpeg refused the clips".to_string()
                } else {
                    detail.chars().take(200).collect()
                }));
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            Err(e) => break Err(AppError::ConvertFailed(e.to_string())),
        }
    };

    if let Some(list) = list_file {
        let _ = std::fs::remove_file(list);
    }
    outcome?;

    Ok(MergeResult {
        path: output.display().to_string(),
        size_bytes: std::fs::metadata(output).map(|m| m.len()).unwrap_or(0),
        duration_seconds: total,
        how: how.to_string(),
    })
}

/// The concat demuxer's input file.
///
/// Single quotes are the demuxer's own escape, and a path containing one would
/// otherwise end the filename early - which is a parse error at best and the
/// wrong file at worst.
pub fn concat_list(items: &[MediaItem]) -> String {
    items
        .iter()
        .map(|i| format!("file '{}'\n", i.path.replace('\'', "'\\''")))
        .collect()
}

/// The filter graph that normalises differing inputs and joins them.
///
/// `setsar=1` is not optional: inputs with different pixel aspect ratios
/// concatenate into a file that plays at the wrong width, and the cause is
/// invisible in every property a table would show.
pub fn concat_filter(count: usize, canvas: (u32, u32), with_audio: bool, fit: Fit) -> String {
    let (w, h) = canvas;
    let (bw, bh) = (even((w / 8).max(16)), even((h / 8).max(16)));
    let mut graph = String::new();
    for i in 0..count {
        match fit {
            // Fill the frame; whatever does not fit is cropped away.
            Fit::Crop => graph.push_str(&format!(
                "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=increase,\
                 crop={w}:{h},setsar=1[v{i}];"
            )),
            // The whole picture on black.
            Fit::Pad => graph.push_str(&format!(
                "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:-1:-1:color=black,setsar=1[v{i}];"
            )),
            // The whole picture over an enlarged, blurred copy of itself. The
            // backdrop is blurred at an eighth scale - identical once enlarged,
            // and a fraction of the cost of a full-frame gaussian.
            Fit::Blur => graph.push_str(&format!(
                "[{i}:v]split=2[bg{i}][fg{i}];\
                 [bg{i}]scale={bw}:{bh}:force_original_aspect_ratio=increase,\
                 crop={bw}:{bh},gblur=sigma=8,scale={w}:{h}[bb{i}];\
                 [fg{i}]scale={w}:{h}:force_original_aspect_ratio=decrease[ff{i}];\
                 [bb{i}][ff{i}]overlay=(W-w)/2:(H-h)/2,setsar=1[v{i}];"
            )),
        }
        if with_audio {
            // One sample rate and channel layout, or the join clicks.
            graph.push_str(&format!("[{i}:a]aformat=sample_rates=48000:channel_layouts=stereo[a{i}];"));
        }
    }
    for i in 0..count {
        graph.push_str(&format!("[v{i}]"));
        if with_audio {
            graph.push_str(&format!("[a{i}]"));
        }
    }
    graph.push_str(&format!(
        "concat=n={count}:v=1:a={}[v]{}",
        if with_audio { 1 } else { 0 },
        if with_audio { "[a]" } else { "" }
    ));
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::scan::MediaKind;

    fn item(path: &str, w: u32, h: u32, fps: f64, v: &str, a: Option<&str>) -> MediaItem {
        MediaItem {
            id: path.into(),
            path: path.into(),
            file_name: path.into(),
            directory: "/tmp".into(),
            kind: MediaKind::Video,
            size_bytes: 1,
            duration_seconds: Some(10.0),
            width: Some(w),
            height: Some(h),
            fps: Some(fps),
            vcodec: Some(v.into()),
            acodec: a.map(str::to_string),
            supported: true,
        }
    }

    #[test]
    fn matching_clips_are_appended_without_re_encoding() {
        let items = [
            item("/a/1.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
            item("/a/2.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
        ];
        assert!(can_copy(&items, VideoFormat::Mp4, (1920, 1080)));
    }

    #[test]
    fn any_difference_that_would_break_playback_forces_an_encode() {
        let base = item("/a/1.mp4", 1920, 1080, 30.0, "h264", Some("aac"));
        for other in [
            // A resolution change mid-file.
            item("/a/2.mp4", 1280, 720, 30.0, "h264", Some("aac")),
            // A frame rate change.
            item("/a/2.mp4", 1920, 1080, 60.0, "h264", Some("aac")),
            // A codec change.
            item("/a/2.mp4", 1920, 1080, 30.0, "hevc", Some("aac")),
            // One clip silent, one not.
            item("/a/2.mp4", 1920, 1080, 30.0, "h264", None),
        ] {
            assert!(
                !can_copy(&[base.clone(), other.clone()], VideoFormat::Mp4, (1920, 1080)),
                "{other:?}"
            );
        }
    }

    #[test]
    fn a_rate_reported_as_2997_matches_one_reported_as_2997() {
        let a = item("/a/1.mp4", 1920, 1080, 29.97, "h264", Some("aac"));
        let mut b = item("/a/2.mp4", 1920, 1080, 29.97, "h264", Some("aac"));
        b.fps = Some(29.971);
        assert!(can_copy(&[a, b], VideoFormat::Mp4, (1920, 1080)));
    }

    #[test]
    fn streams_the_target_container_rejects_are_never_copied() {
        // Two identical H.264 clips still cannot be copied into WebM.
        let items = [
            item("/a/1.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
            item("/a/2.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
        ];
        assert!(!can_copy(&items, VideoFormat::Webm, (1920, 1080)));
    }

    #[test]
    fn a_single_clip_is_not_a_merge() {
        let items = [item("/a/1.mp4", 1920, 1080, 30.0, "h264", Some("aac"))];
        assert!(!can_copy(&items[..0], VideoFormat::Mp4, (1920, 1080)), "empty");
        // One clip can technically be copied, but `merge` refuses it earlier
        // with a message rather than writing a duplicate of the input.
        assert!(can_copy(&items, VideoFormat::Mp4, (1920, 1080)));
    }

    #[test]
    fn a_quote_in_a_path_cannot_end_the_filename_early() {
        let items = [item("/a/it's here.mp4", 1920, 1080, 30.0, "h264", Some("aac"))];
        let list = concat_list(&items);
        assert_eq!(list, "file '/a/it'\\''s here.mp4'\n");
    }

    #[test]
    fn the_canvas_is_the_largest_input_never_larger() {
        let items = [
            item("/a/1.mp4", 1280, 720, 30.0, "h264", Some("aac")),
            item("/a/2.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
        ];
        assert_eq!(common_canvas(&items, Shape::First, None), (1920, 1080));
        // A cap shrinks it, keeping the shape.
        assert_eq!(common_canvas(&items, Shape::First, Some(720)), (1280, 720));
    }

    #[test]
    fn mixing_orientations_follows_the_shape_that_was_asked_for() {
        // The case with no automatic right answer: one portrait, one landscape.
        let mixed = [
            item("/a/wide.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
            item("/a/tall.mp4", 1080, 1920, 30.0, "h264", Some("aac")),
        ];
        // First clip leads, so this is a landscape video.
        assert_eq!(common_canvas(&mixed, Shape::First, None), (1920, 1080));
        // Or say so explicitly, either way.
        assert_eq!(common_canvas(&mixed, Shape::Portrait, None), (1080, 1920));
        assert_eq!(common_canvas(&mixed, Shape::Landscape, None), (1920, 1080));
        assert_eq!(common_canvas(&mixed, Shape::Square, None), (1920, 1920));

        // Reverse the order and "first" follows it.
        let reversed = [mixed[1].clone(), mixed[0].clone()];
        assert_eq!(common_canvas(&reversed, Shape::First, None), (1080, 1920));
    }

    #[test]
    fn a_shape_the_clips_do_not_have_is_never_a_stream_copy() {
        // Two identical landscape clips: copyable as landscape...
        let items = [
            item("/a/1.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
            item("/a/2.mp4", 1920, 1080, 30.0, "h264", Some("aac")),
        ];
        assert!(can_copy(&items, VideoFormat::Mp4, (1920, 1080)));
        // ...but asking for portrait means every frame must be redrawn.
        let portrait = common_canvas(&items, Shape::Portrait, None);
        assert!(!can_copy(&items, VideoFormat::Mp4, portrait));
    }

    #[test]
    fn the_graph_normalises_every_input_before_joining_them() {
        let g = concat_filter(3, (1920, 1080), true, Fit::Pad);
        // Without setsar a mixed-PAR join plays at the wrong width.
        assert_eq!(g.matches("setsar=1").count(), 3);
        assert!(g.contains("concat=n=3:v=1:a=1[v][a]"), "{g}");

        // Silent output when not every clip has sound.
        let silent = concat_filter(2, (1280, 720), false, Fit::Pad);
        assert!(silent.contains("concat=n=2:v=1:a=0[v]"), "{silent}");
        assert!(!silent.contains("[a]"), "{silent}");
    }

    #[test]
    fn each_fit_shapes_every_clip_the_way_it_promises() {
        let pad = concat_filter(2, (1080, 1920), false, Fit::Pad);
        assert_eq!(pad.matches("force_original_aspect_ratio=decrease").count(), 2);
        assert_eq!(pad.matches("pad=1080:1920").count(), 2);

        let crop = concat_filter(2, (1080, 1920), false, Fit::Crop);
        assert_eq!(crop.matches("force_original_aspect_ratio=increase").count(), 2);
        assert_eq!(crop.matches("crop=1080:1920").count(), 2);

        // The backdrop is its own copy of each input, blurred small and
        // enlarged, with the real picture composited on top.
        let blur = concat_filter(2, (1080, 1920), false, Fit::Blur);
        assert_eq!(blur.matches("split=2").count(), 2);
        assert_eq!(blur.matches("gblur").count(), 2);
        assert_eq!(blur.matches("overlay=").count(), 2);
        // Every branch still ends in the label concat consumes.
        assert!(blur.contains("[v0]") && blur.contains("[v1]"), "{blur}");
    }
}
