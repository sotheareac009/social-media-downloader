//! IPC for the device layer: environment, instances, media transfer, app
//! control and screenshots.
//!
//! Thin by design. Every command here is a one-line delegation to
//! [`LdPlayerManager`], so the rules about layering and error shaping live in
//! one place instead of being re-decided per command.

use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::errors::{AppError, Result};
use crate::ldplayer::manager::{DeviceEnvironment, DeviceView, LdPlayerManager};
use crate::ldplayer::settings::DeviceSettings;

type Manager<'a> = State<'a, Arc<LdPlayerManager>>;

/// What tooling this machine has, for the Settings page and the setup notice.
#[tauri::command]
pub async fn ldplayer_environment(manager: Manager<'_>) -> Result<DeviceEnvironment> {
    Ok(manager.environment().await)
}

/// Re-run tool detection after the user installs LDPlayer without restarting.
#[tauri::command]
pub async fn ldplayer_redetect(manager: Manager<'_>) -> Result<DeviceEnvironment> {
    manager.redetect();
    Ok(manager.environment().await)
}

#[tauri::command]
pub async fn ldplayer_get_settings(manager: Manager<'_>) -> Result<DeviceSettings> {
    Ok(manager.settings())
}

#[tauri::command]
pub async fn ldplayer_set_settings(
    manager: Manager<'_>,
    settings: DeviceSettings,
) -> Result<DeviceSettings> {
    manager.set_settings(settings)
}

/// Every LDPlayer instance plus any other adb device.
#[tauri::command]
pub async fn ldplayer_list_devices(
    app: AppHandle,
    manager: Manager<'_>,
) -> Result<Vec<DeviceView>> {
    manager.list_devices(Some(&app)).await
}

#[tauri::command]
pub async fn ldplayer_start(
    app: AppHandle,
    manager: Manager<'_>,
    device_id: String,
) -> Result<DeviceView> {
    manager.start(Some(&app), &device_id).await
}

#[tauri::command]
pub async fn ldplayer_stop(
    app: AppHandle,
    manager: Manager<'_>,
    device_id: String,
) -> Result<DeviceView> {
    manager.stop(Some(&app), &device_id).await
}

/// Boot if needed and wait for Android. Long-running by nature — a cold
/// instance takes up to three minutes — so the UI shows a spinner rather than
/// polling.
#[tauri::command]
pub async fn ldplayer_connect(
    app: AppHandle,
    manager: Manager<'_>,
    device_id: String,
) -> Result<DeviceView> {
    manager.ensure_online(Some(&app), &device_id).await?;
    manager.emit_device(Some(&app), &device_id).await
}

/// Attach to a device by address, for anything auto-discovery misses.
#[tauri::command]
pub async fn ldplayer_connect_endpoint(
    app: AppHandle,
    manager: Manager<'_>,
    address: String,
) -> Result<DeviceView> {
    manager.connect_endpoint(Some(&app), &address).await
}

#[tauri::command]
pub async fn ldplayer_packages(manager: Manager<'_>, device_id: String) -> Result<Vec<String>> {
    manager.packages(&device_id).await
}

/// Copy a video onto a device and make the gallery see it, with no publishing.
///
/// Exposed on its own because it is the step people most need to test in
/// isolation: if this works, the emulator side of the feature is sound.
#[tauri::command]
pub async fn ldplayer_transfer_media(
    app: AppHandle,
    manager: Manager<'_>,
    device_id: String,
    path: String,
) -> Result<String> {
    manager
        .transfer_media(Some(&app), &device_id, Path::new(&path))
        .await
}

#[tauri::command]
pub async fn ldplayer_launch_app(
    app: AppHandle,
    manager: Manager<'_>,
    device_id: String,
    package: String,
) -> Result<()> {
    manager.launch_app(Some(&app), &device_id, &package).await
}

#[tauri::command]
pub async fn ldplayer_stop_app(
    manager: Manager<'_>,
    device_id: String,
    package: String,
) -> Result<()> {
    manager.stop_app(&device_id, &package).await
}

/// Capture the emulator screen. Returns an absolute path the UI renders
/// through Tauri's asset protocol.
#[tauri::command]
pub async fn ldplayer_screenshot(
    manager: Manager<'_>,
    device_id: String,
    label: Option<String>,
) -> Result<String> {
    manager.screenshot(&device_id, label.as_deref()).await
}

/// Pick a video from the computer. Mirrors `upload_pick_files`, but single-select
/// and video-only: publishing takes one video at a time.
#[tauri::command]
pub async fn ldplayer_pick_video(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a video to publish")
        .add_filter("Videos", &["mp4", "mov", "webm", "mkv", "avi", "m4v"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx
        .await
        .map_err(|_| AppError::Internal("file picker closed unexpectedly".into()))?;
    let Some(file) = picked else { return Ok(None) };
    Ok(Some(
        file.into_path()
            .map_err(|e| AppError::MediaFileMissing(e.to_string()))?
            .display()
            .to_string(),
    ))
}

/// Pick a folder or executable for the Settings page's path fields.
#[tauri::command]
pub async fn ldplayer_browse_path(app: AppHandle, kind: String) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();

    // The LDPlayer setting takes a folder (people know where they installed it,
    // not which .exe inside it we want); the ADB setting takes the binary.
    if kind == "folder" {
        dialog
            .set_title("Choose the LDPlayer folder")
            .pick_folder(move |f| {
                let _ = tx.send(f);
            });
    } else {
        dialog
            .set_title("Choose the ADB executable")
            .pick_file(move |f| {
                let _ = tx.send(f);
            });
    }

    let picked = rx
        .await
        .map_err(|_| AppError::Internal("file picker closed unexpectedly".into()))?;
    let Some(p) = picked else { return Ok(None) };
    Ok(Some(
        p.into_path()
            .map_err(|e| AppError::Internal(e.to_string()))?
            .display()
            .to_string(),
    ))
}
