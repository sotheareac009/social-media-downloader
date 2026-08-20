//! `AuthManager` - the provider-agnostic orchestrator.
//!
//! Deliberately contains no OAuth URL, no scope list and no `match` on a
//! specific platform. Adding a provider means registering one more
//! `Arc<dyn AuthProvider>` in [`AuthManager::new`] and changing nothing here.
//!
//! Responsibilities:
//!   * hold the provider registry, the credential store and the metadata db
//!   * run exactly one authorization flow at a time
//!   * validate `state` before any provider code touches the callback
//!   * keep secrets on the Rust side of the IPC boundary

use std::collections::HashMap;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use url::Url;

use crate::auth::callback::CallbackListener;
use crate::auth::providers::{build_registry, AuthProvider, ProviderDescriptor};
use crate::auth::storage::CredentialStore;
use crate::auth::{Credential, ProviderId, EXPIRY_SKEW_SECS};
use crate::db::{AccountDb, AccountView, CredentialMeta};
use crate::errors::{AppError, Result};

/// How long to spend confirming the provider is reachable before sending the
/// user to their browser. Short: this is a "is there a network at all" probe,
/// not a health check.
const PREFLIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub mod events {
    pub const STARTED: &str = "auth://started";
    pub const SUCCESS: &str = "auth://success";
    pub const FAILED: &str = "auth://failed";
    pub const DISCONNECTED: &str = "auth://disconnected";
}

#[derive(Clone, serde::Serialize)]
pub struct AuthStartedEvent {
    pub provider: ProviderId,
}

#[derive(Clone, serde::Serialize)]
pub struct AuthFailedEvent {
    pub provider: ProviderId,
    pub code: String,
    pub message: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AuthDisconnectedEvent {
    pub provider: ProviderId,
    /// False when the provider's revocation call did not succeed. The local
    /// credential is deleted either way; this tells the UI whether to suggest
    /// removing the app from the provider's account settings manually.
    pub revoked_remotely: bool,
}

pub struct AuthManager {
    providers: HashMap<ProviderId, Arc<dyn AuthProvider>>,
    store: Arc<dyn CredentialStore>,
    db: Arc<AccountDb>,
    /// Held for the duration of a flow. `try_lock` failing is what makes a
    /// second concurrent Connect return `FlowAlreadyRunning` instead of
    /// silently racing the first.
    flow_guard: tokio::sync::Mutex<()>,
}

impl AuthManager {
    pub fn new(store: Arc<dyn CredentialStore>, db: Arc<AccountDb>) -> Self {
        let http = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("MediaDownloader/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();

        Self {
            providers: build_registry(http)
                .into_iter()
                .map(|p| (p.provider_id(), p))
                .collect(),
            store,
            db,
            flow_guard: tokio::sync::Mutex::new(()),
        }
    }

    fn provider(&self, id: ProviderId) -> Result<&Arc<dyn AuthProvider>> {
        self.providers
            .get(&id)
            .ok_or_else(|| AppError::UnknownProvider(id.to_string()))
    }

    pub fn descriptors(&self) -> Vec<ProviderDescriptor> {
        // Stable order so the UI does not reshuffle between calls.
        ProviderId::ALL
            .iter()
            .filter_map(|id| self.providers.get(id))
            .map(|p| p.descriptor())
            .collect()
    }

    /// Run a full authorization flow. Returns only non-sensitive account data.
    pub async fn connect(&self, app: &AppHandle, id: ProviderId) -> Result<AccountView> {
        let _guard = self
            .flow_guard
            .try_lock()
            .map_err(|_| AppError::FlowAlreadyRunning)?;

        let result = self.run_flow(app, id).await;

        match &result {
            Ok(view) => {
                let _ = app.emit(events::SUCCESS, view.clone());
            }
            Err(e) => {
                let _ = app.emit(
                    events::FAILED,
                    AuthFailedEvent {
                        provider: id,
                        code: e.code().to_string(),
                        message: e.to_string(),
                    },
                );
            }
        }
        result
    }

    async fn run_flow(&self, app: &AppHandle, id: ProviderId) -> Result<AccountView> {
        let provider = self.provider(id)?;
        if !provider.is_configured() {
            return Err(AppError::ProviderNotConfigured(id.to_string()));
        }

        // Bind the listener *before* building the URL: the redirect URI has to
        // carry the port. A provider with a hosted redirect page (Facebook)
        // needs a fixed port the page can forward to; everyone else gets an
        // ephemeral one.
        let listener = match provider.fixed_callback_port() {
            Some(port) => CallbackListener::bind_fixed(port).await?,
            None => CallbackListener::bind().await?,
        };
        let flow = provider.authorize(listener.redirect_uri())?;

        // Fail fast when there is no network.
        //
        // Opening the browser always "succeeds" - the opener only launches the
        // application, it cannot know the page will not load. Without this
        // check an offline user watches "Waiting for browser" for the full
        // five-minute flow timeout and is then told the sign-in timed out,
        // which points at the wrong problem entirely.
        ensure_reachable(&flow.authorize_url).await?;

        let _ = app.emit(events::STARTED, AuthStartedEvent { provider: id });

        // The user authenticates with the platform directly, in their own
        // browser. This application never renders a login form and never sees
        // a password.
        open_in_system_browser(app, &flow.authorize_url)?;

        let callback = listener.wait_for_callback().await?;

        // State is validated here, before any provider-specific code runs, so
        // no provider implementation can forget to do it.
        if !flow.state_matches(callback.state.as_deref()) {
            return Err(AppError::StateMismatch);
        }

        let auth = provider.handle_callback(&flow, callback).await?;

        // Secret half -> OS keychain. Public half (plus the non-secret facts
        // needed to render the card) -> SQLite.
        let meta = CredentialMeta::new(&auth.credential, provider.can_refresh(&auth.credential));
        self.store.save(id, &auth.credential)?;
        let view = self.db.upsert(&auth.account, meta)?;
        Ok(view)
    }

    /// Snapshot of every provider, connected or not, for the Accounts page.
    pub fn list_accounts(&self) -> Result<Vec<AccountView>> {
        let mut out = Vec::new();
        for descriptor in self.descriptors() {
            out.push(self.account_view(descriptor.id)?);
        }
        Ok(out)
    }

    /// Read a provider's card state.
    ///
    /// Deliberately does **not** open the OS keychain. Rendering the Accounts
    /// page used to cost up to two keychain reads per provider, which on macOS
    /// means a "wants to use your confidential information" prompt per read on
    /// any build whose code signature has changed. Everything the card needs -
    /// name, avatar, expiry, refreshability - is non-secret and cached in
    /// SQLite at connect time, so a page load now costs zero keychain access.
    ///
    /// The keychain remains the source of truth for the credential itself: it
    /// is opened only to connect, to disconnect, or to actually use a token,
    /// and [`Self::access_token`] reconciles the row if the secret has gone.
    pub fn account_view(&self, id: ProviderId) -> Result<AccountView> {
        Ok(self.db.get(id)?.unwrap_or_else(|| AccountView::disconnected(id)))
    }

    /// Return a usable access token for internal callers, refreshing it first
    /// if it is expired. Never exposed through a Tauri command.
    pub async fn access_token(&self, id: ProviderId) -> Result<Credential> {
        let provider = self.provider(id)?;

        let Some(credential) = self.store.get(id)? else {
            // The secret is gone but a row survives - the user cleared their
            // keychain, or restored a machine. Drop the stale row so the UI
            // stops claiming a connection that cannot be used.
            self.db.delete(id)?;
            return Err(AppError::CredentialNotFound(id.to_string()));
        };

        if !credential.is_expired(EXPIRY_SKEW_SECS) {
            self.db.touch(id)?;
            return Ok(credential);
        }
        if !provider.can_refresh(&credential) {
            return Err(AppError::CredentialNotFound(id.to_string()));
        }

        let refreshed = provider.refresh(&credential).await?;
        let meta = CredentialMeta::new(&refreshed, provider.can_refresh(&refreshed));
        self.store.save(id, &refreshed)?;
        self.db.update_meta(id, meta)?;
        Ok(refreshed)
    }

    /// Disconnect: revoke upstream where the provider supports it, then delete
    /// the credential and the metadata row.
    ///
    /// Local deletion happens even if revocation fails - otherwise a network
    /// error would leave the user unable to disconnect.
    pub async fn disconnect(&self, app: &AppHandle, id: ProviderId) -> Result<AccountView> {
        let provider = self.provider(id)?;
        let mut revoked_remotely = false;

        if let Some(credential) = self.store.get(id)? {
            revoked_remotely = provider.revoke(&credential).await.is_ok();
        }

        self.store.delete(id)?;
        self.db.delete(id)?;

        let _ = app.emit(
            events::DISCONNECTED,
            AuthDisconnectedEvent {
                provider: id,
                revoked_remotely,
            },
        );
        Ok(AccountView::disconnected(id))
    }
}

/// Probe that the authorization host accepts a TCP connection.
///
/// Deliberately not an HTTP request: no data is sent, nothing is logged, and a
/// bare connect distinguishes "no DNS / no route" from everything else without
/// touching the provider's API. A captive portal will pass this and then show
/// its own page in the browser, which is the right outcome.
async fn ensure_reachable(authorize_url: &str) -> Result<()> {
    let url = Url::parse(authorize_url).map_err(|_| AppError::Internal("bad authorize url".into()))?;
    let host = url.host_str().ok_or_else(|| AppError::Internal("no host".into()))?;
    let port = url.port_or_known_default().unwrap_or(443);

    match tokio::time::timeout(
        PREFLIGHT_TIMEOUT,
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    {
        Ok(Ok(_stream)) => Ok(()),
        // Refused, unresolvable, or no route - all mean "cannot start a login".
        Ok(Err(_)) => Err(AppError::Network),
        Err(_elapsed) => Err(AppError::Network),
    }
}

fn open_in_system_browser(app: &AppHandle, url: &str) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| AppError::BrowserLaunch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::storage::CredentialStore;
    use crate::auth::AccountInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A store that refuses to be read and counts the attempts.
    ///
    /// On macOS every real read can raise a "wants to use your confidential
    /// information" prompt, so any read on a UI path is a defect.
    #[derive(Default)]
    struct CountingStore {
        reads: AtomicUsize,
    }

    impl CredentialStore for CountingStore {
        fn save(&self, _p: ProviderId, _c: &Credential) -> Result<()> {
            Ok(())
        }
        fn get(&self, _p: ProviderId) -> Result<Option<Credential>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        fn delete(&self, _p: ProviderId) -> Result<()> {
            Ok(())
        }
    }

    fn manager_with(store: Arc<CountingStore>) -> (AuthManager, Arc<AccountDb>) {
        let db = Arc::new(AccountDb::open_in_memory().unwrap());
        (AuthManager::new(store, db.clone()), db)
    }

    #[test]
    fn rendering_the_accounts_page_never_opens_the_keychain() {
        let store = Arc::new(CountingStore::default());
        let (manager, db) = manager_with(store.clone());

        // Both with no accounts...
        manager.list_accounts().unwrap();
        for id in ProviderId::ALL {
            manager.account_view(*id).unwrap();
        }

        // ...and with one connected.
        db.upsert(
            &AccountInfo {
                provider: ProviderId::Google,
                external_id: "ext-1".into(),
                display_name: "Jane Doe".into(),
                avatar_url: None,
                email: None,
            },
            CredentialMeta {
                expires_at: Some(crate::auth::now_unix() + 3600),
                refreshable: true,
            },
        )
        .unwrap();

        manager.list_accounts().unwrap();
        manager.account_view(ProviderId::Google).unwrap();

        assert_eq!(
            store.reads.load(Ordering::SeqCst),
            0,
            "a UI read path opened the keychain; each read can cost the user an \
             OS authorization prompt"
        );
    }

    /// Hermetic: bind a real listener and connect to it, so the "reachable"
    /// case is proven without depending on the machine having internet.
    #[tokio::test]
    async fn reachable_host_passes_preflight() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(ensure_reachable(&format!("http://127.0.0.1:{port}/authorize"))
            .await
            .is_ok());
    }

    /// The offline case. Port 1 on loopback is closed, so connect is refused
    /// immediately - the same class of failure as having no network.
    #[tokio::test]
    async fn unreachable_host_fails_as_a_network_error_not_a_timeout() {
        let err = ensure_reachable("http://127.0.0.1:1/authorize")
            .await
            .unwrap_err();
        assert_eq!(
            err.code(),
            "network",
            "an offline user must be told to check their connection, not that \
             their sign-in timed out"
        );
    }

    /// It must fail fast rather than make the user wait out the flow timeout.
    #[tokio::test]
    async fn preflight_returns_promptly() {
        let started = std::time::Instant::now();
        let _ = ensure_reachable("http://127.0.0.1:1/authorize").await;
        assert!(
            started.elapsed() < PREFLIGHT_TIMEOUT,
            "preflight should refuse immediately on a closed port"
        );
        assert!(PREFLIGHT_TIMEOUT < crate::auth::callback::FLOW_TIMEOUT);
    }

    #[test]
    fn list_accounts_covers_every_provider_in_a_stable_order() {
        let store = Arc::new(CountingStore::default());
        let (manager, _db) = manager_with(store);

        let first: Vec<_> = manager
            .list_accounts()
            .unwrap()
            .iter()
            .map(|a| a.provider)
            .collect();
        let second: Vec<_> = manager
            .list_accounts()
            .unwrap()
            .iter()
            .map(|a| a.provider)
            .collect();

        assert_eq!(first, second, "card order must not shuffle between loads");
        assert_eq!(first.len(), ProviderId::ALL.len());
    }
}
