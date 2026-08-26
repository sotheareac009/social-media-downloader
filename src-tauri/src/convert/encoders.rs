//! Which encoder does the work.
//!
//! Hardware encoding is a real speed difference - minutes against tens of
//! minutes on a folder of clips - but the encoder names are per-platform and a
//! build of FFmpeg is not guaranteed to carry any of them. So the set is
//! probed once from the actual binary rather than assumed from the OS.

use std::path::Path;
use std::process::Stdio;

/// The encoder a conversion will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HardwareEncoder {
    /// Apple silicon and Intel Macs, through VideoToolbox.
    VideoToolbox,
    /// NVIDIA, through NVENC.
    Nvenc,
    /// Intel Quick Sync.
    Qsv,
    /// AMD on Windows.
    Amf,
    /// Software x264. Slower, smaller files, works everywhere.
    None,
}

impl HardwareEncoder {
    /// The name to show next to the GPU toggle, so "GPU acceleration" is not a
    /// promise the machine cannot keep.
    pub fn label(self) -> &'static str {
        match self {
            Self::VideoToolbox => "Apple VideoToolbox",
            Self::Nvenc => "NVIDIA NVENC",
            Self::Qsv => "Intel Quick Sync",
            Self::Amf => "AMD AMF",
            Self::None => "CPU (x264)",
        }
    }

    /// The FFmpeg arguments that select this encoder and its quality.
    ///
    /// Hardware encoders take a quality *index*, not a CRF, and the numbers
    /// are not comparable between them - each of these was chosen to sit near
    /// x264's CRF 21, which is the point where a re-encode stops being visible
    /// at normal viewing distance.
    pub fn video_args(self) -> &'static [&'static str] {
        match self {
            Self::VideoToolbox => &["-c:v", "h264_videotoolbox", "-q:v", "58"],
            Self::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4", "-cq", "23"],
            Self::Qsv => &["-c:v", "h264_qsv", "-global_quality", "23"],
            Self::Amf => &["-c:v", "h264_amf", "-quality", "balanced", "-qp_i", "23"],
            Self::None => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "21"],
        }
    }
}

/// Ask FFmpeg which encoders it was built with, and pick the platform's.
///
/// Listing is cheap and happens once per batch. Nothing here launches an
/// encode, so a machine that lists an encoder its driver cannot actually run
/// still falls back per-file rather than failing the batch.
pub async fn available_encoder(ffmpeg: &Path) -> HardwareEncoder {
    let Ok(out) = crate::process::command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .await
    else {
        return HardwareEncoder::None;
    };

    let listing = String::from_utf8_lossy(&out.stdout);
    let has = |name: &str| listing.contains(name);

    // Ordered by platform, not by preference: only one of these is ever
    // present on a given machine.
    if cfg!(target_os = "macos") && has("h264_videotoolbox") {
        return HardwareEncoder::VideoToolbox;
    }
    if has("h264_nvenc") {
        return HardwareEncoder::Nvenc;
    }
    if has("h264_qsv") {
        return HardwareEncoder::Qsv;
    }
    if has("h264_amf") {
        return HardwareEncoder::Amf;
    }
    HardwareEncoder::None
}

/// The formats this FFmpeg build can actually write, as extension strings.
///
/// One `-encoders` listing answers for every format, so this costs a single
/// process launch rather than one per option.
pub async fn writable_formats(ffmpeg: &Path) -> (Vec<String>, Vec<String>) {
    let listing = match crate::process::command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        // Without a listing, offer only what every build carries rather than
        // hiding everything and looking broken.
        Err(_) => {
            return (
                vec!["mp4".into(), "mkv".into(), "mov".into()],
                vec!["jpg".into(), "png".into()],
            )
        }
    };

    let has_all = |names: &[&str]| names.iter().all(|n| listing.contains(n));

    let video = super::formats::ALL_VIDEO
        .iter()
        .filter(|f| has_all(f.required_encoders()))
        .map(|f| f.extension().to_string())
        .collect();
    let photo = super::formats::ALL_PHOTO
        .iter()
        .filter(|f| has_all(f.required_encoders()))
        .map(|f| f.extension().to_string())
        .collect();
    (video, photo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encoder_names_a_codec_and_a_quality() {
        for enc in [
            HardwareEncoder::VideoToolbox,
            HardwareEncoder::Nvenc,
            HardwareEncoder::Qsv,
            HardwareEncoder::Amf,
            HardwareEncoder::None,
        ] {
            let args = enc.video_args();
            assert_eq!(args[0], "-c:v", "{enc:?}");
            assert!(args.len() >= 4, "{enc:?} has no quality setting: {args:?}");
            assert!(!enc.label().is_empty());
        }
    }

    #[test]
    fn the_software_fallback_is_the_one_that_works_everywhere() {
        assert!(HardwareEncoder::None.video_args().contains(&"libx264"));
    }
}
