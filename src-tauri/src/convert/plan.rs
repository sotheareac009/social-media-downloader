//! Deciding how much work a file actually needs, and building the filter that
//! does it.
//!
//! THE POINT. Re-encoding is the expensive, lossy option, and most files do not
//! need it. A `.ts` recording holds H.264 and AAC - exactly what an MP4 wants -
//! so becoming an MP4 is a container rewrite, not a re-compression: about a
//! second for an hour of video, and bit-for-bit identical output. A folder that
//! is already 1080p/30/MP4 needs nothing at all.
//!
//! So every file is classified before any process starts:
//!
//!   * [`Work::Skip`] - already exactly what was asked for.
//!   * [`Work::Remux`] - streams are fine, only the container changes.
//!   * [`Work::Encode`] - something about the picture must genuinely change.
//!
//! Getting this wrong in the safe direction costs time; getting it wrong the
//! other way writes a file that does not play, so every rule here is a
//! whitelist rather than an assumption.

use serde::Deserialize;

use super::formats::VideoFormat;
use super::scan::{MediaItem, MediaKind};

/// How a file should be processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// Nothing to do: the file already matches the request.
    Skip,
    /// Copy the streams into the target container.
    Remux,
    /// Decode and re-encode.
    Encode,
}

/// How to fit a picture into an aspect ratio it does not already have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Fill the frame, losing the sides. What most vertical reposts want.
    Crop,
    /// Fit the whole picture, with an enlarged blurred copy behind it.
    Blur,
    /// Fit the whole picture on black bars.
    Pad,
}

/// A target shape, from a platform preset.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Aspect {
    /// Ratio numerator and denominator - 9 and 16 for a vertical post.
    pub w: u32,
    pub h: u32,
    pub fit: Fit,
}

impl Aspect {
    fn ratio(&self) -> f64 {
        self.w as f64 / self.h as f64
    }
}

/// Everything the decision depends on, so it can be tested without a file.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub format: VideoFormat,
    pub height_cap: Option<u32>,
    pub fps_cap: Option<u32>,
    pub aspect: Option<Aspect>,
    pub skip_conforming: bool,
}

/// Classify one file.
pub fn plan_work(item: &MediaItem, req: &Request) -> Work {
    if item.kind == MediaKind::Photo {
        // Photos are small and fast; the only saving worth having is skipping
        // one that is already the right format and size.
        return Work::Encode;
    }

    // Audio-only output always re-encodes unless the source audio is already
    // the target codec, which `accepts_audio` covers below.
    let needs_picture_change = needs_resize(item, req)
        || needs_fps_change(item, req)
        || needs_aspect_change(item, req);

    if needs_picture_change && !req.format.is_audio_only() {
        return Work::Encode;
    }

    let vcodec = item.vcodec.as_deref().unwrap_or("");
    let acodec = item.acodec.as_deref().unwrap_or("");

    let streams_fit = if req.format.is_audio_only() {
        // Extracting audio: copyable only when it is already that codec.
        req.format.accepts_audio(acodec)
    } else {
        req.format.accepts_video(vcodec)
            // A file with no audio at all is still copyable.
            && (acodec.is_empty() || req.format.accepts_audio(acodec))
    };

    if !streams_fit {
        return Work::Encode;
    }

    // The streams are fine and the picture needs nothing. If the container is
    // already right, the file IS the answer.
    let same_container = item
        .path
        .rsplit('.')
        .next()
        .map(|e| e.eq_ignore_ascii_case(req.format.extension()))
        .unwrap_or(false);

    if same_container && req.skip_conforming {
        Work::Skip
    } else {
        Work::Remux
    }
}

fn needs_resize(item: &MediaItem, req: &Request) -> bool {
    match (req.height_cap, item.height) {
        // Only shrinking counts: the cap is never an upscale.
        (Some(cap), Some(h)) => h > cap,
        // An unknown height cannot be ruled out, so re-encode rather than
        // copy a file that might be 4K into a "1080p" batch.
        (Some(_), None) => true,
        _ => false,
    }
}

fn needs_fps_change(item: &MediaItem, req: &Request) -> bool {
    super::target_fps(req.fps_cap, item.fps).is_some()
}

fn needs_aspect_change(item: &MediaItem, req: &Request) -> bool {
    let Some(aspect) = req.aspect else {
        return false;
    };
    match (item.width, item.height) {
        (Some(w), Some(h)) if h > 0 => {
            let current = w as f64 / h as f64;
            // Within a percent is the same shape; 1080x1920 and 1079x1920 are
            // not worth a full re-encode apart.
            (current - aspect.ratio()).abs() > 0.01
        }
        // Unknown dimensions: do the work rather than ship the wrong shape.
        _ => true,
    }
}

/// The canvas a file should be rendered onto for a target aspect.
///
/// Height is capped by the source, not just by the request: turning a 720p
/// landscape clip into a 1080x1920 post would upscale it, producing a larger
/// file with the same detail. The shape is what the preset is for; the pixel
/// count still follows the source.
pub fn canvas_for(aspect: &Aspect, source_height: Option<u32>, cap: Option<u32>) -> (u32, u32) {
    let requested = cap.or(source_height).unwrap_or(1080);
    let height = match source_height {
        Some(src) => requested.min(src),
        None => requested,
    };
    let height = even(height.max(2));
    let width = even(((height as f64) * aspect.ratio()).round() as u32);
    (width.max(2), height)
}

/// H.264 refuses odd dimensions, and several filters quietly misbehave on them.
fn even(n: u32) -> u32 {
    if n % 2 == 0 {
        n
    } else {
        n - 1
    }
}

/// The video filter for a plain resize, or `None` when nothing must change.
pub fn scale_filter(height: Option<u32>) -> Option<String> {
    let h = height?;
    Some(format!("scale=-2:'min({h},ih)'"))
}

/// The filter that fits a picture into a target aspect.
///
/// Returned as a simple `-vf` string for crop and pad. Blur needs two copies of
/// the same input composited together, which only `-filter_complex` can
/// express, so it is returned separately by [`blur_backdrop`].
pub fn aspect_filter(aspect: &Aspect, canvas: (u32, u32)) -> Option<String> {
    let (w, h) = canvas;
    match aspect.fit {
        // `increase` then crop: scale until the frame is covered, then take the
        // middle. Nothing is letterboxed, the sides are lost.
        Fit::Crop => Some(format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}"
        )),
        // `decrease` then pad: the whole picture fits, black fills the rest.
        Fit::Pad => Some(format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:-1:-1:color=black"
        )),
        Fit::Blur => None,
    }
}

/// The `-filter_complex` graph for a blurred backdrop, and the label to map.
///
/// The backdrop is downscaled before blurring and scaled back up: a real
/// gaussian over a full 1080x1920 frame costs more than the encode itself,
/// while blurring a tenth-size copy looks identical once enlarged.
pub fn blur_backdrop(canvas: (u32, u32)) -> String {
    let (w, h) = canvas;
    let (bw, bh) = (even((w / 8).max(16)), even((h / 8).max(16)));
    format!(
        "[0:v]split=2[bg][fg];\
         [bg]scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},gblur=sigma=8,scale={w}:{h}[bgb];\
         [fg]scale={w}:{h}:force_original_aspect_ratio=decrease[fgs];\
         [bgb][fgs]overlay=(W-w)/2:(H-h)/2[v]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ext: &str, w: u32, h: u32, fps: f64, v: &str, a: &str) -> MediaItem {
        MediaItem {
            id: "1".into(),
            path: format!("/tmp/clip.{ext}"),
            file_name: format!("clip.{ext}"),
            directory: "/tmp".into(),
            kind: MediaKind::Video,
            size_bytes: 1,
            duration_seconds: Some(60.0),
            width: Some(w),
            height: Some(h),
            fps: Some(fps),
            vcodec: Some(v.into()),
            acodec: Some(a.into()),
            supported: true,
        }
    }

    fn req(format: VideoFormat, cap: Option<u32>, fps: Option<u32>) -> Request {
        Request {
            format,
            height_cap: cap,
            fps_cap: fps,
            aspect: None,
            skip_conforming: true,
        }
    }

    #[test]
    fn a_ts_recording_becomes_an_mp4_without_re_encoding() {
        // The whole reason this module exists: same streams, wrong box.
        let ts = item("ts", 1280, 720, 30.0, "h264", "aac");
        let plan = plan_work(&ts, &req(VideoFormat::Mp4, Some(1080), Some(30)));
        assert_eq!(plan, Work::Remux);
    }

    #[test]
    fn a_file_that_is_already_right_is_left_alone() {
        let mp4 = item("mp4", 1920, 1080, 30.0, "h264", "aac");
        assert_eq!(
            plan_work(&mp4, &req(VideoFormat::Mp4, Some(1080), Some(30))),
            Work::Skip
        );
        // Unless the user turned that off, in which case it is rewritten.
        let mut r = req(VideoFormat::Mp4, Some(1080), Some(30));
        r.skip_conforming = false;
        assert_eq!(plan_work(&mp4, &r), Work::Remux);
    }

    #[test]
    fn anything_the_picture_needs_forces_a_real_encode() {
        let uhd = item("mp4", 3840, 2160, 30.0, "h264", "aac");
        // Too tall for the cap.
        assert_eq!(
            plan_work(&uhd, &req(VideoFormat::Mp4, Some(1080), None)),
            Work::Encode
        );
        // Too many frames.
        let fast = item("mp4", 1920, 1080, 60.0, "h264", "aac");
        assert_eq!(
            plan_work(&fast, &req(VideoFormat::Mp4, Some(1080), Some(30))),
            Work::Encode
        );
        // Wrong shape for the preset.
        let mut r = req(VideoFormat::Mp4, None, None);
        r.aspect = Some(Aspect { w: 9, h: 16, fit: Fit::Crop });
        assert_eq!(plan_work(&fast, &r), Work::Encode);
    }

    #[test]
    fn codecs_the_container_cannot_hold_force_an_encode() {
        // H.264 has no place in WebM, however right everything else is.
        let mp4 = item("mp4", 1280, 720, 30.0, "h264", "aac");
        assert_eq!(
            plan_work(&mp4, &req(VideoFormat::Webm, None, None)),
            Work::Encode
        );
        // And VP9 has none in MP4's usual pairing.
        let webm = item("webm", 1280, 720, 30.0, "vp9", "opus");
        assert_eq!(
            plan_work(&webm, &req(VideoFormat::Mp4, None, None)),
            Work::Encode
        );
        // MKV takes both, so either one is a container rewrite.
        assert_eq!(
            plan_work(&webm, &req(VideoFormat::Mkv, None, None)),
            Work::Remux
        );
    }

    #[test]
    fn unknown_dimensions_are_never_assumed_to_be_fine() {
        // A file FFmpeg could not measure might be 4K; copying it into a
        // "1080p" batch would quietly break the promise the batch made.
        let mut unknown = item("mp4", 1920, 1080, 30.0, "h264", "aac");
        unknown.height = None;
        assert_eq!(
            plan_work(&unknown, &req(VideoFormat::Mp4, Some(1080), None)),
            Work::Encode
        );
        // With no cap asked for, there is nothing to be unsure about.
        assert_eq!(
            plan_work(&unknown, &req(VideoFormat::Mp4, None, None)),
            Work::Skip
        );
    }

    #[test]
    fn extracting_audio_copies_only_when_it_is_already_that_codec() {
        let with_mp3 = item("mkv", 1280, 720, 30.0, "h264", "mp3");
        assert_eq!(
            plan_work(&with_mp3, &req(VideoFormat::Mp3, None, None)),
            Work::Remux
        );
        let with_aac = item("mp4", 1280, 720, 30.0, "h264", "aac");
        assert_eq!(
            plan_work(&with_aac, &req(VideoFormat::Mp3, None, None)),
            Work::Encode
        );
    }

    #[test]
    fn a_vertical_canvas_never_upscales_the_source() {
        let a = Aspect { w: 9, h: 16, fit: Fit::Crop };
        // 1080p landscape asked for a vertical post: 608x1080, not 1080x1920,
        // because the source has only 1080 lines to give.
        assert_eq!(canvas_for(&a, Some(1080), Some(1920)), (608, 1080));
        // A 4K source capped at 1080 gets the same canvas: the cap binds
        // first, and 1080 * 9/16 rounds to an even 608 either way.
        assert_eq!(canvas_for(&a, Some(2160), Some(1080)), (608, 1080));
        // Both dimensions are even, which H.264 requires.
        let (w, h) = canvas_for(&a, Some(721), None);
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
    }

    #[test]
    fn each_fit_builds_the_filter_that_matches_its_promise() {
        let canvas = (1080, 1920);
        let crop = aspect_filter(&Aspect { w: 9, h: 16, fit: Fit::Crop }, canvas).unwrap();
        assert!(crop.contains("increase"), "{crop}");
        assert!(crop.contains("crop=1080:1920"), "{crop}");

        let pad = aspect_filter(&Aspect { w: 9, h: 16, fit: Fit::Pad }, canvas).unwrap();
        assert!(pad.contains("decrease"), "{pad}");
        assert!(pad.contains("pad=1080:1920"), "{pad}");

        // Blur cannot be a plain -vf: it composites the input with itself.
        assert!(aspect_filter(&Aspect { w: 9, h: 16, fit: Fit::Blur }, canvas).is_none());
        let blur = blur_backdrop(canvas);
        assert!(blur.contains("split=2"), "{blur}");
        assert!(blur.contains("overlay="), "{blur}");
        assert!(blur.ends_with("[v]"), "{blur}");
    }
}
