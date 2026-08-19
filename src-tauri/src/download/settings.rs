//! Persisted downloader preferences.
//!
//! Deliberately a small JSON file rather than a row in `accounts.sqlite3`: the
//! download domain must not depend on the auth database (see the layering note
//! in [`crate::download`]), and a download folder is a plain user preference,
//! not account metadata.
//!
//! Nothing secret goes in here. A file path is the only field, and a path is
//! already visible in the UI.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download::quality::Quality;

const FILE_NAME: &str = "downloader-settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Absent means "use the OS Downloads folder", which is also what a
    /// deleted or unreadable file resolves to.
    #[serde(default)]
    pub destination: Option<PathBuf>,
    /// Absent means "best available", which is also what an unreadable or
    /// hand-edited value falls back to.
    #[serde(default)]
    pub quality: Quality,
    /// When an Instagram session was captured, or `None` if there is none.
    ///
    /// A non-secret marker, deliberately kept beside the other preferences
    /// rather than derived from the keychain: answering "is Instagram
    /// connected?" by decrypting the session costs a macOS authorization
    /// prompt every time a page renders. The secret itself stays in the
    /// keychain and is read only when a download actually needs it.
    #[serde(default)]
    pub instagram_connected_at: Option<i64>,
    #[serde(default)]
    pub facebook_connected_at: Option<i64>,
    /// Prefer H.264 so downloads open in QuickTime and Photos.
    ///
    /// Defaults to on, including for settings files written before this field
    /// existed: a file that will not open reads as a broken app, while a
    /// 1080p cap is a visible, explicable trade-off.
    #[serde(default = "default_true")]
    pub prefer_compatible: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            destination: None,
            quality: Quality::default(),
            instagram_connected_at: None,
            facebook_connected_at: None,
            prefer_compatible: true,
        }
    }
}

impl Settings {
    /// Read preferences, tolerating every kind of absence.
    ///
    /// A corrupt or hand-edited file must never stop the app from starting, so
    /// any failure here degrades to defaults rather than propagating.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE_NAME))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join(FILE_NAME), json)
    }
}

/// Where downloads go when the user has never chosen, or their choice has
/// become unusable.
pub fn default_destination(os_downloads: Option<PathBuf>, app_data: &Path) -> PathBuf {
    os_downloads
        .unwrap_or_else(|| app_data.to_path_buf())
        .join("Media Downloader")
}

/// Resolve the folder to start with.
///
/// A saved folder that no longer exists - an unplugged external drive, a
/// renamed directory - falls back to the default instead of failing every
/// download with a path error the user cannot see the cause of.
pub fn resolve_destination(settings: &Settings, default: PathBuf) -> PathBuf {
    match &settings.destination {
        Some(chosen) if chosen.is_dir() => chosen.clone(),
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-settings-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trips_a_chosen_folder() {
        let dir = scratch("roundtrip");
        let s = Settings {
            destination: Some(PathBuf::from("/tmp/somewhere")),
            quality: Quality::P1080,
            instagram_connected_at: Some(1_700_000_000),
            facebook_connected_at: None,
            prefer_compatible: false,
        };
        s.save(&dir).unwrap();
        let back = Settings::load(&dir);
        assert_eq!(back.destination, Some(PathBuf::from("/tmp/somewhere")));
        assert_eq!(back.quality, Quality::P1080);
        assert_eq!(back.instagram_connected_at, Some(1_700_000_000));
        assert!(!back.prefer_compatible);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_defaults_not_an_error() {
        let dir = scratch("missing");
        let s = Settings::load(&dir);
        assert!(s.destination.is_none());
        assert_eq!(s.quality, Quality::Best);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_from_before_quality_existed_still_loads() {
        // Upgrading must not reset someone's download folder.
        let dir = scratch("legacy");
        std::fs::write(
            dir.join(FILE_NAME),
            r#"{"destination":"/tmp/legacy-folder"}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.destination, Some(PathBuf::from("/tmp/legacy-folder")));
        assert_eq!(s.quality, Quality::Best);
        assert!(
            s.instagram_connected_at.is_none(),
            "a file written before this field existed must not claim a session"
        );
        assert!(
            s.prefer_compatible,
            "an older settings file must still default to playable output"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_degrades_to_defaults() {
        // Hand-edited JSON must not brick the app on launch.
        let dir = scratch("corrupt");
        std::fs::write(dir.join(FILE_NAME), "{ not json at all").unwrap();
        assert!(Settings::load(&dir).destination.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_folder_that_vanished_falls_back() {
        let default = PathBuf::from("/tmp/default-dl");
        let settings = Settings {
            destination: Some(PathBuf::from("/Volumes/UnpluggedDrive/Videos")),
            ..Settings::default()
        };
        assert_eq!(resolve_destination(&settings, default.clone()), default);
    }

    #[test]
    fn a_saved_folder_that_still_exists_wins() {
        let dir = scratch("still-there");
        let settings = Settings {
            destination: Some(dir.clone()),
            ..Settings::default()
        };
        assert_eq!(
            resolve_destination(&settings, PathBuf::from("/tmp/default-dl")),
            dir
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
