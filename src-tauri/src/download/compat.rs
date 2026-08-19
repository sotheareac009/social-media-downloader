//! Making a finished download actually playable.
//!
//! Preferring H.264 in the format selector is the cheap fix, but it only works
//! when the platform offers H.264 at all. Instagram serves reels as VP9, and a
//! VP9-in-MP4 file is a *correct, complete* download that QuickTime refuses to
//! open - which reads to a user as a broken app rather than a codec they have
//! never heard of.
//!
//! So when selection cannot deliver a playable codec, this re-encodes. That is
//! a real cost - it spends CPU and loses a little quality - and it is done only
//! when the user has asked for Apple-compatible output.
//!
//! Measured on a real 15-second Instagram reel: 2.5 seconds, 1080x1920
//! preserved, 2.9 MB in and 5.0 MB out. VP9 is the more efficient codec, so
//! compatible files are larger.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::errors::{AppError, Result};

/// Codecs macOS plays without complaint. Everything else is re-encoded when
/// compatibility is requested.
const APPLE_FRIENDLY: &[&str] = &["h264", "hevc", "prores"];

/// Above this, re-encoding stops being a courtesy and becomes a surprise.
/// A long 4K video would burn minutes of CPU without warning, so it is left
/// as downloaded and the caller reports why.
const MAX_TRANSCODE_SECONDS: f64 = 20.0 * 60.0;

/// ffprobe ships alongside ffmpeg, so derive one from the other rather than
/// searching PATH again and risking a mismatched pair.
pub fn ffprobe_beside(ffmpeg: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let candidate = ffmpeg.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// The first video stream's codec name, or `None` when there is no video.
pub async fn video_codec(ffmpeg: &Path, file: &Path) -> Option<String> {
    let ffprobe = ffprobe_beside(ffmpeg)?;
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;

    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

async fn duration_seconds(ffmpeg: &Path, file: &Path) -> Option<f64> {
    let ffprobe = ffprobe_beside(ffmpeg)?;
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-show_entries", "format=duration"])
        .args(["-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

pub fn is_apple_friendly(codec: &str) -> bool {
    APPLE_FRIENDLY.contains(&codec.to_ascii_lowercase().as_str())
}

/// Outcome of a compatibility pass.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Already playable, or not a video - nothing was done.
    Untouched,
    /// Re-encoded in place; the path is unchanged.
    Converted { from: String },
    /// Would have needed re-encoding but was left alone, with the reason.
    Skipped(String),
}

/// Re-encode `file` to H.264 in place, if it needs it.
///
/// The original is replaced only after the new file is written, so a failure
/// mid-encode leaves the download intact.
pub async fn ensure_playable(ffmpeg: &Path, file: &Path) -> Result<Outcome> {
    let Some(codec) = video_codec(ffmpeg, file).await else {
        return Ok(Outcome::Untouched); // audio-only, or unprobeable
    };
    if is_apple_friendly(&codec) {
        return Ok(Outcome::Untouched);
    }

    if let Some(seconds) = duration_seconds(ffmpeg, file).await {
        if seconds > MAX_TRANSCODE_SECONDS {
            return Ok(Outcome::Skipped(format!(
                "{codec} file is {:.0} minutes long; left as downloaded rather than spending several minutes re-encoding",
                seconds / 60.0
            )));
        }
    }

    let temp = file.with_extension("compat.mp4");
    let status = Command::new(ffmpeg)
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(file)
        // veryfast keeps this in seconds for a short reel; crf 20 is visually
        // close to source without doubling the file again.
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k"])
        // Lets a player start before the whole file is read.
        .args(["-movflags", "+faststart"])
        .arg(&temp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("could not run ffmpeg: {e}")))?;

    if !status.status.success() {
        let _ = std::fs::remove_file(&temp);
        let detail = String::from_utf8_lossy(&status.stderr);
        let last = detail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(AppError::EngineFailed(format!(
            "could not convert {codec} for playback: {last}"
        )));
    }

    // Replace only now that the replacement exists.
    std::fs::rename(&temp, file).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        AppError::DownloadPath(format!("could not replace the original: {e}"))
    })?;

    Ok(Outcome::Converted { from: codec })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_codecs_are_left_alone() {
        for c in ["h264", "H264", "hevc", "prores"] {
            assert!(is_apple_friendly(c), "{c}");
        }
    }

    #[test]
    fn the_codecs_quicktime_refuses_are_converted() {
        // These are the ones that produced "isn't compatible with QuickTime
        // Player" on real downloads: VP9 from Instagram, AV1 from YouTube.
        for c in ["vp9", "av1", "vp8", "theora"] {
            assert!(!is_apple_friendly(c), "{c}");
        }
    }

    #[test]
    fn ffprobe_is_derived_from_the_ffmpeg_path() {
        let expected = PathBuf::from("/opt/homebrew/bin").join(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        if expected.is_file() {
            assert_eq!(ffprobe_beside(Path::new("/opt/homebrew/bin/ffmpeg")), Some(expected));
        }
    }
}
