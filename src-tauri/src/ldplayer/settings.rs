//! Persisted device/publishing preferences.
//!
//! A small JSON file beside the downloader's own settings, for the same reason
//! given there: these are plain preferences, not account metadata, and the
//! device layer must not depend on the auth database.
//!
//! Nothing secret lives here. Two executable paths, a device folder, a job
//! limit and a debug flag — all of them already visible in the Settings page.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ldplayer::adb::MediaCollection;

const FILE_NAME: &str = "device-settings.json";

/// Where pushed media lands on the device.
///
/// `/sdcard/Movies` rather than `/sdcard/Download`: Android's MediaStore
/// indexes Movies as video, and every social app's picker reads video from
/// MediaStore. Files dropped in Download are frequently invisible to those
/// pickers, which is the confusing failure this default exists to avoid.
pub const DEFAULT_REMOTE_DIR: &str = "/sdcard/Movies/SocialPublisher";

/// Where pushed images land.
///
/// Separate from the video folder for the same reason the video folder is
/// `Movies`: Android's gallery groups by directory, and photos filed under
/// "Movies" show up in an album called Movies. The MediaStore *type* comes
/// from the file itself, but the album a user sees comes from the path.
pub const DEFAULT_REMOTE_IMAGE_DIR: &str = "/sdcard/Pictures/SocialPublisher";

/// Publishing more than a couple of emulators at once mostly makes each one
/// slower — they share one CPU, one disk and one adb server — and a device
/// under load drops the UI taps a connector depends on.
pub const DEFAULT_MAX_CONCURRENT: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSettings {
    /// Path to `ldconsole.exe`, or the folder containing it. Absent means
    /// "detect automatically", which is also what an unreadable value means.
    #[serde(default)]
    pub ldplayer_path: Option<PathBuf>,
    /// Path to `adb`. Absent prefers the copy bundled with LDPlayer — see
    /// [`crate::ldplayer::adb::Adb::discover`] for why that matters.
    #[serde(default)]
    pub adb_path: Option<PathBuf>,
    #[serde(default = "default_remote_dir")]
    pub remote_dir: String,
    /// Where images go. Defaulted rather than required, so a settings file
    /// written before images were supported still loads.
    #[serde(default = "default_remote_image_dir")]
    pub remote_image_dir: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    /// Keep every adb command and its output in the app log, and capture a
    /// screenshot at each publishing step. Off by default: the screenshots are
    /// large and the logs are only useful while diagnosing a connector.
    #[serde(default)]
    pub verbose_logging: bool,
    /// Delete the pushed file from the device once a job finishes. Off by
    /// default because a failed job's file is the thing you want to inspect.
    #[serde(default)]
    pub cleanup_after_publish: bool,
}

fn default_remote_dir() -> String {
    DEFAULT_REMOTE_DIR.to_string()
}

fn default_remote_image_dir() -> String {
    DEFAULT_REMOTE_IMAGE_DIR.to_string()
}

fn default_max_concurrent() -> usize {
    DEFAULT_MAX_CONCURRENT
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            ldplayer_path: None,
            adb_path: None,
            remote_dir: default_remote_dir(),
            remote_image_dir: default_remote_image_dir(),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            verbose_logging: false,
            cleanup_after_publish: false,
        }
    }
}

impl DeviceSettings {
    /// Read preferences, tolerating every kind of absence: a corrupt or
    /// hand-edited file degrades to defaults rather than stopping startup.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE_NAME))
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(&self.clone().sanitized())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join(FILE_NAME), json)
    }

    /// Clamp anything a hand-edited file could set to a value that would wedge
    /// the queue (zero workers) or thrash the host (twenty).
    fn sanitized(mut self) -> Self {
        self.max_concurrent = self.max_concurrent.clamp(1, 8);
        self.remote_dir = sanitize_dir(&self.remote_dir, default_remote_dir);
        self.remote_image_dir = sanitize_dir(&self.remote_image_dir, default_remote_image_dir);
        self
    }

    /// Absolute path a file with this name will be pushed to, in the folder
    /// that matches its kind.
    pub fn remote_path_for(&self, file_name: &str, collection: MediaCollection) -> String {
        let dir = match collection {
            MediaCollection::Video => &self.remote_dir,
            MediaCollection::Image => &self.remote_image_dir,
        };
        format!("{dir}/{file_name}")
    }
}

/// An absolute, slash-trimmed device path, or the default when the value could
/// not be one. A hand-edited relative path would otherwise push to the shell's
/// working directory, wherever that happens to be.
fn sanitize_dir(value: &str, fallback: fn() -> String) -> String {
    let dir = value.trim().trim_end_matches('/');
    if dir.is_empty() || !dir.starts_with('/') {
        fallback()
    } else {
        dir.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable() {
        let s = DeviceSettings::default();
        assert_eq!(s.remote_dir, DEFAULT_REMOTE_DIR);
        assert_eq!(s.max_concurrent, DEFAULT_MAX_CONCURRENT);
        assert_eq!(
            s.remote_path_for("clip.mp4", MediaCollection::Video),
            "/sdcard/Movies/SocialPublisher/clip.mp4"
        );
        assert_eq!(
            s.remote_path_for("shot.jpg", MediaCollection::Image),
            "/sdcard/Pictures/SocialPublisher/shot.jpg"
        );
    }

    #[test]
    fn a_hand_edited_file_cannot_wedge_the_queue() {
        let s = DeviceSettings { max_concurrent: 0, ..Default::default() }.sanitized();
        assert_eq!(s.max_concurrent, 1);
        let s = DeviceSettings { max_concurrent: 500, ..Default::default() }.sanitized();
        assert_eq!(s.max_concurrent, 8);
    }

    #[test]
    fn a_relative_remote_dir_falls_back_to_the_default() {
        let s = DeviceSettings { remote_dir: "Movies".into(), ..Default::default() }.sanitized();
        assert_eq!(s.remote_dir, DEFAULT_REMOTE_DIR);
    }

    #[test]
    fn trailing_slashes_do_not_produce_a_double_slash_path() {
        let s = DeviceSettings { remote_dir: "/sdcard/Movies/".into(), ..Default::default() }
            .sanitized();
        assert_eq!(s.remote_path_for("a.mp4", MediaCollection::Video), "/sdcard/Movies/a.mp4");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("ldset-{}", uuid::Uuid::new_v4()));
        let mut s = DeviceSettings::default();
        s.verbose_logging = true;
        s.max_concurrent = 3;
        s.save(&dir).unwrap();
        let back = DeviceSettings::load(&dir);
        assert!(back.verbose_logging);
        assert_eq!(back.max_concurrent, 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
