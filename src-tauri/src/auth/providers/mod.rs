//! Provider abstraction.
//!
//! Everything provider-specific - authorization URL, token endpoint, scopes,
//! userinfo shape, revocation endpoint - lives inside an implementation of
//! [`AuthProvider`]. The `AuthManager` holds only `dyn AuthProvider` and must
//! never grow a `match` on a specific platform.

pub mod facebook;
pub mod google;
pub mod instagram;
pub mod tiktok;
pub mod x;

use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::oauth::PendingFlow;
use crate::auth::{AccountInfo, AuthResult, CallbackData, Credential, ProviderId};
use crate::errors::Result;

/// Display metadata for a provider, safe to send to React.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub display_name: String,
    /// False when the build has no client id for this provider; the UI shows
    /// "Requires configuration" instead of an enabled Connect button.
    pub configured: bool,
    /// Whether the provider offers a token revocation endpoint we can call on
    /// disconnect. When false, disconnect only clears local storage.
    pub supports_revocation: bool,
    pub scopes: Vec<String>,
}

#[async_trait]
pub trait AuthProvider: Send + Sync {
    fn provider_id(&self) -> ProviderId;

    fn descriptor(&self) -> ProviderDescriptor;

    /// True when this build carries the configuration needed to authorize.
    fn is_configured(&self) -> bool;

    /// Build the authorization request: the URL to open in the system browser
    /// plus the flow secrets (state, PKCE verifier) the manager must hold onto
    /// until the callback arrives.
    ///
    /// `redirect_uri` is supplied by the callback layer because the loopback
    /// port is only known once the listener has bound.
    fn authorize(&self, redirect_uri: &str) -> Result<PendingFlow>;

    /// A fixed loopback port this provider's callback must arrive on, or `None`
    /// for the usual ephemeral port. Only providers with a hosted redirect
    /// page that forwards to loopback (Facebook) need a fixed port, so the
    /// page has a known address to forward to.
    fn fixed_callback_port(&self) -> Option<u16> {
        None
    }

    /// Validate the callback and exchange it for a credential + account info.
    ///
    /// Implementations must assume `callback` is attacker-controlled. State
    /// validation happens in the manager before this is called, but providers
    /// must still reject a missing code and an `error` parameter.
    async fn handle_callback(
        &self,
        flow: &PendingFlow,
        callback: CallbackData,
    ) -> Result<AuthResult>;

    /// Whether `credential` can currently be refreshed without sending the
    /// user back through the browser.
    ///
    /// The default is the standard OAuth answer - a refresh token must exist.
    /// Providers that renew differently override this: Instagram, for example,
    /// has no refresh token at all and refreshes the access token using
    /// itself, so a credential with `refresh_token: None` is still renewable.
    fn can_refresh(&self, credential: &Credential) -> bool {
        credential.refresh_token.is_some()
    }

    /// Obtain a fresh access token. Only called when [`Self::can_refresh`]
    /// returned true for this credential.
    async fn refresh(&self, credential: &Credential) -> Result<Credential>;

    /// Fetch current account metadata for a live credential.
    async fn get_account(&self, credential: &Credential) -> Result<AccountInfo>;

    /// Best-effort server-side revocation. Default: nothing to revoke.
    ///
    /// Returning `Ok(())` here does not mean the token was revoked, only that
    /// the provider offers no supported way to do so; the manager still deletes
    /// the local credential.
    async fn revoke(&self, _credential: &Credential) -> Result<()> {
        Ok(())
    }
}

/// Build every provider this application supports.
///
/// This is the single place a new platform is registered. `AuthManager` only
/// ever sees `Arc<dyn AuthProvider>`, so adding a provider means writing one
/// module here and adding one line below - the manager itself does not change.
pub fn build_registry(http: reqwest::Client) -> Vec<Arc<dyn AuthProvider>> {
    vec![
        Arc::new(google::GoogleProvider::new(http.clone())),
        Arc::new(facebook::FacebookProvider::new(http.clone())),
        Arc::new(instagram::InstagramProvider::new(http.clone())),
        Arc::new(tiktok::TikTokProvider::new(http.clone())),
        Arc::new(x::XProvider::new(http)),
    ]
}
