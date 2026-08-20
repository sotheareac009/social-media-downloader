//! X (Twitter) provider — OAuth 2.0 Authorization Code with PKCE.
//!
//! Endpoints (X API v2):
//!   authorize  https://twitter.com/i/oauth2/authorize
//!   token      https://api.twitter.com/2/oauth2/token
//!   revoke     https://api.twitter.com/2/oauth2/revoke
//!   user info  https://api.twitter.com/2/users/me
//!
//! Two things differ from a textbook flow:
//!
//!   1. **Exact redirect match, no wildcard port.** Unlike TikTok, X will not
//!      accept `http://127.0.0.1:*/callback`; the port must be registered. So a
//!      fixed loopback port is used and the developer registers exactly one URL
//!      (localhost/127.0.0.1 are the only hosts X allows over plain http).
//!   2. **Confidential clients authenticate the token call with HTTP Basic.**
//!      When a client secret is configured we send `Authorization: Basic
//!      base64(id:secret)`; a public (native) client with no secret sends
//!      `client_id` in the body instead.
//!
//! `offline.access` is requested so a refresh token comes back — without it the
//! access token expires in two hours and the account silently goes stale.

use async_trait::async_trait;
use url::Url;

use crate::auth::oauth::{PendingFlow, Pkce, TokenResponse};
use crate::auth::providers::{AuthProvider, ProviderDescriptor};
use crate::auth::{AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::config;
use crate::errors::{AppError, Result};

const AUTH_ENDPOINT: &str = "https://twitter.com/i/oauth2/authorize";
const TOKEN_ENDPOINT: &str = "https://api.twitter.com/2/oauth2/token";
const REVOKE_ENDPOINT: &str = "https://api.twitter.com/2/oauth2/revoke";
const USERINFO_ENDPOINT: &str = "https://api.twitter.com/2/users/me";

/// `tweet.read` + `users.read` identify the account; `offline.access` yields a
/// refresh token so the connection survives past the 2-hour access-token life.
const SCOPES: &[&str] = &[
    "tweet.read",
    "tweet.write",
    "users.read",
    "media.write",
    "offline.access",
];

/// Default loopback port X's callback must arrive on. Overridable so a taken
/// port can be moved without a rebuild; whatever value is used must match the
/// callback URL registered in the X app settings.
const CALLBACK_PORT: u16 = 8723;

pub struct XProvider {
    client_id: Option<String>,
    /// Optional: present for confidential clients, absent for native/public.
    client_secret: Option<String>,
    http: reqwest::Client,
}

impl XProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client_id: config::read("X_CLIENT_ID"),
            client_secret: config::read("X_CLIENT_SECRET"),
            http,
        }
    }

    fn scopes() -> Vec<String> {
        SCOPES.iter().map(|s| s.to_string()).collect()
    }

    fn require(&self) -> Result<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| AppError::ProviderNotConfigured("x".into()))
    }

    /// POST the token endpoint. Confidential clients use HTTP Basic auth; public
    /// clients pass `client_id` in the body. Extra body params are appended by
    /// the caller (the grant-specific fields).
    async fn post_token_endpoint(&self, extra: &[(&str, &str)]) -> Result<TokenResponse> {
        let client_id = self.require()?;
        let mut form: Vec<(&str, &str)> = extra.to_vec();

        let mut req = self.http.post(TOKEN_ENDPOINT);
        if let Some(secret) = self.client_secret.as_deref() {
            req = req.basic_auth(client_id, Some(secret));
        } else {
            form.push(("client_id", client_id));
        }

        let resp = req.form(&form).send().await.map_err(|_| AppError::Network)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;

        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "X rejected the token request (HTTP {})",
                status.as_u16()
            )));
        }

        serde_json::from_slice::<TokenResponse>(&body)
            .map_err(|_| AppError::MalformedProviderResponse)
    }
}

#[async_trait]
impl AuthProvider for XProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::X
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::X,
            display_name: "X".into(),
            configured: self.is_configured(),
            supports_revocation: true,
            scopes: Self::scopes(),
        }
    }

    fn is_configured(&self) -> bool {
        self.require().is_ok()
    }

    /// X requires an exact redirect match, so the callback binds a fixed port.
    fn fixed_callback_port(&self) -> Option<u16> {
        Some(
            config::read("X_CALLBACK_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(CALLBACK_PORT),
        )
    }

    fn authorize(&self, redirect_uri: &str) -> Result<PendingFlow> {
        let client_id = self.require()?;
        let state = PendingFlow::new_state();
        let pkce = Pkce::generate();

        let mut url =
            Url::parse(AUTH_ENDPOINT).map_err(|_| AppError::Internal("bad auth endpoint".into()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("state", &state)
            // Standard RFC 7636 base64url challenge — X rejects the hex form.
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", pkce.method());

        Ok(PendingFlow {
            provider: ProviderId::X,
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
                other => AppError::ProviderDenied(sanitize(other)),
            });
        }

        let code = callback.code.as_deref().ok_or(AppError::MalformedProviderResponse)?;

        let token = self
            .post_token_endpoint(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", flow.redirect_uri.as_str()),
                ("code_verifier", flow.pkce.verifier()),
            ])
            .await?;

        let credential = token.into_credential(ProviderId::X, &Self::scopes());
        let account = self.get_account(&credential).await?;
        Ok(AuthResult { credential, account })
    }

    async fn refresh(&self, credential: &Credential) -> Result<Credential> {
        let refresh_token = credential
            .refresh_token
            .as_deref()
            .ok_or_else(|| AppError::CredentialNotFound("x".into()))?;

        let token = self
            .post_token_endpoint(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await?;

        let mut fresh = token.into_credential(ProviderId::X, &credential.scopes);
        // X may omit a new refresh token; keep the old one so the account stays
        // refreshable.
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = credential.refresh_token.clone();
        }
        Ok(fresh)
    }

    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            data: User,
        }
        #[derive(serde::Deserialize)]
        struct User {
            id: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            username: Option<String>,
            #[serde(default)]
            profile_image_url: Option<String>,
        }

        let resp = self
            .http
            .get(USERINFO_ENDPOINT)
            .query(&[("user.fields", "profile_image_url,username,name")])
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        let status = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;
        if !status.is_success() {
            return Err(AppError::ProviderDenied(format!(
                "X rejected the profile request (HTTP {})",
                status.as_u16()
            )));
        }

        let env: Envelope =
            serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

        // Prefer the @handle for display; fall back to the name, then a stub.
        let display_name = env
            .data
            .username
            .as_ref()
            .map(|u| format!("@{u}"))
            .or(env.data.name)
            .unwrap_or_else(|| "X account".into());

        Ok(AccountInfo {
            provider: ProviderId::X,
            display_name,
            avatar_url: env.data.profile_image_url,
            external_id: env.data.id,
            email: None,
        })
    }

    async fn revoke(&self, credential: &Credential) -> Result<()> {
        // Best-effort. A failed revoke must not block local disconnect, so the
        // manager ignores the error either way; we still try.
        let _ = self
            .post_revoke(&credential.access_token)
            .await;
        Ok(())
    }
}

impl XProvider {
    async fn post_revoke(&self, token: &str) -> Result<()> {
        let client_id = self.require()?;
        let mut form: Vec<(&str, &str)> =
            vec![("token", token), ("token_type_hint", "access_token")];
        let mut req = self.http.post(REVOKE_ENDPOINT);
        if let Some(secret) = self.client_secret.as_deref() {
            req = req.basic_auth(client_id, Some(secret));
        } else {
            form.push(("client_id", client_id));
        }
        req.form(&form).send().await.map_err(|_| AppError::Network)?;
        Ok(())
    }
}

/// X error codes are short slugs; strip anything that isn't a safe identifier
/// so nothing from the query string leaks into a message.
fn sanitize(code: &str) -> String {
    let cleaned: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "x_denied".into()
    } else {
        cleaned
    }
}
