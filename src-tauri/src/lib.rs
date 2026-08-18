//! Media Downloader.
//!
//! Two independent capabilities, deliberately not wired to each other:
//!
//!   * [`auth`] - sign in to a platform and hold the credential in the OS
//!     keychain. Profile scopes only; this grants no access to media.
//!   * [`download`] - fetch *public* Facebook and TikTok videos through
//!     yt-dlp, with no session, no cookie and no token.
//!
//! Keeping them apart is what lets the download feature work without signing
//! in, and what stops it from ever reaching private posts. See the module note
//! on [`download`] for why that boundary is enforced rather than assumed.

pub mod auth;
pub mod commands;
pub mod config;
pub mod db;
pub mod download;
pub mod errors;

use std::sync::Arc;

use tauri::Manager;

use crate::auth::manager::AuthManager;
use crate::auth::storage::OsCredentialStore;
use crate::db::AccountDb;
use crate::download::manager::DownloadManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load `.env` before anything reads configuration. Providers are built in
    // `setup`, so this must happen first.
    config::load_dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let db = Arc::new(AccountDb::open(&data_dir.join("accounts.sqlite3"))?);
            let store = Arc::new(OsCredentialStore::new());

            app.manage(AuthManager::new(store, db));

            // Default to the OS Downloads folder, falling back to the app's
            // own data directory on the systems that don't define one. A
            // folder the user picked later overrides this; the manager reads
            // that preference from `data_dir` itself.
            let default_downloads = crate::download::settings::default_destination(
                app.path().download_dir().ok(),
                &data_dir,
            );
            app.manage(Arc::new(DownloadManager::new(
                default_downloads,
                data_dir.clone(),
            )));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::auth::auth_connect,
            commands::auth::auth_get_accounts,
            commands::auth::auth_get_account,
            commands::auth::auth_disconnect,
            commands::auth::auth_get_providers,
            commands::download::download_engine_status,
            commands::download::download_inspect,
            commands::download::download_start,
            commands::download::download_list,
            commands::download::download_cancel,
            commands::download::download_remove,
            commands::download::download_clear_finished,
            commands::download::download_get_destination,
            commands::download::download_set_destination,
            commands::download::download_reset_destination,
            commands::download::download_browse_destination,
            commands::download::download_reveal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
