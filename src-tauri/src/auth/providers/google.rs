//! Google provider - OAuth 2.0 for a *Desktop app* client.
//!
//! Endpoints are Google's official ones, discovered from
//! <https://accounts.google.com/.well-known/openid-configuration>, pinned here
//! rather than fetched so a DNS-level attacker cannot redirect the flow.
//!
//! Flow: authorization code + PKCE(S256) + loopback redirect, exactly what
//! Google documents for installed apps ("OAuth 2.0 for Mobile & Desktop Apps").
//!
//! Scopes requested are the minimum needed to display "who is connected":
//! `openid` and `userinfo.profile`. No Drive, YouTube, or content scope is
//! requested - this phase authenticates an identity, nothing more.

use async_trait::async_trait;
use url::Url;

use crate::auth::oauth::{PendingFlow, Pkce, TokenResponse};
use crate::auth::providers::{AuthProvider, ProviderDescriptor};
use crate::config;
use crate::auth::{AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::errors::{AppError, Result};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";

const SCOPES: &[&str] = &[
    "openid",
    "https://www.googleapis.com/auth/userinfo.profile",
    // Upload videos to the user's own YouTube channel. No app review is needed
    // for a user uploading to their own channel, though the OAuth consent
    // screen must list this scope and the YouTube Data API must be enabled.
    "https://www.googleapis.com/auth/youtube.upload",
    // Read-only, needed to *show* which channel the upload will go to.
    // `youtube.upload` alone cannot read channel info (`channels.list?mine`).
    "https://www.googleapis.com/auth/youtube.readonly",
];

pub struct GoogleProvider {
    client_id: Option<String>,
    /// Google issues a "client secret" even for Desktop clients, but RFC 8252
    /// §8.5 and Google's own docs acknowledge it is **not** confidential in an
    /// installed app. PKCE is what actually secures this flow; the secret is
    /// sent only because Google's token endpoint still expects it for some
    /// client configurations.
    client_secret: Option<String>,
    http: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client_id: config::read("GOOGLE_CLIENT_ID"),
            client_secret: config::read("GOOGLE_CLIENT_SECRET"),
            http,
        }
    }

    fn client_id(&self) -> Result<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| AppError::ProviderNotConfigured("google".into()))
    }

    fn scopes() -> Vec<String> {
        SCOPES.iter().map(|s| s.to_string()).collect()
    }

    async fn post_token_endpoint(&self, form: &[(&str, &str)]) -> Result<TokenResponse> {
        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(form)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        if !status.is_success() {
            // SECURITY: the error body of a token endpoint can echo back the
            // submitted code. Only the status class is surfaced.
            return Err(AppError::ProviderDenied(format!(
                "Google rejected the token request (HTTP {})",
                status.as_u16()
            )));
        }

        resp.json::<TokenResponse>()
            .await
            .map_err(|_| AppError::MalformedProviderResponse)
    }
}

#[async_trait]
impl AuthProvider for GoogleProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::Google
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::Google,
            display_name: "Google".into(),
            configured: self.is_configured(),
            supports_revocation: true,
            scopes: Self::scopes(),
        }
    }

    fn is_configured(&self) -> bool {
        self.client_id.is_some()
    }

    fn authorize(&self, redirect_uri: &str) -> Result<PendingFlow> {
        let client_id = self.client_id()?;
        let state = PendingFlow::new_state();
        let pkce = Pkce::generate();

        let mut url =
            Url::parse(AUTH_ENDPOINT).map_err(|_| AppError::Internal("bad auth endpoint".into()))?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", pkce.method())
            // Needed to receive a refresh token for an installed app.
            .append_pair("access_type", "offline")
            .append_pair("prompt", "select_account consent");

        Ok(PendingFlow {
            provider: ProviderId::Google,
            authorize_url: url.to_string(),
            redirect_uri: redirect_uri.to_string(),
            state,
            pkce,
        })
    }

    async fn handle_callback(&self, flow: &PendingFlow, callback: CallbackData) -> Result<AuthResult> {
        if let Some(err) = callback.error.as_deref() {
            return Err(match err {
                "access_denied" => AppError::Cancelled,
                other => AppError::ProviderDenied(sanitize_error(other)),
            });
        }

        let code = callback.code.as_deref().ok_or(AppError::MalformedProviderResponse)?;
        let client_id = self.client_id()?;

        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", flow.pkce.verifier()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", &flow.redirect_uri),
        ];
        if let Some(secret) = self.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }

        let token = self.post_token_endpoint(&form).await?;
        let credential = token.into_credential(ProviderId::Google, &Self::scopes());
        let account = self.get_account(&credential).await?;

        Ok(AuthResult { credential, account })
    }

    async fn refresh(&self, credential: &Credential) -> Result<Credential> {
        let refresh_token = credential
            .refresh_token
            .as_deref()
            .ok_or(AppError::CredentialNotFound("google".into()))?;
        let client_id = self.client_id()?;

        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = self.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }

        let token = self.post_token_endpoint(&form).await?;
        let mut fresh = token.into_credential(ProviderId::Google, &credential.scopes);
        // A refresh response usually omits the refresh token; keep the old one
        // so the account does not silently become un-refreshable.
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = credential.refresh_token.clone();
        }
        Ok(fresh)
    }

    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo> {
        #[derive(serde::Deserialize)]
        struct UserInfo {
            sub: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            given_name: Option<String>,
            #[serde(default)]
            picture: Option<String>,
            #[serde(default)]
            email: Option<String>,
        }

        let resp = self
            .http
            .get(USERINFO_ENDPOINT)
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if !resp.status().is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Google rejected the profile request (HTTP {})",
                resp.status().as_u16()
            )));
        }

        let u: UserInfo = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;

        Ok(AccountInfo {
            provider: ProviderId::Google,
            display_name: u
                .name
                .or(u.given_name)
                .or_else(|| u.email.clone())
                .unwrap_or_else(|| "Google account".into()),
            external_id: u.sub,
            avatar_url: u.picture,
            email: u.email,
        })
    }

    async fn revoke(&self, credential: &Credential) -> Result<()> {
        // Best effort: a failure here must not block local disconnect, so the
        // caller treats an Err as advisory.
        let token = credential
            .refresh_token
            .as_deref()
            .unwrap_or(&credential.access_token);

        let resp = self
            .http
            .post(REVOKE_ENDPOINT)
            .form(&[("token", token)])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(AppError::ProviderDenied(format!(
                "Google revocation returned HTTP {}",
                resp.status().as_u16()
            )))
        }
    }
}

/// Provider error codes are short ASCII identifiers. Anything else is dropped
/// rather than passed through into a UI string.
fn sanitize_error(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "unknown_error".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_with_id() -> GoogleProvider {
        GoogleProvider {
            client_id: Some("test-client-id.apps.googleusercontent.com".into()),
            client_secret: None,
            http: reqwest::Client::new(),
        }
    }

    #[test]
    fn authorize_url_is_https_and_official() {
        let p = provider_with_id();
        let flow = p.authorize("http://127.0.0.1:5555/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("accounts.google.com"));

        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555/callback");
        assert_eq!(q["state"], flow.state);
        assert_eq!(q["code_challenge"], flow.pkce.challenge);
        // The verifier must never appear in the URL that goes to the browser.
        assert!(!flow.authorize_url.contains(flow.pkce.verifier()));
    }

    #[test]
    fn requests_expected_scopes() {
        let p = provider_with_id();
        let flow = p.authorize("http://127.0.0.1:1/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let scope = &q["scope"];
        assert!(scope.contains("openid"));
        assert!(scope.contains("userinfo.profile"));
        // Upload + read-only are deliberately included; broader write is not.
        assert!(scope.contains("youtube.upload"));
        assert!(scope.contains("youtube.readonly"));
        for forbidden in ["drive", "gmail", "photoslibrary", "youtube.force-ssl"] {
            assert!(!scope.contains(forbidden), "over-broad scope requested: {scope}");
        }
    }

    #[test]
    fn every_endpoint_is_https() {
        for ep in [AUTH_ENDPOINT, TOKEN_ENDPOINT, USERINFO_ENDPOINT, REVOKE_ENDPOINT] {
            assert!(ep.starts_with("https://"), "{ep} is not HTTPS");
        }
    }

    #[test]
    fn unconfigured_provider_refuses_to_authorize() {
        let p = GoogleProvider {
            client_id: None,
            client_secret: None,
            http: reqwest::Client::new(),
        };
        assert!(!p.is_configured());
        let err = p.authorize("http://127.0.0.1:1/callback").unwrap_err();
        assert_eq!(err.code(), "provider_not_configured");
    }

    #[test]
    fn error_sanitizer_strips_injected_text() {
        assert_eq!(sanitize_error("access_denied"), "access_denied");
        assert_eq!(sanitize_error("<script>alert(1)</script>"), "scriptalert1script");
        assert_eq!(sanitize_error("   "), "unknown_error");
    }
}
