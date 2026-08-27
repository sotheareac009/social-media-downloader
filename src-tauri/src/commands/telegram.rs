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

/// Store the signed-in account's display name (from GramJS getMe). Non-secret.
#[tauri::command]
pub async fn telegram_set_display_name(
    app: tauri::AppHandle,
    name: String,
) -> Result<TelegramStatus> {
    let dir = data_dir(&app)?;
    telegram::save_display_name(&dir, &name)?;
    Ok(telegram::status(&dir))
}

#[tauri::command]
pub async fn telegram_clear_session(app: tauri::AppHandle) -> Result<TelegramStatus> {
    let dir = data_dir(&app)?;
    telegram::clear(&dir)?;
    Ok(telegram::status(&dir))
}

/// Write bytes the frontend already holds to a file the user chooses.
///
/// WHY A COMMAND AND NOT AN `<a download>`. A webview treats a blob download as
/// a navigation it has nowhere to put: in a Tauri window the link simply does
/// nothing, with no error to see. Saving has to go through the OS dialog.
///
/// THE BYTES ARRIVE RAW, not as a JSON array. Telegram media runs to tens of
/// megabytes, and a JSON-encoded byte array is roughly five times the size of
/// the file it describes - enough to stall the IPC channel on a single video.
/// `InvokeBody::Raw` is the path that carries binary as binary.
#[tauri::command]
pub async fn telegram_save_media(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<Option<String>> {
    use tauri::ipc::InvokeBody;
    use tauri_plugin_dialog::DialogExt;

    let bytes = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        // A JSON body here means the caller sent an array instead of a
        // Uint8Array, which would "work" but pay that five-fold cost.
        InvokeBody::Json(_) => {
            return Err(AppError::Internal("expected raw bytes, not JSON".into()))
        }
    };
    if bytes.is_empty() {
        return Err(AppError::Internal("nothing to save".into()));
    }

    // Headers are ASCII, and a Telegram filename is very often not - so the
    // frontend percent-encodes it and it is decoded back here.
    let suggested = request
        .headers()
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .unwrap_or_else(|| "telegram-media".to_string());

    // A folder means "save several without asking each time": the caller has
    // already chosen where, so a dialog per file would be an interrogation.
    let folder = request
        .headers()
        .get("x-dir")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .filter(|d| !d.is_empty());

    let path = match folder {
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).map_err(|e| {
                AppError::DownloadPath(format!("could not create {}: {e}", dir.display()))
            })?;
            unique_path(&dir.join(sanitise_name(&suggested)))
        }
        None => {
            let (tx, rx) = tokio::sync::oneshot::channel();
            app.dialog()
                .file()
                .set_title("Save media")
                .set_file_name(&suggested)
                .save_file(move |path| {
                    let _ = tx.send(path);
                });

            let picked = rx
                .await
                .map_err(|_| AppError::Internal("save dialog closed".into()))?;
            let Some(target) = picked else {
                // Dismissed: not an error, just nothing to do.
                return Ok(None);
            };
            target
                .into_path()
                .map_err(|e| AppError::DownloadPath(e.to_string()))?
        }
    };
    std::fs::write(&path, &bytes)
        .map_err(|e| AppError::DownloadPath(format!("could not write {}: {e}", path.display())))?;
    Ok(Some(path.display().to_string()))
}

/// Strip what a filesystem will not take from a name Telegram supplied.
///
/// A caption-derived filename can carry a slash, which would otherwise turn one
/// save into a write outside the chosen folder.
fn sanitise_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "telegram-media".to_string()
    } else {
        trimmed
    }
}

/// Never overwrite a file already in the folder: an album often carries two
/// items with the same name, and the second replacing the first is data loss
/// with nothing to show for it.
fn unique_path(desired: &std::path::Path) -> std::path::PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let stem = desired
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let ext = desired
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let dir = desired.parent().unwrap_or_else(|| std::path::Path::new("."));
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    desired.to_path_buf()
}

/// Decode the `%XX` escapes a filename was sent with.
///
/// Deliberately small: this only ever sees output from `encodeURIComponent`,
/// and anything it cannot decode is kept as-is rather than dropped, so a name
/// is never silently mangled into something shorter.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod save_tests {
    use super::percent_decode;

    #[test]
    fn a_non_ascii_filename_survives_the_header() {
        // What `encodeURIComponent("ភាពយន្ត.mp4")` produces.
        assert_eq!(percent_decode("%E1%9E%97.mp4"), "ភ.mp4");
        assert_eq!(percent_decode("clip%20one.mp4"), "clip one.mp4");
        assert_eq!(percent_decode("plain.mp4"), "plain.mp4");
    }

    #[test]
    fn a_name_cannot_escape_the_folder_it_is_saved_into() {
        use super::sanitise_name;
        // The property that matters: no separators survive, and it cannot come
        // back as a dotfile or a `..` entry.
        let cleaned = sanitise_name("../../etc/passwd");
        assert!(!cleaned.contains('/'), "{cleaned}");
        assert!(!cleaned.contains('\\'), "{cleaned}");
        assert!(!cleaned.starts_with('.'), "{cleaned}");
        assert_eq!(sanitise_name("clip: part 2.mp4"), "clip- part 2.mp4");
        assert_eq!(sanitise_name("   "), "telegram-media");
    }

    #[test]
    fn a_stray_percent_is_kept_rather_than_swallowing_the_name() {
        assert_eq!(percent_decode("100%.mp4"), "100%.mp4");
        assert_eq!(percent_decode("%zz.mp4"), "%zz.mp4");
    }
}
