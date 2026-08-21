//! The nouns of the publishing feature: platforms, accounts, media and jobs.
//!
//! SECURITY POSTURE, stated once here because every other file in this module
//! depends on it: an "account" in this app is *not* a social-media login. It is
//! a pointer to an Android app on an emulator that is already signed in. There
//! is no password field, no token field and no cookie field anywhere in these
//! types, and the database guard test in [`super::store`] fails the build if
//! one ever appears.

use serde::{Deserialize, Serialize};

/// A social network this app can publish to.
///
/// Adding one is meant to be boring: a variant here, a package name, and a
/// connector. Nothing in [`crate::ldplayer`] changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Facebook,
    Instagram,
    Tiktok,
    Youtube,
}

impl Platform {
    pub const ALL: &'static [Platform] = &[
        Platform::Facebook,
        Platform::Instagram,
        Platform::Tiktok,
        Platform::Youtube,
    ];

    /// The platforms publishing is switched ON for.
    ///
    /// Facebook only for now: it is the one whose composer has been driven
    /// end to end against a real instance, screen by screen. The others keep
    /// their variants, packages, connectors and recipes — none of that is
    /// deleted — so switching one back on is an entry in this list rather
    /// than a rebuild.
    ///
    /// Deliberately narrower than [`Self::ALL`], which stays the complete set:
    /// an account stored for a parked platform must still parse, so it can be
    /// seen and removed rather than becoming a row nobody can read or delete.
    pub const AVAILABLE: &'static [Platform] = &[Platform::Facebook];

    /// Whether publishing to this platform is switched on.
    pub fn is_available(self) -> bool {
        Self::AVAILABLE.contains(&self)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Facebook => "facebook",
            Platform::Instagram => "instagram",
            Platform::Tiktok => "tiktok",
            Platform::Youtube => "youtube",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::Facebook => "Facebook",
            Platform::Instagram => "Instagram",
            Platform::Tiktok => "TikTok",
            Platform::Youtube => "YouTube",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.as_str().eq_ignore_ascii_case(s))
    }

    /// The Android package(s) this platform ships under.
    ///
    /// More than one because the same network has regional and "Lite" builds
    /// that are genuinely different apps: an instance signed in to TikTok Lite
    /// is still a usable TikTok endpoint, and refusing it because the package
    /// name has a suffix would be an arbitrary "your account doesn't exist".
    /// The first entry is the canonical one, used when the app has to guess.
    pub fn packages(self) -> &'static [&'static str] {
        match self {
            Platform::Facebook => &["com.facebook.katana", "com.facebook.lite"],
            Platform::Instagram => &["com.instagram.android", "com.instagram.lite"],
            Platform::Tiktok => &[
                "com.zhiliaoapp.musically", // global
                "com.ss.android.ugc.trill", // regional
                "com.zhiliaoapp.musically.go",
            ],
            Platform::Youtube => &["com.google.android.youtube", "com.google.android.apps.youtube.creator"],
        }
    }

    pub fn default_package(self) -> &'static str {
        self.packages()[0]
    }

    /// Whether automatic posting can work against this package.
    ///
    /// The "Lite" builds render their whole UI to a canvas and expose nothing
    /// to Android's accessibility tree — a dump of Facebook Lite's composer
    /// returns 56 nodes with not one label between them. They publish fine;
    /// only the final automated tap is impossible, so this is advisory, not a
    /// reason to refuse the account.
    pub fn supports_auto_post(package: &str) -> bool {
        !package.ends_with(".lite") && !package.ends_with(".go")
    }

    /// Which platform a package belongs to, for auto-detecting what an
    /// instance is signed in to.
    pub fn for_package(package: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.packages().contains(&package))
    }
}

/// How several selected assets are turned into posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostMode {
    /// One post per account carrying every asset — a carousel or album.
    Album,
    /// Each asset becomes its own post, on every selected account.
    Single,
}

impl PostMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "album" => Some(PostMode::Album),
            "single" => Some(PostMode::Single),
            _ => None,
        }
    }
}

/// An endpoint: one social app, on one emulator, already signed in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    /// What the user calls it. Defaults to "<Platform> on <device>" and is
    /// theirs to rename — with four Instagram instances, the device name is
    /// the only thing that tells them apart.
    pub name: String,
    pub platform: Platform,
    /// Foreign key into the device layer: `ld:0`, `adb:emulator-5554`.
    pub ldplayer_instance_id: String,
    /// The exact package on that device, which may be a Lite variant.
    pub package_name: String,
    /// The display name this account posts under, as the app itself renders
    /// it on its composer — "Sambath Sotheareach", not a handle or an id.
    ///
    /// Set, it is checked against the composer before anything is submitted,
    /// so a session that has been switched to another identity is caught
    /// before it posts rather than after. Unset, no check runs and the
    /// account behaves exactly as it did before this existed.
    pub profile_name: Option<String>,
    pub created_at: i64,
}

/// Live state of an account, computed rather than stored — a stored
/// "connected" is a lie the moment somebody closes the emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Device is online and the app is installed. Ready to publish.
    Connected,
    /// Device is online but the app is not installed on it.
    AppMissing,
    /// The device exists but is not running.
    DeviceOffline,
    /// The device this account points at is gone entirely — instance deleted,
    /// phone unplugged.
    DeviceMissing,
}

/// Which kind of post a video becomes on Facebook.
///
/// Sharing a video can put up a chooser — Reel, Post, Story — and the answer
/// changes what gets published, not just how it looks: a Reel is a different
/// surface with a different composer. It is the person's call, so it is
/// carried on the job rather than guessed from the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    /// An ordinary feed post.
    Post,
    Reel,
}

impl VideoFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            VideoFormat::Post => "post",
            VideoFormat::Reel => "reel",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Post => "Post",
            VideoFormat::Reel => "Reel",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "post" => Some(VideoFormat::Post),
            "reel" => Some(VideoFormat::Reel),
            _ => None,
        }
    }
}

impl Default for VideoFormat {
    /// A feed post: it is what "publish this video" means to most people, and
    /// it is the flow the caption and Page targeting were built around.
    fn default() -> Self {
        VideoFormat::Post
    }
}

/// Where one post is going: an account, and which identity on it.
///
/// An account is a signed-in app; a Page is one of the identities that app can
/// publish under. Selecting three Pages on one account is three targets on one
/// account, not three accounts — which is exactly what the picker shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishTarget {
    pub account_id: String,
    /// `None` posts as the profile itself.
    #[serde(default)]
    pub page: Option<String>,
}

/// A Facebook Page an account can post as.
///
/// Not an account: a Page has no login and no device of its own. It is one
/// more identity the same signed-in app can publish under, which is why it
/// hangs off an account rather than sitting beside one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountPage {
    pub name: String,
    /// When the app last confirmed this Page exists, so a stale list can say
    /// so rather than quietly offering somewhere that is no longer yours.
    pub last_seen: i64,
}

/// An account plus everything the UI needs to render its row.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub status: AccountStatus,
    /// Device name, so the list can say "LDPlayer #1" rather than "ld:0".
    pub device_name: Option<String>,
    pub device_online: bool,
    pub detail: Option<String>,
    /// False for apps that expose no accessibility labels, so the UI can say
    /// so before a job is queued rather than after it stops.
    pub supports_auto_post: bool,
    /// Whether publishing to this account's platform is switched on. An
    /// account for a parked platform stays listed — hiding it would leave
    /// something in the database nobody could see or delete — but it is not
    /// offered as a target.
    pub available: bool,
    /// Pages this account can post as. Empty until they are discovered or
    /// added, and the publish picker offers these instead of the profile when
    /// there are any — posting to Pages is what the account is for.
    pub pages: Vec<AccountPage>,
}

/// A local file staged for publishing. The file itself is never copied into
/// the app's storage; this is a reference plus the facts needed to show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub duration_seconds: Option<f64>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Queued, waiting for a worker slot.
    Pending,
    /// Copying the file onto the device and indexing it.
    Uploading,
    /// The connector is driving the Android app.
    Publishing,
    Published,
    /// Stopped on purpose, waiting for the person.
    ///
    /// This status exists because of the safety rule in this feature's brief:
    /// when a platform asks for a login confirmation, a checkpoint or a
    /// captcha, the job stops and hands the emulator back to the user. It is
    /// not a failure — the work so far is intact and the video is on the
    /// device — so calling it one would train people to ignore real failures.
    NeedsAttention,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Published | JobStatus::Failed | JobStatus::Cancelled | JobStatus::NeedsAttention
        )
    }

    /// Only a job that stopped can be started again. Retrying a running job
    /// would double-publish, which is the one mistake a user cannot undo.
    pub fn is_retryable(self) -> bool {
        matches!(self, JobStatus::Failed | JobStatus::Cancelled | JobStatus::NeedsAttention)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Uploading => "uploading",
            JobStatus::Publishing => "publishing",
            JobStatus::Published => "published",
            JobStatus::NeedsAttention => "needs_attention",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => JobStatus::Pending,
            "uploading" => JobStatus::Uploading,
            "publishing" => JobStatus::Publishing,
            "published" => JobStatus::Published,
            "needs_attention" => JobStatus::NeedsAttention,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => return None,
        })
    }
}

/// One publishing attempt: this video, to this account.
#[derive(Debug, Clone, Serialize)]
pub struct PublishJob {
    pub id: String,
    pub media_id: String,
    pub account_id: String,
    pub caption: String,
    #[serde(serialize_with = "job_status")]
    pub status: JobStatus,
    /// 0.0–1.0. Coarse by design: the meaningful units are "file copied" and
    /// "app opened", not bytes, and a byte counter that stalls at 100% while
    /// the connector works is worse than four honest steps.
    pub progress: f64,
    /// What is happening right now, in words. Shown under the progress bar.
    pub step: Option<String>,
    pub error_code: Option<String>,
    pub error: Option<String>,
    /// Latest debug screenshot for this job, when one was taken.
    pub screenshot_path: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// Denormalised for the UI, which renders a job list without joining.
    pub account_name: String,
    pub platform: Platform,
    pub device_id: String,
    /// First asset's name — what a compact row shows.
    pub media_name: String,
    /// Every asset, in carousel order. One entry for a single post.
    pub media_names: Vec<String>,
    /// Convenience for the UI: >1 means this is an album post.
    pub media_count: usize,
}

fn job_status<S: serde::Serializer>(v: &JobStatus, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_round_trip_through_their_wire_names() {
        for p in Platform::ALL {
            assert_eq!(Platform::parse(p.as_str()), Some(*p));
        }
    }

    #[test]
    fn every_platform_package_maps_back_to_its_platform() {
        for p in Platform::ALL {
            for pkg in p.packages() {
                assert_eq!(Platform::for_package(pkg), Some(*p), "{pkg}");
            }
        }
    }

    /// Facebook is the only platform switched on right now. AVAILABLE is
    /// deliberately a subset of ALL rather than a replacement for it: a stored
    /// account for a parked platform still has to parse, or it becomes a row
    /// in the database that cannot be read, shown, or deleted.
    #[test]
    fn only_switched_on_platforms_are_offered() {
        assert_eq!(Platform::AVAILABLE, &[Platform::Facebook]);
        assert!(Platform::Facebook.is_available());
        assert!(!Platform::Youtube.is_available());

        for p in Platform::AVAILABLE {
            assert!(Platform::ALL.contains(p), "{p:?} must stay in ALL");
        }
        // A parked platform still round-trips, so its accounts stay readable.
        assert_eq!(Platform::parse("youtube"), Some(Platform::Youtube));
    }

    #[test]
    fn lite_builds_are_flagged_as_unautomatable() {
        assert!(!Platform::supports_auto_post("com.facebook.lite"));
        assert!(!Platform::supports_auto_post("com.instagram.lite"));
        assert!(!Platform::supports_auto_post("com.zhiliaoapp.musically.go"));
        assert!(Platform::supports_auto_post("com.facebook.katana"));
        assert!(Platform::supports_auto_post("com.instagram.android"));
    }

    #[test]
    fn package_names_are_unique_across_platforms() {
        let mut seen = std::collections::HashSet::new();
        for p in Platform::ALL {
            for pkg in p.packages() {
                assert!(seen.insert(*pkg), "{pkg} is claimed by two platforms");
            }
        }
    }

    #[test]
    fn post_modes_round_trip() {
        assert_eq!(PostMode::parse("album"), Some(PostMode::Album));
        assert_eq!(PostMode::parse("single"), Some(PostMode::Single));
        assert_eq!(PostMode::parse("carousel"), None);
    }

    #[test]
    fn statuses_round_trip() {
        for s in [
            JobStatus::Pending,
            JobStatus::Uploading,
            JobStatus::Publishing,
            JobStatus::Published,
            JobStatus::NeedsAttention,
            JobStatus::Failed,
            JobStatus::Cancelled,
        ] {
            assert_eq!(JobStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn only_stopped_jobs_can_be_retried() {
        assert!(!JobStatus::Publishing.is_retryable());
        assert!(!JobStatus::Published.is_retryable());
        assert!(JobStatus::Failed.is_retryable());
        assert!(JobStatus::NeedsAttention.is_retryable());
    }
}
