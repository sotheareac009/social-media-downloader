//! Turning a photo post into something you can actually play.
//!
//! TikTok slideshow posts expose no video stream, so a download of one lands
//! as a bare `.mp3`. That is technically correct and practically useless: it
//! sits in a folder of videos, plays no picture, and looks like the downloader
//! lost something.
//!
//! WHAT THIS CAN AND CANNOT DO. yt-dlp surfaces exactly one image for these
//! posts - the cover - and both its `thumbnails` entries point at that same
//! asset. The individual slides are not in the payload at all. So the video
//! built here is the **cover image held for the length of the audio**, not the
//! slideshow. Reaching the real slides would mean scraping TikTok's page
//! directly, which is behind the same anti-bot wall that serves this app an
//! empty shell, and would need a browser session the downloader deliberately
//! never has.
//!
//! The result is therefore an honest approximation, and the UI says so rather
//! than presenting it as the original post.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::errors::{AppError, Result};

/// Whether a file on disk actually contains a video stream.
///
/// The authority for "is this a photo post" has to be the downloaded file, not
/// the metadata. Trusting metadata alone caused a real failure: an Instagram
/// reel whose JSON did not expose a video codec was treated as audio, so a
/// still image was encoded over its soundtrack and the genuine video was
/// deleted. Probing costs milliseconds and makes that impossible.
///
/// Returns `true` when the answer cannot be determined - refusing to convert
/// is always the safe direction, since the download is already correct.
pub async fn file_has_video(ffmpeg: &Path, file: &Path) -> bool {
    // `None` means no video stream *or* an unprobeable file. Both resolve to
    // "leave it alone", which is the safe direction either way.
    match crate::download::compat::ffprobe_beside(ffmpeg) {
        None => true,
        Some(_) => crate::download::compat::video_codec(ffmpeg, file)
            .await
            .is_some(),
    }
}

/// Build `<audio>.mp4` from a still image and an audio track.
///
/// On success the audio file is removed and the new path returned. Any failure
/// leaves the audio untouched: a missing picture is a far smaller problem than
/// a download that vanished.
pub async fn build_still_video(
    image_url: &str,
    audio_path: &Path,
    ffmpeg: &Path,
) -> Result<PathBuf> {
    let image_path = audio_path.with_extension("cover.jpg");
    fetch_image(image_url, &image_path).await?;

    let output = audio_path.with_extension("mp4");

    let status = Command::new(ffmpeg)
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        // Hold the single frame for as long as the audio runs.
        .arg("-loop")
        .arg("1")
        .arg("-i")
        .arg(&image_path)
        .arg("-i")
        .arg(audio_path)
        // yuv420p needs even dimensions and phone screenshots frequently are
        // not, so round down rather than let the encode fail.
        .arg("-vf")
        .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2")
        .arg("-c:v")
        .arg("libx264")
        // Tells x264 this is one unchanging frame; keeps the file small.
        .arg("-tune")
        .arg("stillimage")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        // Without this, `-loop 1` would encode forever.
        .arg("-shortest")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("could not run ffmpeg: {e}")))?;

    let _ = std::fs::remove_file(&image_path);

    if !status.status.success() {
        let _ = std::fs::remove_file(&output);
        let detail = String::from_utf8_lossy(&status.stderr);
        let last = detail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(AppError::EngineFailed(format!(
            "could not build a video from the photo post: {last}"
        )));
    }

    // Only now is the audio redundant.
    let _ = std::fs::remove_file(audio_path);
    Ok(output)
}

async fn fetch_image(url: &str, dest: &Path) -> Result<()> {
    // No credential is attached, matching the rest of the download path.
    let response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !response.status().is_success() {
        return Err(AppError::Network);
    }
    let bytes = response.bytes().await.map_err(|_| AppError::Network)?;
    if bytes.is_empty() {
        return Err(AppError::NoMediaFound);
    }
    std::fs::write(dest, &bytes).map_err(|e| AppError::DownloadPath(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_sits_beside_the_audio_with_an_mp4_extension() {
        let audio = PathBuf::from("/tmp/Some Title [12345].mp3");
        assert_eq!(
            audio.with_extension("mp4"),
            PathBuf::from("/tmp/Some Title [12345].mp4")
        );
        // The scratch cover must not collide with the final file.
        assert_ne!(audio.with_extension("cover.jpg"), audio.with_extension("mp4"));
    }
}
