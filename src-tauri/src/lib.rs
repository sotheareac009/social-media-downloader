//! SocialSync.
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
pub mod convert;
pub mod db;
pub mod download;
pub mod errors;
pub mod tiktok;
pub mod license;
pub mod facebook;
pub mod ldplayer;
pub mod publish;
pub mod process;
pub mod telegram;
pub mod tools;
pub mod x_post;
pub mod youtube;
pub mod youtube_accounts;

use std::sync::Arc;

use tauri::Manager;

use crate::auth::manager::AuthManager;
use crate::auth::storage::OsCredentialStore;
use crate::db::AccountDb;
use crate::download::manager::DownloadManager;
use crate::ldplayer::manager::LdPlayerManager;
use crate::publish::queue::PublishQueue;
use crate::publish::store::PublishStore;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load `.env` before anything reads configuration. Providers are built in
    // `setup`, so this must happen first.
    config::load_dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;

            // Make first-launch-installed tools discoverable: their folder goes
            // on PATH before anything tries to locate yt-dlp or ffmpeg.
            //
            // `join_paths` rather than formatting with ':' - that separator is
            // Unix-only, and hardcoding it on Windows (where it is ';') produced
            // a corrupt PATH entry, so an auto-installed yt-dlp.exe was never
            // found and the app reported it as not installed.
            {
                let bin = data_dir.join("bin");
                let _ = std::fs::create_dir_all(&bin);

                let existing = std::env::var_os("PATH").unwrap_or_default();
                let mut entries = vec![bin];

                // Binaries shipped inside the installer (Windows bundles
                // yt-dlp/ffmpeg/ffprobe as resources). Added AFTER the app-data
                // folder so a tool the user updated at runtime still wins over
                // the baseline copy frozen into the installer.
                if let Ok(res) = app.path().resource_dir() {
                    let bundled = res.join("bin");
                    if bundled.is_dir() {
                        entries.push(bundled);
                    }
                }

                entries.extend(std::env::split_paths(&existing));
                if let Ok(joined) = std::env::join_paths(entries) {
                    std::env::set_var("PATH", joined);
                }
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

            // Emulator publishing. Its store is a separate SQLite file from
            // `accounts.sqlite3` on purpose - see the note in
            // `publish::store`.
            let devices = Arc::new(LdPlayerManager::new(data_dir.clone()));
            let publish_store = Arc::new(PublishStore::open(&data_dir.join("publisher.sqlite3"))?);

            // A job still marked "uploading" from a previous run has nothing
            // driving it, so it would sit in the UI forever. Fail those once,
            // at startup, before anything can read them.
            match publish_store.fail_interrupted() {
                Ok(0) => {}
                Ok(n) => eprintln!("[publish] failed {n} job(s) interrupted by a previous exit"),
                Err(e) => eprintln!("[publish] could not tidy interrupted jobs: {e}"),
            }

            app.manage(Arc::new(PublishQueue::new(
                publish_store,
                devices.clone(),
            )));
            app.manage(devices);
            // One batch conversion at a time; the queue holds that state.
            app.manage(Arc::new(crate::convert::ConvertQueue::default()));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::upload::upload_tiktok,
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
            commands::download::download_x_connect,
            commands::download::download_x_status,
            commands::download::download_x_disconnect,
            commands::download::download_get_quality,
            commands::download::download_set_quality,
            commands::download::download_set_compatible,
            commands::download::download_get_destination,
            commands::download::download_set_destination,
            commands::download::download_reset_destination,
            commands::download::download_browse_destination,
            commands::download::download_reveal,
            commands::download::download_session_import_cookies,
            commands::download::download_session_check,
            commands::download::download_session_clear,
            commands::download::download_session_status,
            commands::convert::convert_capabilities,
            commands::convert::convert_pick_folder,
            commands::convert::convert_pick_output_dir,
            commands::convert::convert_pick_videos,
            commands::convert::convert_scan,
            commands::convert::convert_start,
            commands::convert::convert_cancel,
            commands::convert::convert_merge,
            commands::convert::convert_pick_file,
            commands::convert::convert_probe,
            commands::convert::convert_split,
            commands::facebook::facebook_list_pages,
            commands::facebook::facebook_pick_photo,
            commands::facebook::facebook_upload_photo,
            commands::facebook::facebook_recent_downloads,
            commands::upload::upload_targets,
            commands::upload::upload_pick_files,
            commands::upload::upload_video_thumbnail,
            commands::upload::upload_video_meta,
            commands::upload::upload_youtube,
            commands::upload::upload_x,
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
            commands::youtube_accounts::youtube_accounts_list,
            commands::youtube_accounts::youtube_account_add,
            commands::youtube_accounts::youtube_account_remove,
            commands::youtube_accounts::youtube_account_upload,
            commands::ldplayer::ldplayer_environment,
            commands::ldplayer::ldplayer_redetect,
            commands::ldplayer::ldplayer_get_settings,
            commands::ldplayer::ldplayer_set_settings,
            commands::ldplayer::ldplayer_list_devices,
            commands::ldplayer::ldplayer_start,
            commands::ldplayer::ldplayer_autoscroll_start,
            commands::ldplayer::ldplayer_autoscroll_stop,
            commands::ldplayer::ldplayer_autoscroll_remove,
            commands::ldplayer::ldplayer_autoscroll_status,
            commands::ldplayer::ldplayer_stop,
            commands::ldplayer::ldplayer_connect,
            commands::ldplayer::ldplayer_connect_endpoint,
            commands::ldplayer::ldplayer_packages,
            commands::ldplayer::ldplayer_transfer_media,
            commands::ldplayer::ldplayer_launch_app,
            commands::ldplayer::ldplayer_stop_app,
            commands::ldplayer::ldplayer_screenshot,
            commands::ldplayer::ldplayer_pick_media,
            commands::ldplayer::ldplayer_browse_path,
            commands::publish::publish_platforms,
            commands::publish::publish_accounts,
            commands::publish::publish_discover_accounts,
            commands::publish::publish_add_account,
            commands::publish::publish_rename_account,
            commands::publish::publish_set_profile_name,
            commands::publish::publish_discover_pages,
            commands::publish::publish_add_page,
            commands::publish::publish_remove_page,
            commands::publish::publish_remove_account,
            commands::publish::publish_submit,
            commands::publish::publish_jobs,
            commands::publish::publish_summary,
            commands::publish::publish_retry,
            commands::publish::publish_cancel,
            commands::publish::publish_remove_job,
            commands::publish::publish_clear_finished,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
