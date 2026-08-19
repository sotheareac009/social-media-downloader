//! Video quality preference and the format selector it produces.
//!
//! Why this is its own module: the selector string is the single most
//! consequential piece of configuration in the downloader, and it has to
//! satisfy two constraints at once.
//!
//!   1. **Never ask for a merge we cannot perform.** Without FFmpeg, a
//!      `video+audio` selector downloads both streams in full and then fails,
//!      which is the worst possible outcome - minutes of bandwidth for
//!      nothing. So the no-FFmpeg branch only ever names single files.
//!
//!   2. **Always leave a fallback.** Every selector ends in a bare `b`, so a
//!      video that has nothing at the requested height still downloads at
//!      whatever it does have, rather than failing as "requested format not
//!      available".
//!
//! Measured against a real YouTube video offering up to 1080p:
//!
//! | selector | with FFmpeg | without |
//! |---|---|---|
//! | Best      | 1080p | 360p |
//! | ≤1080p    | 1080p | 360p |
//! | ≤720p     | 720p  | 360p |
//! | ≤480p     | 480p  | 360p |
//!
//! That right-hand column is the whole reason the UI nags about FFmpeg:
//! without it, the quality picker cannot do anything on YouTube.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// The highest the platform offers.
    #[default]
    Best,
    #[serde(rename = "4320p")]
    P4320,
    #[serde(rename = "2160p")]
    P2160,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
    #[serde(rename = "360p")]
    P360,
}

impl Quality {
    pub const ALL: &'static [Quality] = &[
        Quality::Best,
        Quality::P4320,
        Quality::P2160,
        Quality::P1440,
        Quality::P1080,
        Quality::P720,
        Quality::P480,
        Quality::P360,
    ];

    /// Nearest option at or below a reported tier, used to clamp a saved
    /// preference to what a particular video actually offers.
    pub fn clamp_to(self, available: &[u32]) -> Quality {
        let Some(want) = self.max_height() else {
            return Quality::Best;
        };
        // Already satisfiable.
        if available.iter().any(|h| *h <= want) {
            return self;
        }
        Quality::Best
    }

    /// The height cap, or `None` for "whatever is best".
    pub fn max_height(self) -> Option<u32> {
        match self {
            Quality::Best => None,
            Quality::P4320 => Some(4320),
            Quality::P2160 => Some(2160),
            Quality::P1440 => Some(1440),
            Quality::P1080 => Some(1080),
            Quality::P720 => Some(720),
            Quality::P480 => Some(480),
            Quality::P360 => Some(360),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Best => "Best available",
            Quality::P4320 => "4320p (8K)",
            Quality::P2160 => "2160p (4K)",
            Quality::P1440 => "1440p (2K)",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::P480 => "480p",
            Quality::P360 => "360p",
        }
    }

    /// Build the yt-dlp `-f` argument for this preference.
    ///
    /// `prefer_compatible` puts H.264 (`avc1`) first. That matters more than it
    /// sounds: QuickTime cannot decode VP9 or AV1, so a "successful" download
    /// can produce a file macOS refuses to open, which reads as a broken app.
    /// The cost is real though - on YouTube, H.264 stops at 1080p, and 4K/8K
    /// exist only as VP9/AV1 - so this is a choice, not a default to hide.
    ///
    /// Every branch still ends in a bare `b`, so a site offering no H.264 at
    /// all downloads whatever it has rather than failing.
    pub fn format_selector(self, has_ffmpeg: bool, prefer_compatible: bool) -> String {
        let cap = self
            .max_height()
            .map(|h| format!("[height<={h}]"))
            .unwrap_or_default();

        if !has_ffmpeg {
            // No merger: single files only, or the download fails at the end.
            let compat = if prefer_compatible {
                format!("b{cap}[vcodec^=avc1][ext=mp4]/")
            } else {
                String::new()
            };
            return format!("{compat}b{cap}[ext=mp4]/b{cap}/b[ext=mp4]/b");
        }

        let compat = if prefer_compatible {
            format!("bv*{cap}[vcodec^=avc1]+ba[ext=m4a]/bv*{cap}[vcodec^=avc1]+ba/")
        } else {
            String::new()
        };
        format!("{compat}bv*{cap}[ext=mp4]+ba[ext=m4a]/bv*{cap}+ba/b{cap}[ext=mp4]/b{cap}/b")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_merger_no_selector_may_request_one() {
        // A `+` here means downloading both streams and then failing, which is
        // strictly worse than fetching a lower-quality single file.
        for compat in [true, false] {
            for q in Quality::ALL {
                let f = q.format_selector(false, compat);
                assert!(!f.contains('+'), "{q:?} asked for a merge: {f}");
            }
        }
    }

    #[test]
    fn with_a_merger_every_selector_prefers_the_merged_form() {
        for compat in [true, false] {
            for q in Quality::ALL {
                let f = q.format_selector(true, compat);
                assert!(f.starts_with("bv*"), "{q:?} did not prefer video+audio: {f}");
                assert!(f.contains('+'), "{q:?}: {f}");
            }
        }
    }

    #[test]
    fn every_selector_keeps_a_last_resort() {
        // Without the trailing bare `b`, a video with no matching height fails
        // outright instead of downloading what it has.
        for compat in [true, false] {
            for has_ffmpeg in [true, false] {
                for q in Quality::ALL {
                    let f = q.format_selector(has_ffmpeg, compat);
                    assert!(f.ends_with("/b"), "{q:?} ({has_ffmpeg}, {compat}): {f}");
                }
            }
        }
    }

    #[test]
    fn compatibility_puts_h264_first_and_only_when_asked() {
        // QuickTime cannot play VP9 or AV1, so this ordering decides whether a
        // finished download opens on macOS at all.
        let on = Quality::P1080.format_selector(true, true);
        assert!(on.starts_with("bv*[height<=1080][vcodec^=avc1]"), "{on}");

        let off = Quality::P1080.format_selector(true, false);
        assert!(!off.contains("avc1"), "{off}");
        assert!(off.starts_with("bv*[height<=1080][ext=mp4]"), "{off}");
    }

    #[test]
    fn a_cap_appears_in_every_branch_that_precedes_the_fallback() {
        let f = Quality::P720.format_selector(true, false);
        assert_eq!(f.matches("height<=720").count(), 4, "{f}");
        assert!(!f.contains("height<=1080"));
    }

    #[test]
    fn eight_k_is_offered_and_outranks_four_k() {
        assert_eq!(Quality::P4320.max_height(), Some(4320));
        let i = |q: Quality| Quality::ALL.iter().position(|x| *x == q).unwrap();
        assert!(i(Quality::P4320) < i(Quality::P2160), "8K must sort above 4K");
    }

    #[test]
    fn a_cap_no_video_can_satisfy_falls_back_to_best() {
        // Asking for 8K of a 720p video should download the 720p, not fail.
        assert_eq!(Quality::P4320.clamp_to(&[720, 480, 360]), Quality::P4320);
        assert_eq!(Quality::P360.clamp_to(&[1080, 720]), Quality::Best);
        assert_eq!(Quality::Best.clamp_to(&[1080]), Quality::Best);
    }

    #[test]
    fn best_is_the_default_and_imposes_no_cap() {
        assert_eq!(Quality::default(), Quality::Best);
        assert_eq!(Quality::Best.max_height(), None);
        assert!(!Quality::Best.format_selector(true, true).contains("height"));
    }

    #[test]
    fn quality_round_trips_through_json_as_the_ui_spells_it() {
        // The UI sends "1080p", not "p1080".
        let json = serde_json::to_string(&Quality::P1080).unwrap();
        assert_eq!(json, "\"1080p\"");
        assert_eq!(
            serde_json::from_str::<Quality>("\"1080p\"").unwrap(),
            Quality::P1080
        );
        assert_eq!(
            serde_json::from_str::<Quality>("\"best\"").unwrap(),
            Quality::Best
        );
    }
}
