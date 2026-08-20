//! Media Downloader.
//!
//! Two independent capabilities, deliberately not wired to each other:
//!
//!   * [`auth`] - sign in to a platform and hold the credential in the OS
//!     keychain. Profile scopes only; this grants no access to media.
//!   * [`download`] - fetch public videos through yt-dlp. YouTube, Facebook
//!     and TikTok are fetched with no session at all. Instagram is the single
//!     exception: it refuses anonymous requests, so it uses a session the user
//!     captures in a dedicated login window, stored in the OS keychain.
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
pub mod license;
pub mod facebook;
pub mod process;
pub mod telegram;
pub mod tools;
pub mod youtube;

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

            // Make first-launch-installed tools discoverable: their folder goes
            // on PATH before anything tries to locate yt-dlp or ffmpeg.
            {
                let bin = data_dir.join("bin");
                let _ = std::fs::create_dir_all(&bin);
                let prev = std::env::var("PATH").unwrap_or_default();
                std::env::set_var("PATH", format!("{}:{}", bin.display(), prev));
            }

            let db = Arc::new(AccountDb::open(&data_dir.join("accounts.sqlite3"))?);
            let store = Arc::new(OsCredentialStore::new(data_dir.clone()));

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
            commands::license::license_status,
            commands::license::license_activate,
            commands::license::license_deactivate,
            commands::auth::auth_connect,
            commands::auth::auth_get_accounts,
            commands::auth::auth_get_account,
            commands::auth::auth_disconnect,
            commands::auth::auth_get_providers,
            commands::download::download_engine_status,
            commands::download::download_inspect,
            commands::download::download_inspect_formats,
            commands::download::download_start,
            commands::download::download_submit,
            commands::download::download_start_many,
            commands::download::download_list,
            commands::download::download_cancel,
            commands::download::download_remove,
            commands::download::download_clear_finished,
            commands::download::download_instagram_connect,
            commands::download::download_instagram_status,
            commands::download::download_facebook_connect,
            commands::download::download_facebook_status,
            commands::download::download_facebook_disconnect,
            commands::download::download_instagram_disconnect,
            commands::download::download_get_quality,
            commands::download::download_set_quality,
            commands::download::download_set_compatible,
            commands::download::download_get_destination,
            commands::download::download_set_destination,
            commands::download::download_reset_destination,
            commands::download::download_browse_destination,
            commands::download::download_reveal,
            commands::facebook::facebook_list_pages,
            commands::facebook::facebook_pick_photo,
            commands::facebook::facebook_upload_photo,
            commands::facebook::facebook_recent_downloads,
            commands::upload::upload_targets,
            commands::upload::upload_pick_files,
            commands::upload::upload_video_thumbnail,
            commands::upload::upload_video_meta,
            commands::upload::upload_youtube,
            commands::upload::upload_youtube_channels,
            commands::telegram::telegram_get_config,
            commands::telegram::telegram_set_config,
            commands::telegram::telegram_clear_config,
            commands::telegram::telegram_status,
            commands::telegram::telegram_get_session,
            commands::telegram::telegram_save_session,
            commands::telegram::telegram_set_display_name,
            commands::telegram::telegram_clear_session,
            commands::net::net_ping,
            commands::tools::tools_status,
            commands::tools::tools_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
