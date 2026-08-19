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
//!   * Stored in the OS keychain, never in a file, database or log.
//!   * Written to disk only as a short-lived 0600 temp file that yt-dlp reads,
//!     deleted immediately afterwards (see [`super::cookies`]).
//!   * Sent to Instagram and nowhere else - the cookie file is only ever
//!     passed for `Source::Instagram` jobs.
//!   * Captured from a dedicated login window, so no other site's cookies are
//!     read. This is the reason for not using `--cookies-from-browser`, which
//!     would hand yt-dlp the user's entire browser profile.
//!
//! The boundary that changed: downloads are no longer unconditionally
//! session-free. They are session-free for YouTube, Facebook and TikTok, and
//! use an explicitly captured session for Instagram only.

use serde::{Deserialize, Serialize};

use crate::errors::{AppError, Result};

/// Separate from the auth service namespace: this credential is not an
/// account login, has different lifetime rules, and must not be confused with
/// the OAuth entries by anything sweeping the keychain.
const KEYRING_SERVICE: &str = "com.reach.mediadownloader.download";
const KEYRING_ACCOUNT: &str = "instagram-session";

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

/// A captured Instagram login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstagramSession {
    pub cookies: Vec<StoredCookie>,
    pub captured_at: i64,
}

impl InstagramSession {
    /// Whether this looks like a real logged-in session.
    ///
    /// `sessionid` is the one that matters; without it Instagram treats the
    /// request as anonymous and the download fails exactly as before.
    pub fn is_usable(&self) -> bool {
        self.cookies
            .iter()
            .any(|c| c.name == "sessionid" && !c.value.is_empty())
    }
}

/// Non-secret view for the UI. Deliberately carries no cookie value - the
/// frontend never needs one and must never be able to read one.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStatus {
    pub connected: bool,
    /// Unix seconds, so the UI can say how old the session is.
    pub captured_at: Option<i64>,
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
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    d == "instagram.com" || d.ends_with(".instagram.com")
}

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| AppError::Keychain(e.to_string()))
}

pub fn save(session: &InstagramSession) -> Result<()> {
    let blob = serde_json::to_string(session)
        .map_err(|_| AppError::Internal("session encode failed".into()))?;
    entry()?
        .set_password(&blob)
        .map_err(|e| AppError::Keychain(e.to_string()))
}

pub fn load() -> Result<Option<InstagramSession>> {
    match entry()?.get_password() {
        Ok(blob) => Ok(serde_json::from_str(&blob).ok()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Keychain(e.to_string())),
    }
}

pub fn clear() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Keychain(e.to_string())),
    }
}

/// Status for the UI, without decrypting more than necessary.
pub fn status() -> SessionStatus {
    match load() {
        Ok(Some(s)) if s.is_usable() => SessionStatus {
            connected: true,
            captured_at: Some(s.captured_at),
        },
        _ => SessionStatus {
            connected: false,
            captured_at: None,
        },
    }
}

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
        })
        .unwrap();
        assert!(!json.contains("cookie"), "{json}");
        assert!(!json.contains("sessionid"), "{json}");
    }
}
