//! Authentication domain: provider-agnostic types, storage, OAuth machinery
//! and the manager that ties them together.
//!
//! Layering rule: nothing in here knows about downloading, yt-dlp or media.
//! It produces an *authenticated account* and a securely stored credential.

pub mod callback;
pub mod manager;
pub mod oauth;
pub mod providers;
pub mod storage;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifier for a supported social platform.
///
/// Serialized as a lowercase string (`"google"`, `"facebook"`) so the React
/// side can use plain string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Google,
    Facebook,
    TikTok,
    Instagram,
}

impl ProviderId {
    pub const ALL: &'static [ProviderId] =
        &[
        ProviderId::Google,
        ProviderId::Facebook,
        ProviderId::Instagram,
        ProviderId::TikTok,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderId::Google => "google",
            ProviderId::Facebook => "facebook",
            ProviderId::TikTok => "tiktok",
            ProviderId::Instagram => "instagram",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "google" => Some(ProviderId::Google),
            "facebook" => Some(ProviderId::Facebook),
            "tiktok" => Some(ProviderId::TikTok),
            "instagram" => Some(ProviderId::Instagram),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A secret bearer credential for a provider.
///
/// SECURITY: `Serialize`/`Deserialize` exist for exactly one reason - so the
/// [`storage::CredentialStore`] can persist it as JSON *inside* the OS keychain
/// blob. No `#[tauri::command]` may accept or return this type, so it never
/// crosses the IPC boundary into JavaScript.
///
/// `Debug` is implemented by hand so that an accidental `{:?}` in a log line
/// prints redaction markers instead of the token.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    pub provider: ProviderId,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds at which `access_token` stops being valid, if known.
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
    pub token_type: String,
}

impl Credential {
    /// True when the access token is expired or within `skew_secs` of expiring.
    pub fn is_expired(&self, skew_secs: i64) -> bool {
        match self.expires_at {
            Some(exp) => now_unix() + skew_secs >= exp,
            // A provider that does not advertise expiry is treated as live;
            // the API call itself is the authority.
            None => false,
        }
    }

    /// Standard-OAuth refreshability. Callers deciding whether to *actually*
    /// refresh must ask the provider instead - see `AuthProvider::can_refresh`,
    /// which some providers override.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("provider", &self.provider)
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("token_type", &self.token_type)
            .finish()
    }
}

/// Non-sensitive account metadata. This *is* safe to send to React and to
/// persist in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub provider: ProviderId,
    /// The provider's own stable id for the user.
    pub external_id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    /// Present only when the provider returns it and it is useful to display.
    pub email: Option<String>,
}

/// Result of a completed authorization: a secret half and a public half.
#[derive(Debug)]
pub struct AuthResult {
    pub credential: Credential,
    pub account: AccountInfo,
}

/// Raw parameters received on the OAuth redirect.
#[derive(Debug, Clone)]
pub struct CallbackData {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
    /// Non-standard, but several providers send a machine-readable sub-reason
    /// alongside `error`. TikTok in particular returns `error=access_denied`
    /// with `error_type=non_sandbox_target` for an app-configuration problem -
    /// reading only `error` would report that to the user as "you cancelled".
    pub error_type: Option<String>,
}

impl CallbackData {
    pub fn from_query(query: &str) -> Self {
        let mut out = CallbackData {
            code: None,
            state: None,
            error: None,
            error_description: None,
            error_type: None,
        };
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            match k.as_ref() {
                "code" => out.code = Some(v.into_owned()),
                "state" => out.state = Some(v.into_owned()),
                "error" => out.error = Some(v.into_owned()),
                "error_description" => out.error_description = Some(v.into_owned()),
                "error_type" => out.error_type = Some(v.into_owned()),
                _ => {}
            }
        }
        out
    }
}

/// Treat an access token as expired this many seconds before it actually is,
/// so a request never goes out with a token that dies in flight.
pub const EXPIRY_SKEW_SECS: i64 = 60;

pub fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
