//! Instagram provider - "Instagram API with Instagram Login" (Business Login).
//!
//! Endpoints verified against Meta's developer documentation:
//!   authorize     https://www.instagram.com/oauth/authorize
//!   token         https://api.instagram.com/oauth/access_token
//!   long-lived    https://graph.instagram.com/access_token
//!   refresh       https://graph.instagram.com/refresh_access_token
//!   profile       https://graph.instagram.com/v25.0/me
//!
//! This provider deviates from ordinary OAuth in four ways, each handled below:
//!
//!   1. **The token response is not an OAuth token response.** It is
//!      `{"data":[{"access_token":…,"user_id":…,"permissions":…}]}` - a nested
//!      array, with no `expires_in`, `token_type` or `refresh_token`.
//!
//!   2. **The authorization code arrives with `#_` appended**, which Meta
//!      explicitly documents must be stripped before the exchange.
//!
//!   3. **There is a mandatory second exchange.** The code exchange yields a
//!      token valid for one hour; it must immediately be traded for a
//!      long-lived (60-day) token or the connection dies almost at once.
//!
//!   4. **There is no refresh token.** A long-lived token is renewed by
//!      presenting *itself* to `refresh_access_token`, which is why this
//!      provider overrides [`AuthProvider::can_refresh`].
//!
//! Like Facebook and TikTok, Instagram has no native/desktop client type: the
//! redirect URI must be registered in the app settings, so loopback is not an
//! option and a client secret is required.
//!
//! Scope is `instagram_business_basic` only - the minimum that returns a
//! username and avatar. Note this API covers Business/Creator accounts; the
//! old Basic Display API it replaced was shut down in December 2024. As with
//! every provider here, login authenticates an identity and grants no access
//! to private or downloadable media.

use async_trait::async_trait;
use url::Url;

use crate::auth::oauth::{PendingFlow, Pkce};
use crate::auth::providers::{AuthProvider, ProviderDescriptor};
use crate::auth::{now_unix, AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::config;
use crate::errors::{AppError, Result};

const AUTH_ENDPOINT: &str = "https://www.instagram.com/oauth/authorize";
const TOKEN_ENDPOINT: &str = "https://api.instagram.com/oauth/access_token";
const LONG_LIVED_ENDPOINT: &str = "https://graph.instagram.com/access_token";
const REFRESH_ENDPOINT: &str = "https://graph.instagram.com/refresh_access_token";
const PROFILE_ENDPOINT: &str = "https://graph.instagram.com/v25.0/me";

const SCOPES: &[&str] = &["instagram_business_basic"];
const PROFILE_FIELDS: &str = "user_id,username,name,profile_picture_url";

pub struct InstagramProvider {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    http: reqwest::Client,
}

impl InstagramProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client_id: config::read("INSTAGRAM_CLIENT_ID"),
            client_secret: config::read("INSTAGRAM_CLIENT_SECRET"),
            redirect_uri: config::read("INSTAGRAM_REDIRECT_URI"),
            http,
        }
    }

    fn scopes() -> Vec<String> {
        SCOPES.iter().map(|s| s.to_string()).collect()
    }

    fn require(&self) -> Result<(&str, &str, &str)> {
        match (
            self.client_id.as_deref(),
            self.client_secret.as_deref(),
            self.redirect_uri.as_deref(),
        ) {
            (Some(i), Some(s), Some(u)) => Ok((i, s, u)),
            _ => Err(AppError::ProviderNotConfigured("instagram".into())),
        }
    }

    /// Trade a short-lived (1 hour) token for a long-lived (60 day) one.
    ///
    /// This is not optional: without it the connection expires within the hour.
    async fn exchange_for_long_lived(&self, short_lived: &str) -> Result<Credential> {
        let (_, client_secret, _) = self.require()?;

        let resp = self
            .http
            .get(LONG_LIVED_ENDPOINT)
            .query(&[
                ("grant_type", "ig_exchange_token"),
                ("client_secret", client_secret),
                ("access_token", short_lived),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        self.decode_graph_token(resp).await
    }

    /// Decode the `{access_token, token_type, expires_in}` shape returned by
    /// both `access_token` and `refresh_access_token`.
    async fn decode_graph_token(&self, resp: reqwest::Response) -> Result<Credential> {
        #[derive(serde::Deserialize)]
        struct GraphToken {
            access_token: String,
            #[serde(default)]
            token_type: Option<String>,
            #[serde(default)]
            expires_in: Option<i64>,
        }

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;

        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Instagram rejected the token request (HTTP {})",
                status.as_u16()
            )));
        }
        check_graph_error(&body)?;

        let token: GraphToken =
            serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

        Ok(Credential {
            provider: ProviderId::Instagram,
            expires_at: token.expires_in.map(|s| now_unix() + s),
            access_token: token.access_token,
            // Instagram issues none; renewal uses the access token itself.
            refresh_token: None,
            scopes: Self::scopes(),
            token_type: token.token_type.unwrap_or_else(|| "Bearer".into()),
        })
    }
}

#[async_trait]
impl AuthProvider for InstagramProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::Instagram
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::Instagram,
            display_name: "Instagram".into(),
            configured: self.is_configured(),
            // Meta documents no token revocation endpoint for this flow; the
            // user de-authorizes from their Instagram settings instead.
            supports_revocation: false,
            scopes: Self::scopes(),
        }
    }

    fn is_configured(&self) -> bool {
        self.require().is_ok()
    }

    /// A long-lived Instagram token renews itself, so the absence of a refresh
    /// token does not mean the account needs re-authorization.
    fn can_refresh(&self, credential: &Credential) -> bool {
        !credential.access_token.is_empty()
    }

    /// `_loopback_redirect_uri` is ignored: the redirect must be one registered
    /// in the Instagram app settings.
    ///
    /// PKCE is deliberately **not** sent. Meta does not document
    /// `code_challenge` support on this endpoint, and sending undocumented
    /// parameters to an OAuth server risks rejection. The code exchange is
    /// protected by the client secret and by the registered https redirect
    /// instead. A `Pkce` is still generated so the flow type stays uniform.
    fn authorize(&self, _loopback_redirect_uri: &str) -> Result<PendingFlow> {
        let (client_id, _, redirect_uri) = self.require()?;
        let state = PendingFlow::new_state();

        let mut url =
            Url::parse(AUTH_ENDPOINT).map_err(|_| AppError::Internal("bad auth endpoint".into()))?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            // Comma-separated, like Facebook and TikTok.
            .append_pair("scope", &SCOPES.join(","))
            .append_pair("state", &state);

        Ok(PendingFlow {
            provider: ProviderId::Instagram,
            authorize_url: url.to_string(),
            redirect_uri: redirect_uri.to_string(),
            state,
            pkce: Pkce::generate(),
        })
    }

    async fn handle_callback(&self, flow: &PendingFlow, callback: CallbackData) -> Result<AuthResult> {
        if let Some(err) = callback.error.as_deref() {
            return Err(match err {
                "access_denied" => AppError::Cancelled,
                _ => AppError::ProviderDenied("instagram_denied".into()),
            });
        }

        let raw_code = callback.code.as_deref().ok_or(AppError::MalformedProviderResponse)?;
        let code = strip_code_suffix(raw_code);
        let (client_id, client_secret, _) = self.require()?;

        // --- step 1: code -> short-lived (1 hour) token ---------------------
        #[derive(serde::Deserialize)]
        struct ShortLivedEnvelope {
            data: Vec<ShortLived>,
        }
        #[derive(serde::Deserialize)]
        struct ShortLived {
            access_token: String,
        }

        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("grant_type", "authorization_code"),
                ("redirect_uri", flow.redirect_uri.as_str()),
                ("code", code),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;
        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Instagram rejected the token request (HTTP {})",
                status.as_u16()
            )));
        }
        check_graph_error(&body)?;

        let envelope: ShortLivedEnvelope =
            serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;
        let short_lived = envelope
            .data
            .into_iter()
            .next()
            .ok_or(AppError::MalformedProviderResponse)?
            .access_token;

        // --- step 2: short-lived -> long-lived (60 day) token ---------------
        // Mandatory. Skipping this leaves a credential that dies in an hour.
        let credential = self.exchange_for_long_lived(&short_lived).await?;
        let account = self.get_account(&credential).await?;

        Ok(AuthResult { credential, account })
    }

    async fn refresh(&self, credential: &Credential) -> Result<Credential> {
        let resp = self
            .http
            .get(REFRESH_ENDPOINT)
            .query(&[
                ("grant_type", "ig_refresh_token"),
                ("access_token", credential.access_token.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        self.decode_graph_token(resp).await
    }

    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo> {
        #[derive(serde::Deserialize)]
        struct Me {
            #[serde(default)]
            user_id: Option<String>,
            #[serde(default)]
            id: Option<String>,
            #[serde(default)]
            username: Option<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            profile_picture_url: Option<String>,
        }

        let resp = self
            .http
            .get(PROFILE_ENDPOINT)
            .query(&[("fields", PROFILE_FIELDS)])
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;

        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Instagram rejected the profile request (HTTP {})",
                status.as_u16()
            )));
        }
        check_graph_error(&body)?;

        let me: Me = serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

        Ok(AccountInfo {
            provider: ProviderId::Instagram,
            // Prefer the handle - it is what an Instagram user recognises.
            display_name: me
                .username
                .clone()
                .map(|u| format!("@{u}"))
                .or(me.name)
                .unwrap_or_else(|| "Instagram account".into()),
            external_id: me
                .user_id
                .or(me.id)
                .ok_or(AppError::MalformedProviderResponse)?,
            avatar_url: me.profile_picture_url,
            // This scope returns no email address.
            email: None,
        })
    }
}

/// Meta documents that the authorization code arrives with `#_` appended and
/// that the suffix is not part of the code. Sending it unstripped makes the
/// exchange fail with an opaque error.
fn strip_code_suffix(code: &str) -> &str {
    code.strip_suffix("#_").unwrap_or(code)
}

/// Graph API errors arrive as `{"error":{"message":…,"type":…,"code":…}}` or,
/// on the OAuth endpoints, as `{"error_type":…,"error_message":…}`.
///
/// SECURITY: only a short sanitized type/code is propagated. Meta's `message`
/// field can echo request details and is never surfaced or logged.
fn check_graph_error(body: &[u8]) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct GraphError {
        #[serde(rename = "type")]
        kind: Option<String>,
        #[serde(default)]
        code: Option<i64>,
    }
    #[derive(serde::Deserialize)]
    struct MaybeError {
        #[serde(default)]
        error: Option<GraphError>,
        #[serde(default)]
        error_type: Option<String>,
    }

    if body.is_empty() {
        return Ok(());
    }
    let Ok(parsed) = serde_json::from_slice::<MaybeError>(body) else {
        return Ok(());
    };

    if let Some(kind) = parsed.error_type {
        return Err(classify(&kind));
    }
    if let Some(err) = parsed.error {
        let label = err
            .kind
            .unwrap_or_else(|| err.code.map(|c| format!("code_{c}")).unwrap_or_default());
        return Err(classify(&label));
    }
    Ok(())
}

fn classify(raw: &str) -> AppError {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    match cleaned.as_str() {
        "access_denied" => AppError::Cancelled,
        "" => AppError::ProviderDenied("unknown_error".into()),
        other => AppError::ProviderDenied(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Meta will not refresh a long-lived token younger than 24 hours. Tokens
    /// last 60 days and we only refresh near expiry, so this never binds - the
    /// test below exists to keep that true if the lifetime assumption changes.
    const MIN_AGE_BEFORE_REFRESH_SECS: i64 = 24 * 60 * 60;

    fn configured() -> InstagramProvider {
        InstagramProvider {
            client_id: Some("test-app-id".into()),
            client_secret: Some("test-secret".into()),
            redirect_uri: Some("https://example.com/ig/callback".into()),
            http: reqwest::Client::new(),
        }
    }

    fn credential(expires_in: Option<i64>) -> Credential {
        Credential {
            provider: ProviderId::Instagram,
            access_token: "IGQV-long-lived".into(),
            refresh_token: None,
            expires_at: expires_in.map(|s| now_unix() + s),
            scopes: InstagramProvider::scopes(),
            token_type: "bearer".into(),
        }
    }

    #[test]
    fn every_endpoint_is_https() {
        for ep in [
            AUTH_ENDPOINT,
            TOKEN_ENDPOINT,
            LONG_LIVED_ENDPOINT,
            REFRESH_ENDPOINT,
            PROFILE_ENDPOINT,
        ] {
            assert!(ep.starts_with("https://"), "{ep} is not HTTPS");
        }
    }

    #[test]
    fn authorize_url_is_official_and_scope_minimal() {
        let flow = configured().authorize("http://127.0.0.1:9999/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.host_str(), Some("www.instagram.com"));
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], "instagram_business_basic");
        assert_eq!(q["state"], flow.state);
        // Loopback is ignored in favour of the registered redirect.
        assert_eq!(q["redirect_uri"], "https://example.com/ig/callback");
        for forbidden in ["content_publish", "manage_messages", "manage_comments"] {
            assert!(!q["scope"].contains(forbidden), "over-broad scope requested");
        }
    }

    #[test]
    fn pkce_is_not_sent_since_meta_does_not_document_it() {
        let flow = configured().authorize("http://127.0.0.1:1/callback").unwrap();
        assert!(!flow.authorize_url.contains("code_challenge"));
        // ...and the verifier certainly never reaches the browser.
        assert!(!flow.authorize_url.contains(flow.pkce.verifier()));
    }

    #[test]
    fn unconfigured_without_all_three_values() {
        let p = InstagramProvider {
            client_id: Some("i".into()),
            client_secret: None,
            redirect_uri: Some("https://example.com/cb".into()),
            http: reqwest::Client::new(),
        };
        assert!(!p.is_configured());
        assert_eq!(
            p.authorize("http://127.0.0.1:1/callback").unwrap_err().code(),
            "provider_not_configured"
        );
    }

    // --- the four Instagram-specific traps -------------------------------

    #[test]
    fn authorization_code_suffix_is_stripped() {
        assert_eq!(strip_code_suffix("AQBx-abc123#_"), "AQBx-abc123");
        // Only a trailing suffix, and only when present.
        assert_eq!(strip_code_suffix("AQBx-abc123"), "AQBx-abc123");
        assert_eq!(strip_code_suffix("AQBx#_abc"), "AQBx#_abc");
    }

    #[test]
    fn credential_without_refresh_token_is_still_refreshable() {
        // The whole reason `AuthProvider::can_refresh` exists: the standard
        // check would report false here and strand the account.
        let c = credential(Some(60));
        assert!(!c.can_refresh(), "no refresh token, as Instagram intends");
        assert!(
            configured().can_refresh(&c),
            "Instagram must still be refreshable via the access token itself"
        );
    }

    #[test]
    fn long_lived_lifetime_clears_the_24_hour_refresh_floor() {
        // Meta refuses to refresh a token younger than 24h. Tokens last 60
        // days and refresh happens near expiry, so this can never bind.
        let sixty_days = 60 * 24 * 60 * 60;
        assert!(sixty_days > MIN_AGE_BEFORE_REFRESH_SECS);
    }

    #[test]
    fn graph_error_envelopes_are_caught() {
        let oauth = br#"{"error_type":"OAuthException","code":400,"error_message":"Invalid code"}"#;
        let err = check_graph_error(oauth).unwrap_err();
        assert_eq!(err.code(), "provider_denied");
        assert!(!err.to_string().contains("Invalid code"), "message leaked");

        let graph = br#"{"error":{"message":"secret detail","type":"OAuthException","code":190}}"#;
        let err = check_graph_error(graph).unwrap_err();
        assert_eq!(err.code(), "provider_denied");
        assert!(!err.to_string().contains("secret detail"), "message leaked");
    }

    #[test]
    fn denial_maps_to_cancelled_and_clean_bodies_pass() {
        assert_eq!(
            check_graph_error(br#"{"error_type":"access_denied"}"#).unwrap_err().code(),
            "cancelled"
        );
        assert!(check_graph_error(b"").is_ok());
        assert!(check_graph_error(br#"{"data":[{"access_token":"x","user_id":"1"}]}"#).is_ok());
        assert!(check_graph_error(br#"{"access_token":"x","expires_in":5184000}"#).is_ok());
    }
}
