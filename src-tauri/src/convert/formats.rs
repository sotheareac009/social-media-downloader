//! What container to write, and what codecs belong inside it.
//!
//! A container is not a free choice of codecs. WebM takes VP9 and Opus and
//! nothing else; AVI predates AAC and players handle it badly there; MP4 and
//! MOV want `+faststart` or they only begin playing once fully downloaded.
//! Getting any of these wrong produces a file FFmpeg writes happily and a
//! player refuses, which is the worst kind of failure - it looks like it
//! worked.
//!
//! So the format picked in the UI decides the codecs here, rather than being a
//! file extension pasted onto a fixed command.

use serde::Deserialize;

/// A video container the converter can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    /// H.264 + AAC. Plays everywhere; the right default.
    #[default]
    Mp4,
    /// H.264 + AAC in Matroska. Same streams, a container that takes anything.
    Mkv,
    /// H.264 + AAC in QuickTime. What macOS tools expect.
    Mov,
    /// VP9 + Opus. For the web, and the one format that cannot use the
    /// hardware H.264 encoders.
    Webm,
    /// H.264 + MP3, for the old software that still asks for AVI.
    Avi,
    /// Audio only, stripped out as MP3.
    Mp3,
}

impl VideoFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Avi => "avi",
            Self::Mp3 => "mp3",
        }
    }

    /// Whether the platform's H.264 hardware encoder can write this container.
    ///
    /// WebM cannot hold H.264, so a hardware encoder is not merely slower
    /// there - it produces a file the container rejects. MP3 has no video at
    /// all.
    pub fn takes_h264(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mkv | Self::Mov | Self::Avi)
    }

    /// True when the output carries no video stream.
    pub fn is_audio_only(self) -> bool {
        matches!(self, Self::Mp3)
    }

    /// The audio side of the encode.
    pub fn audio_args(self) -> &'static [&'static str] {
        match self {
            // AAC is what every modern player expects.
            Self::Mp4 | Self::Mkv | Self::Mov => &["-c:a", "aac", "-b:a", "160k"],
            // Opus is WebM's audio codec; AAC in WebM is not a valid file.
            Self::Webm => &["-c:a", "libopus", "-b:a", "128k"],
            // AVI's players predate AAC, so MP3 is the compatible choice.
            Self::Avi => &["-c:a", "libmp3lame", "-b:a", "192k"],
            Self::Mp3 => &["-vn", "-c:a", "libmp3lame", "-b:a", "192k"],
        }
    }

    /// The software video encoder for this container.
    ///
    /// Used whenever hardware is off, unavailable, or - for WebM - impossible.
    pub fn software_video_args(self) -> &'static [&'static str] {
        match self {
            Self::Mp4 | Self::Mkv | Self::Mov | Self::Avi => {
                &["-c:v", "libx264", "-preset", "veryfast", "-crf", "21"]
            }
            // `-row-mt 1` is what makes VP9 use more than one core; without it
            // a WebM encode is several times slower for no reason.
            Self::Webm => &[
                "-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-row-mt", "1",
            ],
            Self::Mp3 => &[],
        }
    }

    /// Container-specific flags applied after the codecs.
    pub fn container_args(self) -> &'static [&'static str] {
        match self {
            // Without this the index sits at the end of the file, so a player
            // cannot start until the whole thing has been read.
            Self::Mp4 | Self::Mov => &["-movflags", "+faststart"],
            _ => &[],
        }
    }
}

/// A photo format the converter can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PhotoFormat {
    #[default]
    Jpg,
    Png,
    Webp,
}

impl PhotoFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// Quality settings, which mean different things per format.
    ///
    /// JPEG's `-q:v 2` is near-lossless; the default would visibly soften a
    /// photo that is only being resized. PNG is lossless already, so it takes
    /// a compression level instead. WebP's quality is a 0-100 scale, unrelated
    /// to JPEG's.
    pub fn quality_args(self) -> &'static [&'static str] {
        match self {
            Self::Jpg => &["-q:v", "2"],
            Self::Png => &["-compression_level", "6"],
            Self::Webp => &["-quality", "88"],
        }
    }
}

impl VideoFormat {
    /// Whether this container can carry `codec` as-is.
    ///
    /// A whitelist, deliberately. Guessing wrong here writes a file FFmpeg
    /// produces without complaint and no player opens, which is worse than an
    /// unnecessary re-encode.
    pub fn accepts_video(self, codec: &str) -> bool {
        let codec = codec.to_ascii_lowercase();
        match self {
            // MP4 and MOV carry the MPEG family; QuickTime also reads ProRes,
            // but nothing here writes it, so a copy is all that matters.
            Self::Mp4 | Self::Mov => {
                matches!(codec.as_str(), "h264" | "hevc" | "av1" | "mpeg4")
            }
            // Matroska is the container that takes essentially anything.
            Self::Mkv => matches!(
                codec.as_str(),
                "h264" | "hevc" | "av1" | "vp8" | "vp9" | "mpeg4" | "mpeg2video" | "theora"
            ),
            // WebM is a strict subset of Matroska: these three and no others.
            Self::Webm => matches!(codec.as_str(), "vp8" | "vp9" | "av1"),
            Self::Avi => matches!(codec.as_str(), "mpeg4" | "h264" | "mjpeg"),
            Self::Mp3 => false,
        }
    }

    /// Whether this container can carry `codec` as its audio track.
    pub fn accepts_audio(self, codec: &str) -> bool {
        let codec = codec.to_ascii_lowercase();
        match self {
            Self::Mp4 | Self::Mov => matches!(codec.as_str(), "aac" | "mp3" | "alac"),
            Self::Mkv => matches!(
                codec.as_str(),
                "aac" | "mp3" | "opus" | "vorbis" | "flac" | "ac3" | "eac3" | "dts"
            ),
            Self::Webm => matches!(codec.as_str(), "opus" | "vorbis"),
            Self::Avi => matches!(codec.as_str(), "mp3" | "ac3" | "pcm_s16le"),
            Self::Mp3 => codec == "mp3",
        }
    }
}

/// Every video format, for capability reporting.
pub const ALL_VIDEO: &[VideoFormat] = &[
    VideoFormat::Mp4,
    VideoFormat::Mkv,
    VideoFormat::Mov,
    VideoFormat::Webm,
    VideoFormat::Avi,
    VideoFormat::Mp3,
];

pub const ALL_PHOTO: &[PhotoFormat] = &[PhotoFormat::Jpg, PhotoFormat::Png, PhotoFormat::Webp];

impl VideoFormat {
    /// The encoders this format cannot be written without.
    ///
    /// FFmpeg builds differ in what they carry - Homebrew's ships VP9 but a
    /// minimal build may not - so a format is offered only once the binary in
    /// front of us says it can write it. Discovering that per file instead
    /// would mean a batch that fails one row at a time.
    pub fn required_encoders(self) -> &'static [&'static str] {
        match self {
            Self::Mp4 | Self::Mkv | Self::Mov => &["libx264", "aac"],
            Self::Avi => &["libx264", "libmp3lame"],
            Self::Webm => &["libvpx-vp9", "libopus"],
            Self::Mp3 => &["libmp3lame"],
        }
    }
}

impl PhotoFormat {
    pub fn required_encoders(self) -> &'static [&'static str] {
        match self {
            Self::Jpg => &["mjpeg"],
            Self::Png => &["png"],
            // The one that is genuinely often missing: a Homebrew FFmpeg
            // without libwebp writes nothing here.
            Self::Webp => &["libwebp"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webm_never_gets_h264_or_aac() {
        // Both would produce a file FFmpeg writes and players refuse.
        assert!(!VideoFormat::Webm.takes_h264());
        let audio = VideoFormat::Webm.audio_args().join(" ");
        assert!(audio.contains("libopus"), "{audio}");
        assert!(!audio.contains("aac"), "{audio}");
        assert!(VideoFormat::Webm
            .software_video_args()
            .contains(&"libvpx-vp9"));
    }

    #[test]
    fn the_h264_containers_all_accept_hardware_encoding() {
        for f in [
            VideoFormat::Mp4,
            VideoFormat::Mkv,
            VideoFormat::Mov,
            VideoFormat::Avi,
        ] {
            assert!(f.takes_h264(), "{f:?}");
            assert!(f.software_video_args().contains(&"libx264"), "{f:?}");
        }
    }

    #[test]
    fn avi_takes_mp3_because_its_players_predate_aac() {
        assert!(VideoFormat::Avi.audio_args().contains(&"libmp3lame"));
    }

    #[test]
    fn audio_only_output_drops_the_video_stream() {
        assert!(VideoFormat::Mp3.is_audio_only());
        assert!(VideoFormat::Mp3.audio_args().contains(&"-vn"));
        assert!(VideoFormat::Mp3.software_video_args().is_empty());
        assert!(!VideoFormat::Mp3.takes_h264());
    }

    #[test]
    fn only_the_streaming_containers_ask_for_faststart() {
        // Elsewhere the flag is meaningless, and FFmpeg warns about it.
        assert!(VideoFormat::Mp4.container_args().contains(&"+faststart"));
        assert!(VideoFormat::Mov.container_args().contains(&"+faststart"));
        assert!(VideoFormat::Mkv.container_args().is_empty());
        assert!(VideoFormat::Webm.container_args().is_empty());
    }

    #[test]
    fn every_format_names_the_extension_it_writes() {
        assert_eq!(VideoFormat::Mp4.extension(), "mp4");
        assert_eq!(VideoFormat::Webm.extension(), "webm");
        assert_eq!(PhotoFormat::Jpg.extension(), "jpg");
        assert_eq!(PhotoFormat::Webp.extension(), "webp");
        for f in [PhotoFormat::Jpg, PhotoFormat::Png, PhotoFormat::Webp] {
            assert!(!f.quality_args().is_empty(), "{f:?}");
        }
    }

    #[test]
    fn every_format_declares_what_it_needs_from_the_build() {
        for f in ALL_VIDEO {
            assert!(!f.required_encoders().is_empty(), "{f:?}");
        }
        for f in ALL_PHOTO {
            assert!(!f.required_encoders().is_empty(), "{f:?}");
        }
        // The two that are actually build-dependent in practice.
        assert!(VideoFormat::Webm.required_encoders().contains(&"libvpx-vp9"));
        assert!(PhotoFormat::Webp.required_encoders().contains(&"libwebp"));
    }

    #[test]
    fn a_container_only_admits_the_codecs_it_can_actually_hold() {
        // The pairing that makes a `.ts` an `.mp4` for free.
        assert!(VideoFormat::Mp4.accepts_video("h264"));
        assert!(VideoFormat::Mp4.accepts_audio("aac"));
        // The pairing that must never be copied.
        assert!(!VideoFormat::Webm.accepts_video("h264"));
        assert!(!VideoFormat::Webm.accepts_audio("aac"));
        assert!(VideoFormat::Webm.accepts_video("vp9"));
        assert!(VideoFormat::Webm.accepts_audio("opus"));
        // Matroska takes both sides.
        assert!(VideoFormat::Mkv.accepts_video("h264"));
        assert!(VideoFormat::Mkv.accepts_video("vp9"));
        // Case from ffprobe is not guaranteed.
        assert!(VideoFormat::Mp4.accepts_video("H264"));
        // Audio-only output has no video to accept.
        assert!(!VideoFormat::Mp3.accepts_video("h264"));
        assert!(VideoFormat::Mp3.accepts_audio("mp3"));
        assert!(!VideoFormat::Mp3.accepts_audio("aac"));
    }

    #[test]
    fn the_defaults_are_what_plays_everywhere() {
        assert_eq!(VideoFormat::default(), VideoFormat::Mp4);
        assert_eq!(PhotoFormat::default(), PhotoFormat::Jpg);
    }
}
