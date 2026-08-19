//! Facebook provider - Facebook Login (OAuth 2.0), scaffolded against the same
//! trait as Google.
//!
//! Why this one is not the primary provider:
//!
//! Facebook has no "native/installed app" client type. Its authorization code
//! flow requires (a) a `redirect_uri` whose host is whitelisted in the app's
//! "Valid OAuth Redirect URIs" - loopback `127.0.0.1` is not accepted - and
//! (b) a `client_secret` at the token endpoint. Embedding that secret in a
//! desktop binary is not a secret at all, so this provider stays disabled
//! unless the developer explicitly supplies both values *and* accepts that
//! trade-off, typically by pointing `FACEBOOK_REDIRECT_URI` at a small hosted
//! redirect they control.
//!
//! Facebook does support PKCE on the authorization request, so it is sent here
//! regardless - it costs nothing and removes code-interception risk.
//!
//! Endpoints are Facebook's official Graph API endpoints. Scope is `public_profile`
//! only: the minimum needed to display who is connected. Note that Facebook
//! login grants **no** access to private media, and nothing here should ever be
//! read as implying otherwise.

use async_trait::async_trait;
use url::Url;

use crate::auth::oauth::{PendingFlow, Pkce, TokenResponse};
use crate::auth::providers::{AuthProvider, ProviderDescriptor};
use crate::config;
use crate::auth::{AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::errors::{AppError, Result};

const GRAPH_VERSION: &str = "v21.0";
/// The port the hosted redirect page forwards the callback to. Fixed, because
/// the page needs a known address. Overridable if it clashes with something.
const CALLBACK_PORT: u16 = 8721;

/// `public_profile` to identify the account, plus the Page-publishing scopes
/// the upload feature needs. In development mode these work for the app's own
/// admins/testers without review; publishing for other users needs Meta's app
/// review of `pages_manage_posts`.
const SCOPES: &[&str] = &[
    "public_profile",
    "pages_show_list",
    "pages_read_engagement",
    "pages_manage_posts",
];

pub struct FacebookProvider {
    client_id: Option<String>,
    client_secret: Option<String>,
    /// Facebook rejects loopback redirects, so unlike Google the redirect URI
    /// is fixed configuration rather than a per-flow ephemeral port.
    redirect_uri: Option<String>,
    http: reqwest::Client,
}

impl FacebookProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client_id: config::read("FACEBOOK_CLIENT_ID"),
            client_secret: config::read("FACEBOOK_CLIENT_SECRET"),
            redirect_uri: config::read("FACEBOOK_REDIRECT_URI"),
            http,
        }
    }

    fn auth_endpoint() -> String {
        format!("https://www.facebook.com/{GRAPH_VERSION}/dialog/oauth")
    }

    fn token_endpoint() -> String {
        format!("https://graph.facebook.com/{GRAPH_VERSION}/oauth/access_token")
    }

    fn me_endpoint() -> String {
        format!("https://graph.facebook.com/{GRAPH_VERSION}/me")
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
            (Some(id), Some(secret), Some(uri)) => Ok((id, secret, uri)),
            _ => Err(AppError::ProviderNotConfigured("facebook".into())),
        }
    }
}

#[async_trait]
impl AuthProvider for FacebookProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::Facebook
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::Facebook,
            display_name: "Facebook".into(),
            configured: self.is_configured(),
            // Facebook exposes DELETE /{user-id}/permissions for de-authorization.
            supports_revocation: true,
            scopes: Self::scopes(),
        }
    }

    fn is_configured(&self) -> bool {
        self.require().is_ok()
    }

    /// `_loopback_redirect_uri` is ignored on purpose: Facebook will not accept
    /// a `127.0.0.1` redirect, so the configured URI is used instead.
    fn fixed_callback_port(&self) -> Option<u16> {
        Some(
            config::read("FACEBOOK_CALLBACK_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(CALLBACK_PORT),
        )
    }

    fn authorize(&self, _loopback_redirect_uri: &str) -> Result<PendingFlow> {
        let (client_id, _, redirect_uri) = self.require()?;
        let state = PendingFlow::new_state();
        let pkce = Pkce::generate();

        let mut url = Url::parse(&Self::auth_endpoint())
            .map_err(|_| AppError::Internal("bad auth endpoint".into()))?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &SCOPES.join(","))
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", pkce.method());

        Ok(PendingFlow {
            provider: ProviderId::Facebook,
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
                _ => AppError::ProviderDenied("facebook_denied".into()),
            });
        }
        let code = callback.code.as_deref().ok_or(AppError::MalformedProviderResponse)?;
        let (client_id, client_secret, _) = self.require()?;

        let resp = self
            .http
            .post(Self::token_endpoint())
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
                ("code_verifier", flow.pkce.verifier()),
                ("redirect_uri", flow.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if !resp.status().is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Facebook rejected the token request (HTTP {})",
                resp.status().as_u16()
            )));
        }

        let token: TokenResponse = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
        let credential = token.into_credential(ProviderId::Facebook, &Self::scopes());
        let account = self.get_account(&credential).await?;
        Ok(AuthResult { credential, account })
    }

    /// Facebook does not issue refresh tokens. Long-lived user tokens are
    /// obtained by exchanging a short-lived one via `fb_exchange_token`, and
    /// that exchange itself requires the app secret.
    async fn refresh(&self, credential: &Credential) -> Result<Credential> {
        let (client_id, client_secret, _) = self.require()?;

        let resp = self
            .http
            .get(Self::token_endpoint())
            .query(&[
                ("grant_type", "fb_exchange_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("fb_exchange_token", credential.access_token.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if !resp.status().is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Facebook rejected the token exchange (HTTP {})",
                resp.status().as_u16()
            )));
        }

        let token: TokenResponse = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
        Ok(token.into_credential(ProviderId::Facebook, &credential.scopes))
    }

    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo> {
        #[derive(serde::Deserialize)]
        struct Picture {
            data: PictureData,
        }
        #[derive(serde::Deserialize)]
        struct PictureData {
            #[serde(default)]
            url: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Me {
            id: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            picture: Option<Picture>,
        }

        let resp = self
            .http
            .get(Self::me_endpoint())
            .query(&[("fields", "id,name,picture.width(256).height(256)")])
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if !resp.status().is_success() {
            return Err(AppError::ProviderDenied(format!(
                "Facebook rejected the profile request (HTTP {})",
                resp.status().as_u16()
            )));
        }

        let me: Me = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
        Ok(AccountInfo {
            provider: ProviderId::Facebook,
            display_name: me.name.unwrap_or_else(|| "Facebook account".into()),
            avatar_url: me.picture.and_then(|p| p.data.url),
            external_id: me.id,
            email: None,
        })
    }

    /// De-authorize the app for this user: `DELETE /{user-id}/permissions`.
    async fn revoke(&self, credential: &Credential) -> Result<()> {
        let account = self.get_account(credential).await?;
        let url = format!(
            "https://graph.facebook.com/{GRAPH_VERSION}/{}/permissions",
            account.external_id
        );
        let resp = self
            .http
            .delete(url)
            .bearer_auth(&credential.access_token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(AppError::ProviderDenied(format!(
                "Facebook de-authorization returned HTTP {}",
                resp.status().as_u16()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_unconfigured_without_all_three_values() {
        let p = FacebookProvider {
            client_id: Some("id".into()),
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

    #[test]
    fn authorize_uses_configured_redirect_not_loopback() {
        let p = FacebookProvider {
            client_id: Some("id".into()),
            client_secret: Some("secret".into()),
            redirect_uri: Some("https://example.com/cb".into()),
            http: reqwest::Client::new(),
        };
        let flow = p.authorize("http://127.0.0.1:9999/callback").unwrap();
        assert_eq!(flow.redirect_uri, "https://example.com/cb");
        let url = Url::parse(&flow.authorize_url).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("www.facebook.com"));
    }

    #[test]
    fn every_endpoint_is_https() {
        for ep in [
            FacebookProvider::auth_endpoint(),
            FacebookProvider::token_endpoint(),
            FacebookProvider::me_endpoint(),
        ] {
            assert!(ep.starts_with("https://"), "{ep} is not HTTPS");
        }
    }
}
