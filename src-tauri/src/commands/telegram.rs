//! Tauri commands for Telegram.
//!
//! These persist and retrieve the GramJS session string, and expose the
//! app's api_id/api_hash so the frontend can build a client. The login flow
//! itself - phone, code, 2FA - runs in the frontend, because MTProto lives in
//! GramJS; Rust only stores the result.

use tauri::Manager;

use crate::config;
use crate::errors::{AppError, Result};
use crate::telegram::{self, TelegramStatus};

/// The app credentials GramJS needs, plus whether this build has them.
#[derive(serde::Serialize)]
pub struct TelegramConfig {
    pub configured: bool,
    /// api_id is a number; sent as the value GramJS expects, or 0 when unset.
    pub api_id: i32,
    pub api_hash: String,
}

/// Non-secret: identifies the application to Telegram, not the user. Safe to
/// hand the frontend, which is where GramJS runs.
#[tauri::command]
pub async fn telegram_get_config() -> Result<TelegramConfig> {
    let api_id_raw = config::read("TELEGRAM_API_ID");
    let api_hash = config::read("TELEGRAM_API_HASH");

    let api_id = api_id_raw.as_deref().and_then(|s| s.parse::<i32>().ok());

    match (api_id, api_hash) {
        (Some(id), Some(hash)) if id > 0 && !hash.is_empty() => Ok(TelegramConfig {
            configured: true,
            api_id: id,
            api_hash: hash,
        }),
        _ => Ok(TelegramConfig {
            configured: false,
            api_id: 0,
            api_hash: String::new(),
        }),
    }
}

fn data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("no app data dir: {e}")))
}

#[tauri::command]
pub async fn telegram_status(app: tauri::AppHandle) -> Result<TelegramStatus> {
    Ok(telegram::status(&data_dir(&app)?))
}

/// The stored session string, for reconnecting on launch. Returns `null` when
/// signed out. See the module note on why this value reaches JavaScript.
#[tauri::command]
pub async fn telegram_get_session(app: tauri::AppHandle) -> Result<Option<String>> {
    Ok(telegram::load(&data_dir(&app)?))
}

#[tauri::command]
pub async fn telegram_save_session(
    app: tauri::AppHandle,
    session: String,
) -> Result<TelegramStatus> {
    let dir = data_dir(&app)?;
    telegram::save(&dir, &session)?;
    Ok(telegram::status(&dir))
}

#[tauri::command]
pub async fn telegram_clear_session(app: tauri::AppHandle) -> Result<TelegramStatus> {
    let dir = data_dir(&app)?;
    telegram::clear(&dir)?;
    Ok(telegram::status(&dir))
}
