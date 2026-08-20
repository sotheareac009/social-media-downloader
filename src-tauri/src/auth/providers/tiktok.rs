//! TikTok provider - Login Kit v2.
//!
//! Endpoints verified against TikTok's developer documentation:
//!   authorize  https://www.tiktok.com/v2/auth/authorize/
//!   token      https://open.tiktokapis.com/v2/oauth/token/
//!   revoke     https://open.tiktokapis.com/v2/oauth/revoke/
//!   user info  https://open.tiktokapis.com/v2/user/info/
//!
//! Three things make TikTok differ from every other provider here, and each one
//! is a place a generic OAuth implementation would silently break:
//!
//!   1. The client identifier parameter is `client_key`, **not** `client_id`.
//!   2. Scopes are **comma**-separated, not space-separated.
//!   3. The API returns **HTTP 200 with an `error` object** on failure, so the
//!      status code alone is not a success signal. See [`check_api_error`].
//!
//! TikTok ships a distinct **Login Kit for Desktop** product, and its redirect
//! rules are the inverse of the web one: `localhost` and `127.0.0.1` are the
//! *only* permitted hosts, a port is mandatory, and a wildcard port (`*`) is
//! supported and recommended for ephemeral ports. So this provider uses the
//! same loopback listener as Google - no hosted redirect page is needed.
//!
//! Register this once in the TikTok app's Login Kit settings:
//!
//! ```text
//! http://127.0.0.1:*/callback
//! ```
//!
//! PKCE is mandatory for desktop - TikTok rejects the authorization request
//! outright with `error_type=code_challenge` if it is missing.
//!
//! Scope is `user.info.basic` only - the minimum that returns an open id,
//! display name and avatar. Note that Login Kit authenticates an identity and
//! grants no access to private or downloadable media.

use async_trait::async_trait;
use url::Url;

use crate::auth::oauth::{PendingFlow, Pkce, TokenResponse};
use crate::auth::providers::{AuthProvider, ProviderDescriptor};
use crate::auth::{AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::config;
use crate::errors::{AppError, Result};

const AUTH_ENDPOINT: &str = "https://www.tiktok.com/v2/auth/authorize/";
const TOKEN_ENDPOINT: &str = "https://open.tiktokapis.com/v2/oauth/token/";
const REVOKE_ENDPOINT: &str = "https://open.tiktokapis.com/v2/oauth/revoke/";
const USERINFO_ENDPOINT: &str = "https://open.tiktokapis.com/v2/user/info/";

/// Scopes requested at login.
///
/// `user.info.basic` shows who is connected. The two video scopes exist for the
/// Upload page and were added deliberately - they are a real expansion of what
/// the app asks for, and the consent screen now says so to the user:
///
///   `video.upload`  - send to the creator's DRAFTS, they publish in the app
///   `video.publish` - post DIRECTLY to the profile
///
/// `video.publish` additionally requires "Direct Post" to be enabled on the app
/// in TikTok's portal. If login starts failing with `scope_not_authorized`,
/// that configuration is missing and dropping this one entry restores it.
///
/// Note that TikTok restricts everything an *unaudited* app posts to private
/// viewing regardless of scope.
const SCOPES: &[&str] = &["user.info.basic", "video.upload", "video.publish"];
const USERINFO_FIELDS: &str = "open_id,union_id,display_name,avatar_url";

pub struct TikTokProvider {
    /// TikTok's name for the public client identifier.
    client_key: Option<String>,
    client_secret: Option<String>,
    http: reqwest::Client,
}

impl TikTokProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client_key: config::read("TIKTOK_CLIENT_KEY"),
            client_secret: config::read("TIKTOK_CLIENT_SECRET"),
            http,
        }
    }

    fn scopes() -> Vec<String> {
        SCOPES.iter().map(|s| s.to_string()).collect()
    }

    fn require(&self) -> Result<(&str, &str)> {
        match (self.client_key.as_deref(), self.client_secret.as_deref()) {
            (Some(k), Some(s)) => Ok((k, s)),
            _ => Err(AppError::ProviderNotConfigured("tiktok".into())),
        }
    }

    /// POST to the token endpoint and decode the credential.
    ///
    /// SECURITY: the response body carries tokens, so it is decoded straight
    /// into `TokenResponse` and never stringified into an error or a log.
    async fn post_token_endpoint(&self, form: &[(&str, &str)]) -> Result<TokenResponse> {
        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(form)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;

        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "TikTok rejected the token request (HTTP {})",
                status.as_u16()
            )));
        }

        // A 200 can still be a failure; TikTok signals it in the body.
        check_api_error(&body)?;

        serde_json::from_slice::<TokenResponse>(&body)
            .map_err(|_| AppError::MalformedProviderResponse)
    }
}

#[async_trait]
impl AuthProvider for TikTokProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::TikTok
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::TikTok,
            display_name: "TikTok".into(),
            configured: self.is_configured(),
            supports_revocation: true,
            scopes: Self::scopes(),
        }
    }

    fn is_configured(&self) -> bool {
        self.require().is_ok()
    }

    /// Uses the ephemeral loopback redirect. TikTok's desktop product requires
    /// a loopback host and supports a wildcard port, so the port the OS just
    /// handed us needs no prior registration.
    fn authorize(&self, redirect_uri: &str) -> Result<PendingFlow> {
        let (client_key, _) = self.require()?;
        let state = PendingFlow::new_state();
        let pkce = Pkce::generate();

        let mut url =
            Url::parse(AUTH_ENDPOINT).map_err(|_| AppError::Internal("bad auth endpoint".into()))?;
        url.query_pairs_mut()
            // `client_key`, not `client_id` - TikTok rejects the latter.
            .append_pair("client_key", client_key)
            .append_pair("response_type", "code")
            // Comma-separated, unlike the space-separated OAuth default.
            .append_pair("scope", &SCOPES.join(","))
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", &state)
            // HEX, not base64url. TikTok documents `code_challenge` as the hex
            // encoding of SHA256(code_verifier) even though it names the method
            // S256; the RFC 7636 base64url form is rejected as an invalid code
            // challenge.
            .append_pair("code_challenge", &pkce.challenge_hex())
            .append_pair("code_challenge_method", pkce.method());

        Ok(PendingFlow {
            provider: ProviderId::TikTok,
            authorize_url: url.to_string(),
            redirect_uri: redirect_uri.to_string(),
            state,
            pkce,
        })
    }

    async fn handle_callback(&self, flow: &PendingFlow, callback: CallbackData) -> Result<AuthResult> {
        // `error_type` is checked first and deliberately: TikTok reports app
        // configuration problems as `error=access_denied` *plus* a specific
        // `error_type`. Reading `error` alone would tell the user they
        // cancelled a sign-in they actually completed.
        if let Some(kind) = callback.error_type.as_deref() {
            if let Some(explanation) = explain_error_type(kind) {
                return Err(AppError::ProviderConfiguration(explanation));
            }
        }
        if let Some(err) = callback.error.as_deref() {
            return Err(match err {
                "access_denied" => AppError::Cancelled,
                _ => AppError::ProviderDenied("tiktok_denied".into()),
            });
        }

        let code = callback.code.as_deref().ok_or(AppError::MalformedProviderResponse)?;
        let (client_key, client_secret) = self.require()?;

        let token = self
            .post_token_endpoint(&[
                ("client_key", client_key),
                ("client_secret", client_secret),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", flow.redirect_uri.as_str()),
                ("code_verifier", flow.pkce.verifier()),
            ])
            .await?;

        let credential = token.into_credential(ProviderId::TikTok, &Self::scopes());
        let account = self.get_account(&credential).await?;
        Ok(AuthResult { credential, account })
    }

    async fn refresh(&self, credential: &Credential) -> Result<Credential> {
        let refresh_token = credential
            .refresh_token
            .as_deref()
            .ok_or_else(|| AppError::CredentialNotFound("tiktok".into()))?;
        let (client_key, client_secret) = self.require()?;

        let token = self
            .post_token_endpoint(&[
                ("client_key", client_key),
                ("client_secret", client_secret),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await?;

        let mut fresh = token.into_credential(ProviderId::TikTok, &credential.scopes);
        // TikTok may rotate the refresh token, but if it omits one entirely we
        // keep the previous value so the account stays refreshable.
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = credential.refresh_token.clone();
        }
        Ok(fresh)
    }

    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            data: UserData,
        }
        #[derive(serde::Deserialize)]
        struct UserData {
            user: User,
        }
        #[derive(serde::Deserialize)]
        struct User {
            open_id: String,
            #[serde(default)]
            display_name: Option<String>,
            #[serde(default)]
            avatar_url: Option<String>,
        }

        let resp = self
            .http
            .get(USERINFO_ENDPOINT)
            .query(&[("fields", USERINFO_FIELDS)])
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;

        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "TikTok rejected the profile request (HTTP {})",
                status.as_u16()
            )));
        }
        check_api_error(&body)?;

        let env: Envelope =
            serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

        Ok(AccountInfo {
            provider: ProviderId::TikTok,
            display_name: env
                .data
                .user
                .display_name
                .unwrap_or_else(|| "TikTok account".into()),
            avatar_url: env.data.user.avatar_url,
            external_id: env.data.user.open_id,
            // Login Kit's basic scope returns no email address.
            email: None,
        })
    }

    async fn revoke(&self, credential: &Credential) -> Result<()> {
        let (client_key, client_secret) = self.require()?;

        let resp = self
            .http
            .post(REVOKE_ENDPOINT)
            .form(&[
                ("client_key", client_key),
                ("client_secret", client_secret),
                ("token", credential.access_token.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "TikTok revocation returned HTTP {}",
                status.as_u16()
            )));
        }

        // Success is an empty body, but an error object may still appear.
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;
        check_api_error(&body)
    }
}

/// TikTok answers failures with HTTP 200 and an `error` object, so every
/// response body must be inspected before it is trusted.
///
/// Two shapes exist in the wild: the Open API envelope
/// `{"error": {"code": "ok", ...}}` used by `/v2/user/info/`, and the OAuth
/// style `{"error": "invalid_grant", "error_description": "..."}` used by
/// `/v2/oauth/token/`. Both are handled.
///
/// SECURITY: only the provider's short error *code* is propagated, never the
/// message or `log_id`, and never the body itself.
pub(crate) fn check_api_error(body: &[u8]) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum ErrorField {
        Structured { code: String },
        Message(String),
    }
    #[derive(serde::Deserialize)]
    struct MaybeError {
        #[serde(default)]
        error: Option<ErrorField>,
        /// OAuth-style detail from `/v2/oauth/token/`. This names the parameter
        /// TikTok objected to, which is the difference between a developer
        /// being able to fix a misconfiguration and staring at
        /// "invalid_request".
        ///
        /// Only the OAuth `error_description` is propagated - never `message`
        /// from the Open API envelope, which can carry request detail.
        #[serde(default)]
        error_description: Option<String>,
    }

    // An empty body (a successful revoke) is not an error.
    if body.is_empty() {
        return Ok(());
    }

    let Ok(parsed) = serde_json::from_slice::<MaybeError>(body) else {
        // Not JSON we recognise; let the caller's own decode decide.
        return Ok(());
    };

    let detail = parsed.error_description.as_deref().map(sanitize_detail);
    let code = match parsed.error {
        None => return Ok(()),
        Some(ErrorField::Structured { code }) => code,
        Some(ErrorField::Message(code)) => code,
    };

    // TikTok uses "ok" for success in the Open API envelope; an empty string
    // appears in some responses and likewise means no error.
    if code.is_empty() || code == "ok" {
        return Ok(());
    }
    if code == "access_denied" {
        return Err(AppError::Cancelled);
    }
    Err(AppError::ProviderDenied(match detail {
        Some(d) if !d.is_empty() => format!("{} - {d}", sanitize_code(&code)),
        _ => sanitize_code(&code),
    }))
}

/// Turn TikTok's `error_type` into operator-facing guidance.
///
/// Returns `None` for values that genuinely mean "the user said no", so those
/// still map to a plain cancellation.
fn explain_error_type(kind: &str) -> Option<String> {
    let text = match kind {
        "non_sandbox_target" => {
            "This TikTok app is in Sandbox mode and the account you signed in with \
             is not one of its target users. In the TikTok developer portal open \
             your app, go to Sandbox -> Target users -> Add account, sign in as \
             that account and accept the developer terms. TikTok can take up to \
             an hour to apply the change."
        }
        "code_challenge" => {
            "TikTok rejected the PKCE challenge. Desktop apps must send \
             code_challenge with code_challenge_method=S256."
        }
        "redirect_uri" => {
            "TikTok did not recognise the redirect URI. Register \
             http://127.0.0.1:*/callback under Login Kit -> Redirect URI. Only \
             localhost and 127.0.0.1 are allowed, a port is required, and the \
             wildcard port covers the ephemeral port this app binds."
        }
        "client_key" => {
            "TikTok did not accept the client key. Check TIKTOK_CLIENT_KEY, and \
             confirm the Login Kit product is added to the app."
        }
        "scope_not_authorized" => {
            "This TikTok app is not authorized for the user.info.basic scope. \
             Add Login Kit and request that scope in the developer portal."
        }
        // Anything else, including a plain user refusal, is not a config issue.
        _ => return None,
    };
    // Collapse the line continuations above into single spaces.
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Keep an OAuth `error_description` readable but bounded.
///
/// Printable ASCII only and length-capped, so a hostile or oversized body
/// cannot inject control characters or flood the interface.
fn sanitize_detail(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(160)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Provider error codes are short ASCII identifiers; anything else is dropped
/// rather than passed into a UI string.
fn sanitize_code(raw: &str) -> String {
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

    fn configured() -> TikTokProvider {
        TikTokProvider {
            client_key: Some("test-client-key".into()),
            client_secret: Some("test-secret".into()),
            http: reqwest::Client::new(),
        }
    }

    #[test]
    fn every_endpoint_is_https() {
        for ep in [AUTH_ENDPOINT, TOKEN_ENDPOINT, REVOKE_ENDPOINT, USERINFO_ENDPOINT] {
            assert!(ep.starts_with("https://"), "{ep} is not HTTPS");
        }
    }

    #[test]
    fn authorize_url_uses_client_key_not_client_id() {
        let flow = configured().authorize("http://127.0.0.1:9999/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.host_str(), Some("www.tiktok.com"));
        assert_eq!(q["client_key"], "test-client-key");
        assert!(!q.contains_key("client_id"), "TikTok rejects client_id");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], flow.state);
        // The verifier must never reach the browser.
        assert!(!flow.authorize_url.contains(flow.pkce.verifier()));
    }

    /// TikTok's *desktop* product requires a loopback host with a port, and
    /// supports a wildcard port. The ephemeral listener URI is used verbatim.
    #[test]
    fn authorize_uses_the_ephemeral_loopback_redirect() {
        let flow = configured().authorize("http://127.0.0.1:54321/callback").unwrap();
        assert_eq!(flow.redirect_uri, "http://127.0.0.1:54321/callback");

        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let sent = Url::parse(&q["redirect_uri"]).unwrap();

        // The rules TikTok enforces, asserted so a future edit cannot break them.
        assert!(matches!(sent.host_str(), Some("127.0.0.1") | Some("localhost")));
        assert!(sent.port().is_some(), "TikTok requires a port in the redirect URI");
        assert!(sent.query().is_none(), "params may not be appended");
        assert!(sent.fragment().is_none(), "fragments may not be appended");
    }

    /// PKCE is not optional here: omitting `code_challenge` makes TikTok reject
    /// the request with `error_type=code_challenge` before the user sees a
    /// login page at all.
    #[test]
    fn pkce_is_always_present() {
        let flow = configured().authorize("http://127.0.0.1:1234/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert!(!q["code_challenge"].is_empty());
        assert_eq!(q["code_challenge_method"], "S256");
    }

    /// The bug this guards: TikTok wants the challenge hex-encoded, and the
    /// standards-compliant base64url value is refused at the token exchange
    /// with "Code verifier or code challenge is invalid".
    #[test]
    fn the_challenge_sent_to_tiktok_is_hex_not_base64url() {
        let flow = configured().authorize("http://127.0.0.1:1234/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let sent = &q["code_challenge"];

        assert_eq!(sent.len(), 64, "expected 64 hex characters, got {sent:?}");
        assert!(sent.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(*sent, flow.pkce.challenge_hex());
        assert_ne!(*sent, flow.pkce.challenge, "base64url form was sent");
    }

    /// Pins the exact scope set.
    ///
    /// Asserting the whole string rather than "does not contain X" means any
    /// future addition fails here and has to be justified, instead of quietly
    /// widening what users are asked to consent to.
    #[test]
    fn scopes_are_comma_separated_and_pinned() {
        let flow = configured().authorize("http://127.0.0.1:1/callback").unwrap();
        let url = Url::parse(&flow.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(q["scope"], "user.info.basic,video.upload,video.publish");
        assert!(!q["scope"].contains(' '), "TikTok wants comma-delimited scopes");

        // Scopes that read or post beyond what the app actually does.
        for forbidden in ["user.info.stats", "user.info.profile", "research."] {
            assert!(!q["scope"].contains(forbidden), "over-broad scope requested");
        }
    }

    #[test]
    fn unconfigured_without_both_values() {
        let p = TikTokProvider {
            client_key: Some("k".into()),
            client_secret: None,
            http: reqwest::Client::new(),
        };
        assert!(!p.is_configured());
        assert_eq!(
            p.authorize("http://127.0.0.1:1/callback").unwrap_err().code(),
            "provider_not_configured"
        );
    }

    // --- the HTTP-200-with-an-error trap ---------------------------------

    #[test]
    fn ok_envelope_is_not_an_error() {
        let body = br#"{"data":{"user":{"open_id":"x"}},"error":{"code":"ok","message":"","log_id":"1"}}"#;
        assert!(check_api_error(body).is_ok());
    }

    #[test]
    fn structured_error_envelope_is_caught() {
        let body = br#"{"error":{"code":"scope_not_authorized","message":"nope","log_id":"1"}}"#;
        let err = check_api_error(body).unwrap_err();
        assert_eq!(err.code(), "provider_denied");
        // The human-readable message and log id must not be propagated.
        assert!(!err.to_string().contains("nope"));
        assert!(!err.to_string().contains("log_id"));
    }

    #[test]
    fn oauth_style_string_error_is_caught() {
        let body = br#"{"error":"invalid_grant","error_description":"Authorization code is invalid"}"#;
        let err = check_api_error(body).unwrap_err();
        assert_eq!(err.code(), "provider_denied");
        // The description names what TikTok objected to; without it a developer
        // has nothing to act on.
        assert!(
            err.to_string().contains("Authorization code is invalid"),
            "the actionable detail was dropped: {err}"
        );
    }

    #[test]
    fn the_open_api_message_field_is_still_withheld() {
        // `error_description` is developer-facing; `message` on the Open API
        // envelope can carry request detail and stays suppressed.
        let body = br#"{"error":{"code":"scope_not_authorized","message":"request detail here"}}"#;
        let err = check_api_error(body).unwrap_err();
        assert!(!err.to_string().contains("request detail here"));
    }

    #[test]
    fn user_denial_maps_to_cancelled() {
        let body = br#"{"error":"access_denied","error_description":"user denied"}"#;
        assert_eq!(check_api_error(body).unwrap_err().code(), "cancelled");
    }

    #[test]
    fn empty_and_error_free_bodies_pass() {
        assert!(check_api_error(b"").is_ok());
        assert!(check_api_error(br#"{"access_token":"a","expires_in":3600}"#).is_ok());
        // Not JSON at all - the caller's own decode reports the problem.
        assert!(check_api_error(b"<html>502</html>").is_ok());
    }

    fn callback_with(error: &str, error_type: Option<&str>) -> CallbackData {
        CallbackData {
            code: None,
            state: Some("s".into()),
            error: Some(error.into()),
            error_description: None,
            error_type: error_type.map(str::to_string),
        }
    }

    /// The bug this guards: TikTok reports a sandbox misconfiguration as
    /// `access_denied`, which naive handling shows as "you cancelled".
    #[tokio::test]
    async fn sandbox_target_error_is_not_reported_as_a_cancellation() {
        let p = configured();
        let flow = p.authorize("http://127.0.0.1:1/callback").unwrap();

        let err = p
            .handle_callback(&flow, callback_with("access_denied", Some("non_sandbox_target")))
            .await
            .unwrap_err();

        assert_eq!(err.code(), "provider_configuration");
        assert_ne!(err.code(), "cancelled", "user blamed for an app config problem");
        let msg = err.to_string();
        assert!(msg.contains("Sandbox"), "{msg}");
        assert!(msg.contains("Target users"), "{msg}");
        // Guidance must be one readable line, not a wrapped string literal.
        assert!(!msg.contains('\n'));
        assert!(!msg.contains("  "));
    }

    #[tokio::test]
    async fn a_real_user_refusal_is_still_a_cancellation() {
        let p = configured();
        let flow = p.authorize("http://127.0.0.1:1/callback").unwrap();
        let err = p
            .handle_callback(&flow, callback_with("access_denied", None))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "cancelled");
    }

    #[test]
    fn every_explained_error_type_yields_actionable_text() {
        for kind in [
            "non_sandbox_target",
            "code_challenge",
            "redirect_uri",
            "client_key",
            "scope_not_authorized",
        ] {
            let msg = explain_error_type(kind).expect(kind);
            assert!(msg.len() > 40, "{kind}: guidance too thin");
            assert!(!msg.contains('\n'), "{kind}: not a single line");
        }
        assert!(explain_error_type("something_else").is_none());
    }

    #[test]
    fn sanitizer_strips_injected_text() {
        assert_eq!(sanitize_code("invalid_grant"), "invalid_grant");
        assert_eq!(sanitize_code("<b>x</b>"), "bxb");
        assert_eq!(sanitize_code(" "), "unknown_error");
    }
}
