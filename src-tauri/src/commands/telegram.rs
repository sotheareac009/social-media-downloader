//! Tauri commands for Telegram.
//!
//! These persist and retrieve the GramJS session string, and expose the
//! app's api_id/api_hash so the frontend can build a client. The login flow
//! itself - phone, code, 2FA - runs in the frontend, because MTProto lives in
//! GramJS; Rust only stores the result.

use tauri::Manager;

use crate::config;
use crate::errors::{AppError, Result};
use crate::telegram::{self, TelegramCredentials, TelegramStatus};

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
///
/// Values saved in Settings win over `.env`, because a packaged build does not
/// read `.env` at all - which is the whole reason the Settings form exists.
#[tauri::command]
pub async fn telegram_get_config(app: tauri::AppHandle) -> Result<TelegramConfig> {
    let dir = data_dir(&app)?;

    let (api_id_raw, api_hash) = match telegram::load_credentials(&dir) {
        Some(c) => (Some(c.api_id), Some(c.api_hash)),
        None => (config::read("TELEGRAM_API_ID"), config::read("TELEGRAM_API_HASH")),
    };

    let api_id = api_id_raw.as_deref().map(str::trim).and_then(|s| s.parse::<i32>().ok());
    let api_hash = api_hash.map(|h| h.trim().to_string());

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

/// Save api_id / api_hash entered in Settings, so a packaged build can be
/// configured without editing `.env`. Rejects a non-numeric api_id up front.
#[tauri::command]
pub async fn telegram_set_config(
    app: tauri::AppHandle,
    api_id: String,
    api_hash: String,
) -> Result<TelegramConfig> {
    let id_trimmed = api_id.trim();
    let hash_trimmed = api_hash.trim();

    if id_trimmed.parse::<i32>().map(|n| n <= 0).unwrap_or(true) {
        return Err(AppError::Internal(
            "api_id must be a positive number — it's the shorter value on my.telegram.org".into(),
        ));
    }
    if hash_trimmed.len() < 8 {
        return Err(AppError::Internal(
            "api_hash looks wrong — it's the long hex string on my.telegram.org".into(),
        ));
    }

    telegram::save_credentials(
        &data_dir(&app)?,
        &TelegramCredentials {
            api_id: id_trimmed.to_string(),
            api_hash: hash_trimmed.to_string(),
        },
    )?;
    telegram_get_config(app).await
}

/// Forget saved credentials (falls back to `.env`, if any).
#[tauri::command]
pub async fn telegram_clear_config(app: tauri::AppHandle) -> Result<TelegramConfig> {
    telegram::clear_credentials(&data_dir(&app)?)?;
    telegram_get_config(app).await
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
