//! Instagram download session.
//!
//! WHY THIS EXISTS, AND WHAT IT COSTS.
//!
//! Every other source in this app downloads with no session at all. Instagram
//! does not permit that: anonymous requests are answered with an empty media
//! response, so a reel cannot be fetched without a logged-in cookie jar.
//!
//! This module therefore holds something strictly more dangerous than anything
//! else in the app. An OAuth token elsewhere carries profile-only scopes. An
//! Instagram `sessionid` cookie *is* the account - it can read DMs, post, and
//! change settings. It is treated accordingly:
//!
//!   * Stored in an owner-only file in the app data directory. Not the OS
//!     keychain: reading that costs a login-password prompt on every app
//!     launch for an unsigned build, which made downloading unusable. The
//!     tools this app is built on - yt-dlp, gallery-dl, instaloader - all keep
//!     the same cookies in the same kind of file, as do `~/.ssh/id_ed25519`
//!     and `~/.aws/credentials`.
//!   * Handed to the engines as a short-lived temp file, deleted immediately
//!     afterwards (see [`super::cookies`]).
//!   * Sent to Instagram and nowhere else - the cookie file is only ever
//!     passed for `Source::Instagram` jobs.
//!   * Captured from a dedicated login window, so no other site's cookies are
//!     read. This is the reason for not using `--cookies-from-browser`, which
//!     would hand yt-dlp the user's entire browser profile.
//!
//! The boundary that changed: downloads are no longer unconditionally
//! session-free. They are session-free for YouTube, Facebook and TikTok, and
//! use an explicitly captured session for Instagram only.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{AppError, Result};

/// Legacy keychain location, read once so an existing Instagram sign-in
/// survives the move to file storage. Written to only by versions before that.
const LEGACY_KEYRING_SERVICE: &str = "com.reach.mediadownloader.download";
const LEGACY_KEYRING_ACCOUNT: &str = "instagram-session";

/// Which platform a captured session belongs to.
///
/// The storage, capture and cookie-jar machinery is identical across
/// platforms; only three things differ per platform, and they live here: the
/// filename, which cookie proves a real login, and the cookie domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionKind {
    Instagram,
    Facebook,
}

impl SessionKind {
    fn file_name(self) -> &'static str {
        match self {
            SessionKind::Instagram => "instagram-session.json",
            SessionKind::Facebook => "facebook-session.json",
        }
    }

    /// A session is "usable" only when the cookie(s) that prove a real login
    /// are present. Anonymous visitors get other cookies (csrftoken, datr),
    /// so their presence means nothing.
    fn required_cookies(self) -> &'static [&'static str] {
        match self {
            // Instagram's login cookie.
            SessionKind::Instagram => &["sessionid"],
            // Facebook needs both: c_user is the account id, xs the secret.
            SessionKind::Facebook => &["c_user", "xs"],
        }
    }

    /// Whether a cookie domain belongs to this platform.
    pub fn domain_matches(self, domain: &str) -> bool {
        let d = domain.trim_start_matches('.').to_ascii_lowercase();
        match self {
            SessionKind::Instagram => d == "instagram.com" || d.ends_with(".instagram.com"),
            SessionKind::Facebook => d == "facebook.com" || d.ends_with(".facebook.com"),
        }
    }
}

/// One cookie, reduced to the fields a Netscape cookie file needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; 0 means a session cookie with no stated expiry.
    pub expires: i64,
}

/// A captured web login: the cookies plus when they were taken.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSession {
    pub cookies: Vec<StoredCookie>,
    pub captured_at: i64,
}

/// The original name, kept so existing Instagram code reads unchanged.
pub type InstagramSession = WebSession;

impl WebSession {
    /// Whether this looks like a real logged-in session for `kind`.
    pub fn is_usable_for(&self, kind: SessionKind) -> bool {
        kind.required_cookies().iter().all(|needed| {
            self.cookies
                .iter()
                .any(|c| c.name == *needed && !c.value.is_empty())
        })
    }

    /// Back-compat shorthand for the Instagram check.
    pub fn is_usable(&self) -> bool {
        self.is_usable_for(SessionKind::Instagram)
    }
}

/// Non-secret view for the UI. Deliberately carries no cookie value - the
/// frontend never needs one and must never be able to read one.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStatus {
    pub connected: bool,
    /// Unix seconds, so the UI can say how old the session is.
    pub captured_at: Option<i64>,
    /// The logged-in account's display name, when it could be fetched.
    pub display_name: Option<String>,
    /// A profile-picture URL (https), when available. Not secret — it is the
    /// same avatar anyone sees, and the CSP already permits https images.
    pub avatar_url: Option<String>,
}

/// Non-secret display metadata for a connected account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionProfile {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

const PROFILES_FILE: &str = "session-profiles.json";

fn profiles_path(dir: &Path) -> PathBuf {
    dir.join(PROFILES_FILE)
}

fn profile_key(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Instagram => "instagram",
        SessionKind::Facebook => "facebook",
    }
}

/// Load the stored display profile for a platform, if any. Never fails hard:
/// a missing or corrupt file just means "no profile", and the account still
/// shows as connected.
pub fn load_profile(dir: &Path, kind: SessionKind) -> Option<SessionProfile> {
    let raw = std::fs::read_to_string(profiles_path(dir)).ok()?;
    let map: std::collections::HashMap<String, SessionProfile> =
        serde_json::from_str(&raw).ok()?;
    map.get(profile_key(kind)).cloned()
}

/// Merge a platform's profile into the shared profiles file.
pub fn save_profile(dir: &Path, kind: SessionKind, profile: &SessionProfile) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::DownloadPath(format!("profiles directory: {e}")))?;
    let mut map: std::collections::HashMap<String, SessionProfile> =
        std::fs::read_to_string(profiles_path(dir))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
    map.insert(profile_key(kind).to_string(), profile.clone());
    let json = serde_json::to_string_pretty(&map)
        .map_err(|_| AppError::Internal("profile encode failed".into()))?;
    std::fs::write(profiles_path(dir), json)
        .map_err(|e| AppError::DownloadPath(format!("profiles file: {e}")))
}

pub fn clear_profile(dir: &Path, kind: SessionKind) {
    if let Ok(raw) = std::fs::read_to_string(profiles_path(dir)) {
        if let Ok(mut map) =
            serde_json::from_str::<std::collections::HashMap<String, SessionProfile>>(&raw)
        {
            map.remove(profile_key(kind));
            if let Ok(json) = serde_json::to_string_pretty(&map) {
                let _ = std::fs::write(profiles_path(dir), json);
            }
        }
    }
}

/// Whether a cookie's domain belongs to Instagram.
///
/// This exists because `Webview::cookies_for_url` cannot answer it. On macOS
/// wry filters with `cookie.domain() == url.domain()` - an exact string
/// comparison - so a `.instagram.com` cookie is never matched by a
/// `www.instagram.com` URL, and the call returns an empty jar every time.
/// Asking for *all* cookies and matching the domain here is the only way to
/// see a session at all.
pub fn is_instagram_domain(domain: &str) -> bool {
    SessionKind::Instagram.domain_matches(domain)
}

pub fn is_facebook_domain(domain: &str) -> bool {
    SessionKind::Facebook.domain_matches(domain)
}

fn path(dir: &Path, kind: SessionKind) -> PathBuf {
    dir.join(kind.file_name())
}

/// Write the session so only the owner can read it.
///
/// Permissions are set as the file is created, not chmod'ed afterwards: the
/// gap between the two is exactly when another process could read a session.
///
/// On Windows there is no POSIX mode. The app data directory
/// (`%APPDATA%\com.reach.mediadownloader`) is already scoped to the user
/// account by its inherited ACL, which is the same protection `~/.aws` and
/// `%USERPROFILE%\.ssh` rely on there.
pub fn save(dir: &Path, kind: SessionKind, session: &WebSession) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::DownloadPath(format!("session directory: {e}")))?;

    let target = path(dir, kind);
    // Replace atomically so a crash mid-write cannot leave a truncated file
    // where a valid session used to be.
    let temp = target.with_extension("json.tmp");

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let blob = serde_json::to_string(session)
        .map_err(|_| AppError::Internal("session encode failed".into()))?;

    {
        let mut file = options
            .open(&temp)
            .map_err(|e| AppError::DownloadPath(format!("session file: {e}")))?;
        file.write_all(blob.as_bytes())
            .map_err(|e| AppError::DownloadPath(format!("session file: {e}")))?;
    }

    std::fs::rename(&temp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        AppError::DownloadPath(format!("session file: {e}"))
    })
}

/// Read the session, falling back to the old keychain entry once.
///
/// A corrupt or hand-edited file is treated as "no session" rather than an
/// error: the worst outcome is being asked to sign in again.
pub fn load(dir: &Path, kind: SessionKind) -> Result<Option<WebSession>> {
    if let Ok(blob) = std::fs::read_to_string(path(dir, kind)) {
        return Ok(serde_json::from_str(&blob).ok());
    }
    // Only Instagram ever lived in the keychain; nothing to migrate otherwise.
    if kind == SessionKind::Instagram {
        return Ok(migrate_from_keychain(dir));
    }
    Ok(None)
}

/// One-time move of a session stored by an earlier version.
///
/// Costs a single keychain prompt, then never again - which is the whole point
/// of the change. Failure is silent: the user simply signs in once more.
fn migrate_from_keychain(dir: &Path) -> Option<WebSession> {
    let entry = keyring::Entry::new(LEGACY_KEYRING_SERVICE, LEGACY_KEYRING_ACCOUNT).ok()?;
    let blob = entry.get_password().ok()?;
    let session: WebSession = serde_json::from_str(&blob).ok()?;

    if save(dir, SessionKind::Instagram, &session).is_ok() {
        // Only remove the old copy once the new one is safely written.
        let _ = entry.delete_credential();
    }
    Some(session)
}

pub fn clear(dir: &Path, kind: SessionKind) -> Result<()> {
    match std::fs::remove_file(path(dir, kind)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppError::DownloadPath(format!("session file: {e}"))),
    }
    if kind != SessionKind::Instagram {
        return Ok(());
    }
    // Also drop any legacy entry, so signing out really signs out.
    if let Ok(entry) = keyring::Entry::new(LEGACY_KEYRING_SERVICE, LEGACY_KEYRING_ACCOUNT) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

// Note: there is deliberately no `status()` here that reads the keychain.
// Answering "is Instagram connected?" by decrypting the session costs a macOS
// authorization prompt on every render, and three pages ask on mount. The UI
// reads a non-secret marker from the settings file instead - see
// `DownloadManager::instagram_status`. This mirrors the same rule stated in
// `auth::storage`, which this module briefly forgot.

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str) -> StoredCookie {
        StoredCookie {
            name: name.into(),
            value: value.into(),
            domain: ".instagram.com".into(),
            path: "/".into(),
            secure: true,
            expires: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-session-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn session() -> InstagramSession {
        InstagramSession {
            cookies: vec![cookie("sessionid", "1234%3Aabcd")],
            captured_at: 42,
        }
    }

    #[test]
    fn a_session_round_trips_through_the_file() {
        let dir = scratch("roundtrip");
        save(&dir, SessionKind::Instagram, &session()).unwrap();
        let back = load(&dir, SessionKind::Instagram).unwrap().expect("session should load");
        assert!(back.is_usable());
        assert_eq!(back.captured_at, 42);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_session_file_is_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = scratch("perms");
            save(&dir, SessionKind::Instagram, &session()).unwrap();
            let mode = std::fs::metadata(path(&dir, SessionKind::Instagram)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "a session cookie must not be world-readable");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn clearing_removes_the_file_and_is_idempotent() {
        let dir = scratch("clear");
        save(&dir, SessionKind::Instagram, &session()).unwrap();
        clear(&dir, SessionKind::Instagram).unwrap();
        assert!(!path(&dir, SessionKind::Instagram).exists());
        // Signing out twice must not error.
        clear(&dir, SessionKind::Instagram).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_reads_as_no_session_rather_than_an_error() {
        let dir = scratch("corrupt");
        std::fs::write(path(&dir, SessionKind::Instagram), "{ not json").unwrap();
        assert!(load(&dir, SessionKind::Instagram).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_temp_file_is_left_behind_after_a_write() {
        let dir = scratch("temp");
        save(&dir, SessionKind::Instagram, &session()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write must clean up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instagram_domains_match_with_or_without_the_leading_dot() {
        // The exact shapes Instagram sets. `.instagram.com` is the one that
        // broke `cookies_for_url`, so it is the important case here.
        for d in [
            ".instagram.com",
            "instagram.com",
            "www.instagram.com",
            "i.instagram.com",
        ] {
            assert!(is_instagram_domain(d), "{d}");
        }
    }

    #[test]
    fn lookalike_domains_are_not_instagram() {
        for d in [
            "instagram.com.evil.test",
            "notinstagram.com",
            "facebook.com",
            "",
        ] {
            assert!(!is_instagram_domain(d), "{d}");
        }
    }

    #[test]
    fn a_session_without_sessionid_is_not_usable() {
        // Instagram hands out csrftoken and mid to anonymous visitors too, so
        // their presence proves nothing about being logged in.
        let anonymous = InstagramSession {
            cookies: vec![cookie("csrftoken", "abc"), cookie("mid", "xyz")],
            captured_at: 0,
        };
        assert!(!anonymous.is_usable());
    }

    #[test]
    fn an_empty_sessionid_does_not_count_as_logged_in() {
        let blank = InstagramSession {
            cookies: vec![cookie("sessionid", "")],
            captured_at: 0,
        };
        assert!(!blank.is_usable());
    }

    #[test]
    fn facebook_needs_both_c_user_and_xs() {
        let only_id = WebSession { cookies: vec![cookie("c_user", "100")], captured_at: 0 };
        assert!(!only_id.is_usable_for(SessionKind::Facebook), "c_user alone is not a login");
        let full = WebSession {
            cookies: vec![cookie("c_user", "100"), cookie("xs", "secret")],
            captured_at: 0,
        };
        assert!(full.is_usable_for(SessionKind::Facebook));
        // The Instagram check must not accept a Facebook jar.
        assert!(!full.is_usable_for(SessionKind::Instagram));
    }

    #[test]
    fn facebook_domain_matching() {
        assert!(is_facebook_domain(".facebook.com"));
        assert!(is_facebook_domain("www.facebook.com"));
        assert!(!is_facebook_domain("notfacebook.com"));
        assert!(!is_facebook_domain("facebook.com.evil.test"));
    }

    #[test]
    fn a_real_session_is_usable() {
        let real = InstagramSession {
            cookies: vec![cookie("csrftoken", "abc"), cookie("sessionid", "1234%3Aabcd")],
            captured_at: 1,
        };
        assert!(real.is_usable());
    }

    #[test]
    fn the_ui_status_type_cannot_carry_a_cookie() {
        // A compile-time guard expressed as a test: if someone adds a value
        // field to SessionStatus, this serialisation check is where the
        // review conversation should start.
        let json = serde_json::to_string(&SessionStatus {
            connected: true,
            captured_at: Some(42),
            display_name: None,
            avatar_url: None,
        })
        .unwrap();
        assert!(!json.contains("cookie"), "{json}");
        assert!(!json.contains("sessionid"), "{json}");
    }
}
