//! Telegram session storage.
//!
//! Login itself runs in the frontend through GramJS (an MTProto client written
//! in JavaScript), because MTProto is not something this app reimplements in
//! Rust. GramJS produces a `StringSession` - a single opaque string that, like
//! an Instagram `sessionid`, *is* the account: it can read every chat and act
//! as the user.
//!
//! Rust's job is only to persist that string safely, so it survives restarts
//! without living in `localStorage` (readable by any script in the webview).
//! It is written to an owner-only file in the app data directory, exactly like
//! [`crate::download::session`]:
//!
//!   * `0600` on macOS and Linux, the mode set as the file is created.
//!   * On Windows, inside `%APPDATA%`, whose inherited ACL is user-scoped.
//!   * Written atomically via a temp file + rename, so a crash mid-write can't
//!     truncate a valid session.
//!
//! Unavoidable caveat, stated plainly: GramJS runs the crypto in JavaScript, so
//! the session string is handed back to the webview to reconnect on each
//! launch. That is inherent to using a JS MTProto client and is why the value
//! crosses the IPC boundary here, unlike the Instagram cookie which never does.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::errors::{AppError, Result};

const FILE_NAME: &str = "telegram-session.txt";

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Non-secret marker for the UI: connected or not, and since when. The session
/// string itself is never part of this.
#[derive(Debug, Clone, Serialize)]
pub struct TelegramStatus {
    pub connected: bool,
    /// Unix seconds the session file was written.
    pub connected_at: Option<i64>,
}

/// Persist the GramJS session string.
pub fn save(dir: &Path, session: &str) -> Result<()> {
    if session.trim().is_empty() {
        return Err(AppError::Internal("empty telegram session".into()));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::DownloadPath(format!("session directory: {e}")))?;

    let target = path(dir);
    let temp = target.with_extension("txt.tmp");

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    {
        let mut file = options
            .open(&temp)
            .map_err(|e| AppError::DownloadPath(format!("session file: {e}")))?;
        file.write_all(session.as_bytes())
            .map_err(|e| AppError::DownloadPath(format!("session file: {e}")))?;
    }

    std::fs::rename(&temp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        AppError::DownloadPath(format!("session file: {e}"))
    })
}

/// The stored session string, or `None` when signed out.
pub fn load(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path(dir)).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn clear(dir: &Path) -> Result<()> {
    match std::fs::remove_file(path(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::DownloadPath(format!("session file: {e}"))),
    }
}

pub fn status(dir: &Path) -> TelegramStatus {
    match std::fs::metadata(path(dir)) {
        Ok(meta) => {
            let connected_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            TelegramStatus {
                connected: load(dir).is_some(),
                connected_at,
            }
        }
        Err(_) => TelegramStatus {
            connected: false,
            connected_at: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-tg-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_session_round_trips() {
        let dir = scratch("roundtrip");
        save(&dir, "1AaBbCcSESSIONSTRING").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("1AaBbCcSESSIONSTRING"));
        assert!(status(&dir).connected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_session_is_refused() {
        let dir = scratch("empty");
        assert!(save(&dir, "   ").is_err());
        assert!(!status(&dir).connected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_is_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = scratch("perms");
            save(&dir, "session").unwrap();
            let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn clearing_is_idempotent() {
        let dir = scratch("clear");
        save(&dir, "session").unwrap();
        clear(&dir).unwrap();
        assert!(load(&dir).is_none());
        clear(&dir).unwrap(); // signing out twice must not error
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_temp_file_is_left_after_a_write() {
        let dir = scratch("temp");
        save(&dir, "session").unwrap();
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
