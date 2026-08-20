//! Commands for the multi-account YouTube uploader.
//!
//! Adding an account runs a full Google OAuth flow (reusing the auth manager's
//! browser dance) and stores the credential in the dedicated uploader store,
//! separate from the single-account slot on the Accounts page. Uploading pulls
//! that account's token, refreshing it first if needed.

use tauri::{AppHandle, Manager, State};

use crate::auth::manager::AuthManager;
use crate::auth::{now_unix, ProviderId};
use crate::errors::{AppError, Result};
use crate::youtube::{self, Privacy};
use crate::youtube_accounts::{self, StoredAccount, YoutubeAccountView};

fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("no app data dir: {e}")))
}

/// Every connected uploader account (no tokens).
#[tauri::command]
pub async fn youtube_accounts_list(app: AppHandle) -> Result<Vec<YoutubeAccountView>> {
    let dir = data_dir(&app)?;
    Ok(youtube_accounts::list(&dir)
        .iter()
        .map(YoutubeAccountView::from)
        .collect())
}

/// Sign in to another Google account and store it as an uploader. Google's
/// account chooser is shown, so the user can pick a different account each time.
#[tauri::command]
pub async fn youtube_account_add(
    app: AppHandle,
    manager: State<'_, AuthManager>,
) -> Result<YoutubeAccountView> {
    let auth = manager.connect_additional(&app, ProviderId::Google).await?;

    // Resolve the channel this login uploads to, for a friendlier card. A
    // missing channel is not fatal — the account can still be selected, and the
    // upload itself will surface any "no channel" problem.
    let http = reqwest::Client::new();
    let (channel_title, channel_avatar) =
        match youtube::my_channels(&http, &auth.credential.access_token).await {
            Ok(list) => match list.into_iter().next() {
                Some(c) => (Some(c.title), c.thumbnail),
                None => (None, None),
            },
            Err(_) => (None, None),
        };

    let account = StoredAccount {
        external_id: auth.account.external_id.clone(),
        display_name: auth.account.display_name.clone(),
        avatar_url: auth.account.avatar_url.clone(),
        email: auth.account.email.clone(),
        channel_title,
        channel_avatar,
        added_at: now_unix(),
        credential: auth.credential,
    };
    youtube_accounts::save(&data_dir(&app)?, &account)?;
    Ok(YoutubeAccountView::from(&account))
}

/// Forget an uploader account. Local only — Google keeps the grant until the
/// user removes it from their Google account settings.
#[tauri::command]
pub async fn youtube_account_remove(app: AppHandle, account_id: String) -> Result<()> {
    youtube_accounts::remove(&data_dir(&app)?, &account_id)
}

/// Upload a video to one specific uploader account, refreshing its token first
/// if it has expired and writing the refreshed credential back.
#[tauri::command]
pub async fn youtube_account_upload(
    app: AppHandle,
    manager: State<'_, AuthManager>,
    account_id: String,
    file_path: String,
    title: String,
    description: String,
    privacy: String,
) -> Result<String> {
    let dir = data_dir(&app)?;
    let mut account = youtube_accounts::load(&dir, &account_id)
        .ok_or_else(|| AppError::Internal("that YouTube account is no longer connected".into()))?;

    let fresh = manager
        .ensure_fresh(ProviderId::Google, account.credential.clone())
        .await?;
    // Persist a refreshed token so the next upload doesn't refresh again.
    if fresh.access_token != account.credential.access_token {
        account.credential = fresh.clone();
        let _ = youtube_accounts::save(&dir, &account);
    }

    youtube::upload_video(
        &reqwest::Client::new(),
        &fresh.access_token,
        std::path::Path::new(&file_path),
        &title,
        &description,
        Privacy::parse(&privacy),
    )
    .await
}
