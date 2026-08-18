//! Provider-agnostic OAuth 2.0 primitives for a *native* application, per
//! RFC 8252 (OAuth for Native Apps) and RFC 7636 (PKCE).
//!
//! Two rules this module exists to enforce:
//!   1. `state` is cryptographically random and is checked on every callback.
//!   2. The authorization code is bound to this process by a PKCE verifier that
//!      never leaves the machine.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::auth::ProviderId;

/// 256 bits of entropy, URL-safe base64 (43 chars). Comfortably above the
/// 128-bit floor RFC 6819 asks for on `state`.
fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE code verifier + its S256 challenge.
#[derive(Clone)]
pub struct Pkce {
    verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        // RFC 7636 allows 43-128 chars; 32 random bytes -> 43 chars.
        let verifier = random_urlsafe(32);
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(digest);
        Self { verifier, challenge }
    }

    /// Deliberately a method rather than a public field: the verifier is a
    /// secret and every read site should be greppable.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn method(&self) -> &'static str {
        "S256"
    }
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &"<redacted>")
            .finish()
    }
}

/// An authorization request that has been started but not yet completed.
///
/// Held in memory by the `AuthManager` for the lifetime of one flow, and
/// dropped as soon as the callback resolves - success or failure.
pub struct PendingFlow {
    pub provider: ProviderId,
    /// The fully-built URL to hand to the system browser.
    pub authorize_url: String,
    /// Exact redirect URI sent in the authorization request. It must be echoed
    /// byte-for-byte in the token exchange or the provider rejects it.
    pub redirect_uri: String,
    pub state: String,
    pub pkce: Pkce,
}

impl PendingFlow {
    pub fn new_state() -> String {
        random_urlsafe(32)
    }

    /// Constant-time-ish comparison of the returned state against the expected
    /// one. Length is compared first, then every byte is visited so an early
    /// mismatch does not shorten the loop.
    pub fn state_matches(&self, returned: Option<&str>) -> bool {
        let Some(returned) = returned else {
            return false;
        };
        let expected = self.state.as_bytes();
        let got = returned.as_bytes();
        if expected.len() != got.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.iter().zip(got.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl std::fmt::Debug for PendingFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingFlow")
            .field("provider", &self.provider)
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .field("authorize_url", &"<redacted>")
            .finish()
    }
}

/// The shape every OAuth 2.0 token endpoint returns (RFC 6749 §5.1).
#[derive(Debug, serde::Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl TokenResponse {
    pub fn into_credential(self, provider: ProviderId, fallback_scopes: &[String]) -> crate::auth::Credential {
        let scopes = self
            .scope
            .as_deref()
            // Providers disagree on the delimiter: RFC 6749 says space, but
            // TikTok returns a comma-separated list. Accept either.
            .map(|s| {
                s.split([' ', ',', '\t'])
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| fallback_scopes.to_vec());

        crate::auth::Credential {
            provider,
            expires_at: self.expires_in.map(|s| crate::auth::now_unix() + s),
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            scopes,
            token_type: self.token_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_random_and_long_enough() {
        let a = PendingFlow::new_state();
        let b = PendingFlow::new_state();
        assert_ne!(a, b);
        assert!(a.len() >= 43, "state too short: {}", a.len());
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = Pkce::generate();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier().as_bytes()));
        assert_eq!(p.challenge, expected);
        assert!((43..=128).contains(&p.verifier().len()));
    }

    fn flow_with_state(state: &str) -> PendingFlow {
        PendingFlow {
            provider: ProviderId::Google,
            authorize_url: String::new(),
            redirect_uri: String::new(),
            state: state.to_string(),
            pkce: Pkce::generate(),
        }
    }

    #[test]
    fn state_validation_rejects_everything_but_an_exact_match() {
        let f = flow_with_state("expected-state-value");
        assert!(f.state_matches(Some("expected-state-value")));
        assert!(!f.state_matches(Some("expected-state-valuX")));
        assert!(!f.state_matches(Some("expected-state-value-extra")));
        assert!(!f.state_matches(Some("")));
        assert!(!f.state_matches(None));
    }

    #[test]
    fn token_response_computes_absolute_expiry() {
        let tr = TokenResponse {
            access_token: "a".into(),
            refresh_token: None,
            expires_in: Some(3600),
            scope: Some("openid profile".into()),
            token_type: "Bearer".into(),
        };
        let before = crate::auth::now_unix();
        let c = tr.into_credential(ProviderId::Google, &[]);
        assert!(c.expires_at.unwrap() >= before + 3600);
        assert_eq!(c.scopes, vec!["openid", "profile"]);
    }
}
