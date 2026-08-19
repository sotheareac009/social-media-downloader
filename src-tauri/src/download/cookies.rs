//! Handing a session to yt-dlp, briefly.
//!
//! yt-dlp reads cookies from a file, so the session held in the keychain has
//! to touch disk to be usable. This module keeps that window as small and as
//! narrow as possible:
//!
//!   * The file lives in the OS temp directory, not beside the downloads.
//!   * It is created with mode 0600 on Unix - owner-only - *before* any
//!     content is written, so it is never briefly world-readable.
//!   * [`CookieFile`] deletes it on drop, including when the download fails or
//!     the job is cancelled mid-flight.
//!
//! Netscape format is what yt-dlp expects: tab-separated
//! `domain / include_subdomains / path / secure / expiry / name / value`.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::download::session::StoredCookie;
use crate::errors::{AppError, Result};

/// A cookie file that removes itself.
pub struct CookieFile {
    path: PathBuf,
}

impl CookieFile {
    /// Write cookies to a fresh owner-only temp file.
    pub fn write(cookies: &[StoredCookie]) -> Result<Self> {
        if cookies.is_empty() {
            return Err(AppError::Internal("no cookies to write".into()));
        }

        let name = format!("md-cookies-{}.txt", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(name);

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Set before the file exists, not with a chmod afterwards: the gap
            // between create and chmod is exactly when another process could
            // read a session cookie.
            options.mode(0o600);
        }

        let mut file = options
            .open(&path)
            .map_err(|e| AppError::DownloadPath(format!("cookie file: {e}")))?;

        writeln!(file, "# Netscape HTTP Cookie File").and_then(|_| {
            for c in cookies {
                // A leading dot is what marks a domain cookie; yt-dlp reads the
                // include-subdomains column, but browsers and Instagram both
                // rely on the dot, so keep them consistent.
                let include_subdomains = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
                let secure = if c.secure { "TRUE" } else { "FALSE" };
                writeln!(
                    file,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.domain,
                    include_subdomains,
                    if c.path.is_empty() { "/" } else { &c.path },
                    secure,
                    c.expires.max(0),
                    c.name,
                    c.value
                )?;
            }
            Ok(())
        })
        .map_err(|e| AppError::DownloadPath(format!("cookie file: {e}")))?;

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        // Best effort by necessity - there is nothing useful to do if this
        // fails, and panicking in a destructor would be worse.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, domain: &str, secure: bool) -> StoredCookie {
        StoredCookie {
            name: name.into(),
            value: format!("{name}-value"),
            domain: domain.into(),
            path: String::new(),
            secure,
            expires: 1_800_000_000,
        }
    }

    #[test]
    fn writes_netscape_format_yt_dlp_can_read() {
        let f = CookieFile::write(&[cookie("sessionid", ".instagram.com", true)]).unwrap();
        let body = std::fs::read_to_string(f.path()).unwrap();

        assert!(body.starts_with("# Netscape HTTP Cookie File"));
        let row: Vec<&str> = body.lines().nth(1).unwrap().split('\t').collect();
        assert_eq!(row.len(), 7, "seven tab-separated fields: {row:?}");
        assert_eq!(row[0], ".instagram.com");
        assert_eq!(row[1], "TRUE", "leading dot means include-subdomains");
        assert_eq!(row[2], "/", "an empty path must become /");
        assert_eq!(row[3], "TRUE");
        assert_eq!(row[5], "sessionid");
    }

    #[test]
    fn a_host_only_cookie_is_not_marked_as_a_domain_cookie() {
        let f = CookieFile::write(&[cookie("mid", "instagram.com", false)]).unwrap();
        let body = std::fs::read_to_string(f.path()).unwrap();
        let row: Vec<&str> = body.lines().nth(1).unwrap().split('\t').collect();
        assert_eq!(row[1], "FALSE");
        assert_eq!(row[3], "FALSE", "insecure cookie must not claim secure");
    }

    #[test]
    fn the_file_is_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = CookieFile::write(&[cookie("sessionid", ".instagram.com", true)]).unwrap();
            let mode = std::fs::metadata(f.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "session cookies must not be readable by others");
        }
    }

    #[test]
    fn dropping_the_handle_deletes_the_file() {
        let path = {
            let f = CookieFile::write(&[cookie("sessionid", ".instagram.com", true)]).unwrap();
            f.path().to_path_buf()
        };
        assert!(!path.exists(), "a session cookie must not outlive the download");
    }

    #[test]
    fn refuses_to_write_an_empty_jar() {
        assert!(CookieFile::write(&[]).is_err());
    }
}
