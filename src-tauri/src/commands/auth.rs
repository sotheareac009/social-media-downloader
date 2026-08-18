//! Tauri commands exposed to React.
//!
//! Every return type here is non-sensitive by construction: `AccountView` and
//! `ProviderDescriptor` have no field that can hold a token. There is
//! deliberately no command that returns a `Credential` - if a future feature
//! needs an access token it should call `AuthManager::access_token` from Rust
//! and make the authenticated request in Rust too.

use tauri::{AppHandle, State};

use crate::auth::manager::AuthManager;
use crate::auth::providers::ProviderDescriptor;
use crate::auth::ProviderId;
use crate::db::AccountView;
use crate::errors::{AppError, Result};

fn parse_provider(raw: &str) -> Result<ProviderId> {
    ProviderId::parse(raw).ok_or_else(|| AppError::UnknownProvider(raw.to_string()))
}

/// Start an authorization flow. Resolves when the user has finished in their
/// browser, or rejects with a structured error.
#[tauri::command]
pub async fn auth_connect(
    app: AppHandle,
    manager: State<'_, AuthManager>,
    provider: String,
) -> Result<AccountView> {
    manager.connect(&app, parse_provider(&provider)?).await
}

/// Every provider and its current connection state - what the Accounts page
/// renders on load.
#[tauri::command]
pub async fn auth_get_accounts(manager: State<'_, AuthManager>) -> Result<Vec<AccountView>> {
    manager.list_accounts()
}

#[tauri::command]
pub async fn auth_get_account(
    manager: State<'_, AuthManager>,
    provider: String,
) -> Result<AccountView> {
    manager.account_view(parse_provider(&provider)?)
}

#[tauri::command]
pub async fn auth_disconnect(
    app: AppHandle,
    manager: State<'_, AuthManager>,
    provider: String,
) -> Result<AccountView> {
    manager.disconnect(&app, parse_provider(&provider)?).await
}

/// Static provider metadata: display name, whether this build is configured for
/// it, and the scopes it will request.
#[tauri::command]
pub async fn auth_get_providers(manager: State<'_, AuthManager>) -> Result<Vec<ProviderDescriptor>> {
    Ok(manager.descriptors())
}
