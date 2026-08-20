//! Where an activated licence key is kept.
//!
//! The key itself is stored verbatim and re-verified on every read. That is
//! deliberate: the file is not trusted, so editing it cannot grant anything -
//! a tampered key simply fails signature verification and the app returns to
//! the activation screen.
//!
//! It lives in the app data directory rather than the OS keychain because it is
//! not a secret in the way a token is: it belongs to the user, it only unlocks
//! software they bought, and putting it in the keychain would raise an
//! authorisation prompt on a path the app takes at every launch.

use std::path::{Path, PathBuf};

use crate::errors::{AppError, Result};

const FILE: &str = "license.key";

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Persist an activated key. Overwrites any previous one.
pub fn save(dir: &Path, raw_key: &str) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::Internal(format!("could not create data dir: {e}")))?;

    let p = path(dir);
    std::fs::write(&p, raw_key.trim().as_bytes())
        .map_err(|e| AppError::Internal(format!("could not save licence: {e}")))?;

    restrict_permissions(&p);
    Ok(())
}

/// Read the stored key, if there is one. The caller must still verify it.
pub fn load(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path(dir)).ok()?;
    let trimmed = raw.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Remove the stored key. Removing something already gone is success.
pub fn clear(dir: &Path) -> Result<()> {
    match std::fs::remove_file(path(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Internal(format!("could not clear licence: {e}"))),
    }
}

/// Owner-only, matching how the other per-user files here are written. Not a
/// security boundary on its own; it just keeps the key out of other accounts'
/// reach on a shared machine.
#[cfg(unix)]
fn restrict_permissions(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-license-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_load_clear_round_trip() {
        let dir = scratch("roundtrip");
        assert!(load(&dir).is_none());

        save(&dir, "SMD1-ABCDEF").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("SMD1-ABCDEF"));

        clear(&dir).unwrap();
        assert!(load(&dir).is_none());
        // Clearing twice is not an error.
        clear(&dir).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn surrounding_whitespace_is_not_persisted() {
        let dir = scratch("trim");
        save(&dir, "  SMD1-ABCDEF \n").unwrap();
        assert_eq!(load(&dir).as_deref(), Some("SMD1-ABCDEF"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_file_reads_as_no_licence() {
        let dir = scratch("empty");
        save(&dir, "   ").unwrap();
        assert!(load(&dir).is_none(), "a blank file must not count as activated");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("perms");
        save(&dir, "SMD1-ABCDEF").unwrap();
        let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "licence file is readable by others");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
