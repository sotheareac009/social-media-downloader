//! IPC for the device layer: environment, instances, media transfer, app
//! control and screenshots.
//!
//! Thin by design. Every command here is a one-line delegation to
//! [`LdPlayerManager`], so the rules about layering and error shaping live in
//! one place instead of being re-decided per command.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, State};

use crate::errors::{AppError, Result};
use crate::ldplayer::manager::{DeviceEnvironment, DeviceView, LdPlayerManager, TransferredMedia};
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

/// Start the feed-scroll loop across the given instances: repeated upward
/// swipes at `interval_ms`, until stopped. Stopped instances are booted first.
#[tauri::command]
pub async fn ldplayer_autoscroll_start(
    app: AppHandle,
    manager: Manager<'_>,
    device_ids: Vec<String>,
    interval_ms: u64,
    // Priority list of app packages to try; the first one installed on each
    // device is opened (e.g. full Facebook, then Facebook Lite).
    packages: Option<Vec<String>>,
) -> Result<()> {
    if device_ids.is_empty() {
        return Err(AppError::Internal("pick at least one instance to scroll".into()));
    }
    let mgr = manager.inner().clone();
    if !mgr.begin_autoscroll(device_ids.clone()) {
        return Err(AppError::Internal("auto-scroll is already running".into()));
    }
    // Floor the interval so a tiny value can't hammer adb.
    let interval = Duration::from_millis(interval_ms.max(500));

    tauri::async_runtime::spawn(async move {
        // One-click flow: bring every device online first — a stopped LDPlayer
        // instance is launched and waited for; an adb device must already be
        // running — then open the chosen app on each. For each device the first
        // installed package from the priority list wins, so "Facebook" opens the
        // full app when present and falls back to Facebook Lite otherwise.
        let candidates = packages.unwrap_or_default();
        for id in &device_ids {
            // A device removed before it even booted is simply skipped.
            if !mgr.autoscroll_is_active(id) {
                continue;
            }
            // Boot + wait. If it can't come online, skip it; the swipe loop
            // will simply have nothing to do for that device.
            if mgr.ensure_online(Some(&app), id).await.is_err() {
                continue;
            }
            for pkg in &candidates {
                if mgr.launch_app(Some(&app), id, pkg).await.is_ok() {
                    break;
                }
            }
        }
        // Let the app settle on its feed before the first swipe.
        if !candidates.is_empty() {
            let mut waited = Duration::ZERO;
            let load = Duration::from_secs(5);
            while waited < load && !mgr.autoscroll_active_ids().is_empty() {
                tokio::time::sleep(Duration::from_millis(250)).await;
                waited += Duration::from_millis(250);
            }
        }

        // Scroll every still-active device each pass; the loop ends when the
        // set empties (Stop, or every device removed one by one).
        loop {
            let ids = mgr.autoscroll_active_ids();
            if ids.is_empty() {
                break;
            }
            for id in &ids {
                if !mgr.autoscroll_is_active(id) {
                    continue;
                }
                // A device that errors (still booting, stopped) is skipped, not
                // fatal — the loop keeps the others scrolling.
                let _ = mgr.swipe_up(Some(&app), id).await;
            }
            // Wait in small slices so Stop is responsive mid-interval.
            let mut waited = Duration::ZERO;
            let slice = Duration::from_millis(200);
            while waited < interval && !mgr.autoscroll_active_ids().is_empty() {
                tokio::time::sleep(slice).await;
                waited += slice;
            }
        }
        mgr.end_autoscroll();
    });
    Ok(())
}

/// Ask the auto-scroll loop to stop after its current pass.
#[tauri::command]
pub async fn ldplayer_autoscroll_stop(manager: Manager<'_>) -> Result<()> {
    manager.stop_autoscroll();
    Ok(())
}

/// Stop scrolling one device without touching the others.
#[tauri::command]
pub async fn ldplayer_autoscroll_remove(manager: Manager<'_>, device_id: String) -> Result<()> {
    manager.autoscroll_remove(&device_id);
    Ok(())
}

/// The device ids currently being scrolled (empty when idle).
#[tauri::command]
pub async fn ldplayer_autoscroll_status(manager: Manager<'_>) -> Result<Vec<String>> {
    Ok(manager.autoscroll_active_ids())
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
) -> Result<TransferredMedia> {
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

/// Pick a video or photo from the computer. Mirrors `upload_pick_files`, but
/// multi-select, because an album post is several files chosen together.
///
/// The combined filter is listed first so the default view shows everything
/// publishable — someone who opened the dialog wanting a photo should not have
/// to notice a dropdown to see one.
///
/// Returns an empty list when the dialog was cancelled, which is not an error.
#[tauri::command]
pub async fn ldplayer_pick_media(app: AppHandle) -> Result<Vec<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a video or photo to publish")
        .add_filter(
            "Videos and photos",
            &[
                "mp4", "mov", "webm", "mkv", "avi", "m4v", "jpg", "jpeg", "png", "gif",
                "webp", "heic",
            ],
        )
        .add_filter("Videos", &["mp4", "mov", "webm", "mkv", "avi", "m4v"])
        .add_filter("Photos", &["jpg", "jpeg", "png", "gif", "webp", "heic"])
        .pick_files(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx
        .await
        .map_err(|_| AppError::Internal("file picker closed unexpectedly".into()))?;

    let mut out = Vec::new();
    for file in picked.unwrap_or_default() {
        out.push(
            file.into_path()
                .map_err(|e| AppError::MediaFileMissing(e.to_string()))?
                .display()
                .to_string(),
        );
    }
    Ok(out)
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
