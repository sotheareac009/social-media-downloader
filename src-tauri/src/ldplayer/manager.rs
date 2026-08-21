//! The LDPlayer Manager: one service that owns every emulator interaction.
//!
//! Mirrors [`crate::download::manager::DownloadManager`] on purpose — commands
//! return the current view synchronously and events keep it fresh afterwards,
//! which is the pattern the frontend already knows.
//!
//! LAYERING, AND WHY IT IS ENFORCED RATHER THAN ASSUMED:
//!
//! ```text
//!   publish::connector   ← knows Facebook / Instagram / TikTok
//!          ↓ (only through the small API below)
//!   ldplayer::manager    ← knows emulators and files
//!          ↓
//!   ldplayer::console    ldplayer::adb
//! ```
//!
//! Nothing in this file, or below it, may mention a social platform. The
//! moment a package name or a screen tap for one app appears here, every other
//! connector has to work around it, and adding the fifth platform stops being
//! a new file and starts being a rewrite. A guard test at the bottom fails the
//! build if a platform name shows up in this module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::errors::{AppError, Result};
use crate::ldplayer::adb::{Adb, AdbDevice};
use crate::ldplayer::console::{conventional_endpoint, Instance, LdConsole};
use crate::ldplayer::settings::DeviceSettings;

pub mod events {
    /// The whole device list was refreshed.
    pub const DEVICES: &str = "ldplayer://devices";
    /// One device changed state (booting, online, stopped).
    pub const DEVICE: &str = "ldplayer://device";
    /// A line for the debug log pane.
    pub const LOG: &str = "ldplayer://log";
}

/// How long to wait for Android to finish booting after a launch. LDPlayer on
/// a cold, spinning disk has been measured at just under two minutes; three is
/// slack, not a target.
const BOOT_TIMEOUT: Duration = Duration::from_secs(180);

/// Whether this machine can drive emulators at all, and how.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceEnvironment {
    /// adb was found; without it nothing in this feature works.
    pub adb_available: bool,
    pub adb_path: Option<String>,
    pub adb_version: Option<String>,
    /// LDPlayer was found. False is not fatal — plain ADB devices still work,
    /// which is what makes this developable away from Windows.
    pub ldplayer_available: bool,
    pub ldplayer_path: Option<String>,
    /// True on the one platform LDPlayer ships for. The UI uses it to say
    /// "not available on macOS" instead of "not installed".
    pub ldplayer_supported: bool,
    pub remote_dir: String,
    pub max_concurrent: usize,
    pub verbose_logging: bool,
    pub cleanup_after_publish: bool,
}

/// How the app came to know about a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    /// An LDPlayer instance, startable and stoppable from this app.
    Ldplayer,
    /// Something else adb can see — a phone, a different emulator. Usable for
    /// publishing, but this app cannot boot it.
    Adb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceState {
    /// Known to LDPlayer, not running.
    Stopped,
    /// Launch requested; Android is not up yet.
    Booting,
    /// adb sees it, but it is `offline`/`unauthorized`.
    Unreachable,
    /// adb reports `device` and Android has finished booting.
    Online,
}

/// Everything the UI renders for one device.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    /// Stable id: `ld:<index>` for an instance, `adb:<serial>` otherwise. Used
    /// as the foreign key from an account, so it must survive a reboot — which
    /// a raw serial does not, because the port can change.
    pub id: String,
    pub kind: DeviceKind,
    /// LDPlayer's index, absent for plain adb devices.
    pub index: Option<u32>,
    /// The instance name, or the serial when there is nothing better.
    pub name: String,
    pub state: DeviceState,
    /// Present only while reachable.
    pub serial: Option<String>,
    pub model: Option<String>,
    pub android_release: Option<String>,
    /// Filled in on demand — listing packages for every device on every
    /// refresh costs a shell round trip each and is not worth it.
    pub packages: Option<Vec<String>>,
    pub error: Option<String>,
}

impl DeviceView {
    pub fn is_online(&self) -> bool {
        self.state == DeviceState::Online
    }
}

/// One line of the debug log, mirrored to the UI when verbose logging is on.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub at: i64,
    /// "info" | "warn" | "error".
    pub level: String,
    /// Device id or job id this line belongs to, when it has one.
    pub scope: Option<String>,
    pub message: String,
}

pub struct LdPlayerManager {
    app_data: PathBuf,
    settings: Mutex<DeviceSettings>,
    /// Resolved tools, re-derived whenever settings change. `None` means "not
    /// found on the last look", not "never looked".
    console: Mutex<Option<LdConsole>>,
    adb: Mutex<Option<Adb>>,
    /// index → serial, so a publish batch doesn't re-ask LDPlayer per job.
    /// Cleared whenever an instance stops, because the port can move.
    serials: Mutex<HashMap<u32, String>>,
    /// Instances we asked to launch, so the list can show "Booting" instead of
    /// flapping back to "Stopped" for the minute before Android is up.
    booting: Mutex<Vec<u32>>,
}

impl LdPlayerManager {
    pub fn new(app_data: PathBuf) -> Self {
        let settings = DeviceSettings::load(&app_data);
        let m = Self {
            app_data,
            settings: Mutex::new(settings),
            console: Mutex::new(None),
            adb: Mutex::new(None),
            serials: Mutex::new(HashMap::new()),
            booting: Mutex::new(Vec::new()),
        };
        m.redetect();
        m
    }

    // ---------------------------------------------------------- settings

    pub fn settings(&self) -> DeviceSettings {
        self.settings.lock().expect("settings lock").clone()
    }

    /// Replace the settings, persist them, and re-run tool detection — a new
    /// adb path must take effect without a restart.
    pub fn set_settings(&self, next: DeviceSettings) -> Result<DeviceSettings> {
        next.save(&self.app_data)
            .map_err(|e| AppError::Internal(format!("could not save device settings: {e}")))?;
        *self.settings.lock().expect("settings lock") = next;
        self.redetect();
        Ok(self.settings())
    }

    /// Re-run discovery for both tools. Cheap (a few `is_file` checks plus, on
    /// Windows, one registry read), so it is safe to call on every settings
    /// change and at startup.
    pub fn redetect(&self) {
        let s = self.settings();
        let console = LdConsole::discover(s.ldplayer_path.as_deref());
        let adb = Adb::discover(s.adb_path.as_deref(), console.as_ref().map(|c| c.dir()));
        *self.console.lock().expect("console lock") = console;
        *self.adb.lock().expect("adb lock") = adb;
        self.serials.lock().expect("serials lock").clear();
    }

    fn console(&self) -> Result<LdConsole> {
        self.console
            .lock()
            .expect("console lock")
            .clone()
            .ok_or(AppError::LdPlayerMissing)
    }

    /// The adb handle, or a clear error. Every device operation starts here,
    /// so this is the single place "adb is missing" is decided.
    pub fn adb(&self) -> Result<Adb> {
        self.adb
            .lock()
            .expect("adb lock")
            .clone()
            .ok_or(AppError::AdbMissing)
    }

    pub async fn environment(&self) -> DeviceEnvironment {
        let s = self.settings();
        let adb = self.adb.lock().expect("adb lock").clone();
        let console = self.console.lock().expect("console lock").clone();
        let adb_version = match &adb {
            Some(a) => a.version().await.ok(),
            None => None,
        };
        DeviceEnvironment {
            adb_available: adb.is_some(),
            adb_path: adb.as_ref().map(|a| a.path().display().to_string()),
            adb_version,
            ldplayer_available: console.is_some(),
            ldplayer_path: console.as_ref().map(|c| c.path().display().to_string()),
            ldplayer_supported: cfg!(windows),
            remote_dir: s.remote_dir,
            max_concurrent: s.max_concurrent,
            verbose_logging: s.verbose_logging,
            cleanup_after_publish: s.cleanup_after_publish,
        }
    }

    // ------------------------------------------------------------ logging

    /// Record a line and, when verbose logging is on, mirror it to the UI.
    ///
    /// Errors are always emitted regardless of the setting: a failure the user
    /// cannot see is a support ticket.
    pub fn log(&self, app: Option<&AppHandle>, level: &str, scope: Option<&str>, message: impl Into<String>) {
        let line = LogLine {
            at: now_unix(),
            level: level.to_string(),
            scope: scope.map(str::to_string),
            message: message.into(),
        };
        let verbose = self.settings().verbose_logging;
        if verbose || level == "error" {
            eprintln!(
                "[ldplayer] {} {} {}",
                line.level,
                line.scope.as_deref().unwrap_or("-"),
                line.message
            );
            if let Some(app) = app {
                let _ = app.emit(events::LOG, line);
            }
        }
    }

    // ------------------------------------------------------------- devices

    /// Every device the app can publish to: LDPlayer instances first (running
    /// or not), then any other adb device that isn't already one of them.
    ///
    /// Tolerant by construction. A missing LDPlayer yields the adb-only list
    /// rather than an error, because a user with one phone plugged in should
    /// still see it.
    pub async fn list_devices(&self, app: Option<&AppHandle>) -> Result<Vec<DeviceView>> {
        let adb = self.adb()?;
        adb.start_server().await.ok();

        let attached = adb.devices().await.unwrap_or_default();
        let mut views = Vec::new();
        let mut claimed: Vec<String> = Vec::new();

        let instances = match self.console() {
            Ok(console) => console.list().await.unwrap_or_else(|e| {
                self.log(app, "warn", None, format!("could not list LDPlayer instances: {e}"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };

        for inst in instances {
            let view = self.view_for_instance(&adb, &inst, &attached).await;
            if let Some(s) = &view.serial {
                claimed.push(s.clone());
            }
            views.push(view);
        }

        for d in &attached {
            if claimed.iter().any(|s| s == &d.serial) {
                continue;
            }
            views.push(self.view_for_adb_device(&adb, d).await);
        }

        if let Some(app) = app {
            let _ = app.emit(events::DEVICES, &views);
        }
        Ok(views)
    }

    async fn view_for_instance(
        &self,
        adb: &Adb,
        inst: &Instance,
        attached: &[AdbDevice],
    ) -> DeviceView {
        let id = format!("ld:{}", inst.index);
        let mut view = DeviceView {
            id,
            kind: DeviceKind::Ldplayer,
            index: Some(inst.index),
            name: inst.name.clone(),
            state: DeviceState::Stopped,
            serial: None,
            model: None,
            android_release: None,
            packages: None,
            error: None,
        };

        if !inst.running {
            self.serials.lock().expect("serials lock").remove(&inst.index);
            self.booting.lock().expect("booting lock").retain(|i| *i != inst.index);
            return view;
        }

        match self.serial_for(inst.index).await {
            Ok(serial) => {
                let online = attached.iter().any(|d| d.serial == serial && d.is_online());
                view.serial = Some(serial.clone());
                if online && adb.is_booted(&serial).await {
                    view.state = DeviceState::Online;
                    view.model = adb.model(&serial).await;
                    view.android_release = adb.android_release(&serial).await;
                    self.booting.lock().expect("booting lock").retain(|i| *i != inst.index);
                } else {
                    // Running but not answering yet is the normal state for the
                    // first minute after a launch, so it reads as "Booting"
                    // rather than as a fault.
                    view.state = if self.is_booting(inst.index) {
                        DeviceState::Booting
                    } else {
                        DeviceState::Unreachable
                    };
                }
            }
            Err(e) => {
                view.state = if self.is_booting(inst.index) {
                    DeviceState::Booting
                } else {
                    DeviceState::Unreachable
                };
                view.error = Some(e.to_string());
            }
        }
        view
    }

    async fn view_for_adb_device(&self, adb: &Adb, d: &AdbDevice) -> DeviceView {
        let online = d.is_online();
        DeviceView {
            id: format!("adb:{}", d.serial),
            kind: DeviceKind::Adb,
            index: None,
            name: if online {
                adb.model(&d.serial).await.unwrap_or_else(|| d.serial.clone())
            } else {
                d.serial.clone()
            },
            state: if online { DeviceState::Online } else { DeviceState::Unreachable },
            serial: Some(d.serial.clone()),
            model: if online { adb.model(&d.serial).await } else { None },
            android_release: if online { adb.android_release(&d.serial).await } else { None },
            packages: None,
            error: (!online).then(|| format!("adb reports this device as {}", d.state)),
        }
    }

    fn is_booting(&self, index: u32) -> bool {
        self.booting.lock().expect("booting lock").contains(&index)
    }

    /// Resolve one device id (`ld:0`, `adb:emulator-5554`) to a live serial,
    /// connecting if needed. The single entry point connectors use, so that
    /// "which emulator is this account on" is answered in exactly one place.
    pub async fn serial_for_device(&self, device_id: &str) -> Result<String> {
        match device_id.split_once(':') {
            Some(("ld", idx)) => {
                let index: u32 = idx
                    .parse()
                    .map_err(|_| AppError::InstanceNotFound(device_id.to_string()))?;
                self.serial_for(index).await
            }
            Some(("adb", serial)) => Ok(serial.to_string()),
            _ => Err(AppError::InstanceNotFound(device_id.to_string())),
        }
    }

    /// Serial for an LDPlayer index, cached.
    ///
    /// Asks LDPlayer first, because computing the port from the index is only
    /// right on a machine where instances were never deleted — and being wrong
    /// means publishing to the wrong account, which is the worst failure this
    /// feature has. The computed endpoint is a last resort, and only after an
    /// explicit `adb connect` proves something is actually there.
    pub async fn serial_for(&self, index: u32) -> Result<String> {
        if let Some(s) = self.serials.lock().expect("serials lock").get(&index) {
            return Ok(s.clone());
        }
        let adb = self.adb()?;

        let resolved = match self.console() {
            Ok(console) => console.serial(index).await.ok(),
            Err(_) => None,
        };

        let serial = match resolved {
            Some(s) => s,
            None => {
                let endpoint = conventional_endpoint(index);
                adb.connect(&endpoint).await?;
                endpoint
            }
        };

        self.serials
            .lock()
            .expect("serials lock")
            .insert(index, serial.clone());
        Ok(serial)
    }

    /// Ensure a device is up and answering, booting it if this app is allowed
    /// to. Every publish begins here.
    pub async fn ensure_online(&self, app: Option<&AppHandle>, device_id: &str) -> Result<String> {
        let adb = self.adb()?;

        // A plain adb device is somebody else's to start.
        if let Some(("adb", serial)) = device_id.split_once(':') {
            if adb.is_booted(serial).await {
                return Ok(serial.to_string());
            }
            return Err(AppError::InstanceOffline(serial.to_string()));
        }

        let index = device_index(device_id)?;
        let console = self.console()?;

        if !console.is_running(index).await.unwrap_or(false) {
            self.log(app, "info", Some(device_id), "starting LDPlayer instance");
            self.booting.lock().expect("booting lock").push(index);
            console.launch(index).await?;
        }

        let serial = self.wait_for_serial(index).await?;
        adb.wait_for_device(&serial, BOOT_TIMEOUT).await?;
        self.booting.lock().expect("booting lock").retain(|i| *i != index);
        self.log(app, "info", Some(device_id), format!("online as {serial}"));
        Ok(serial)
    }

    /// A just-launched instance has no adb port for a few seconds, so the
    /// serial lookup has to be retried rather than failed.
    async fn wait_for_serial(&self, index: u32) -> Result<String> {
        let deadline = std::time::Instant::now() + BOOT_TIMEOUT;
        loop {
            if let Ok(s) = self.serial_for(index).await {
                return Ok(s);
            }
            if std::time::Instant::now() >= deadline {
                return Err(AppError::InstanceOffline(index.to_string()));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Attach to a device by address, for the cases auto-discovery cannot
    /// reach: an LDPlayer install this app failed to detect, an emulator from
    /// another vendor, or a device on another machine.
    ///
    /// The escape hatch matters more than it looks. Without it, a user whose
    /// LDPlayer sits somewhere unusual has an empty device list and no way at
    /// all to proceed — the feature looks broken rather than misconfigured.
    pub async fn connect_endpoint(
        &self,
        app: Option<&AppHandle>,
        address: &str,
    ) -> Result<DeviceView> {
        let address = normalize_endpoint(address)?;
        let adb = self.adb()?;
        adb.start_server().await.ok();
        self.log(app, "info", None, format!("connecting to {address}"));
        adb.connect(&address).await?;

        // Re-scan rather than synthesising a view: the device may turn out to
        // be an LDPlayer instance the console also knows about, and it should
        // appear once, as that instance, with its Start/Stop controls.
        let devices = self.list_devices(app).await?;
        devices
            .into_iter()
            .find(|d| d.serial.as_deref() == Some(address.as_str()))
            .ok_or(AppError::InstanceOffline(address))
    }

    pub async fn start(&self, app: Option<&AppHandle>, device_id: &str) -> Result<DeviceView> {
        let index = device_index(device_id)?;
        self.booting.lock().expect("booting lock").push(index);
        self.console()?.launch(index).await?;
        self.emit_device(app, device_id).await
    }

    pub async fn stop(&self, app: Option<&AppHandle>, device_id: &str) -> Result<DeviceView> {
        let index = device_index(device_id)?;
        self.console()?.quit(index).await?;
        self.serials.lock().expect("serials lock").remove(&index);
        self.booting.lock().expect("booting lock").retain(|i| *i != index);
        self.emit_device(app, device_id).await
    }

    /// Re-read one device and tell the UI about it.
    pub async fn emit_device(&self, app: Option<&AppHandle>, device_id: &str) -> Result<DeviceView> {
        let view = self
            .list_devices(None)
            .await?
            .into_iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| AppError::InstanceNotFound(device_id.to_string()))?;
        if let Some(app) = app {
            let _ = app.emit(events::DEVICE, &view);
        }
        Ok(view)
    }

    /// Packages installed on a device. Separate from `list_devices` because it
    /// costs a shell round trip and is only needed when the user is actually
    /// looking at one device's detail.
    pub async fn packages(&self, device_id: &str) -> Result<Vec<String>> {
        let serial = self.serial_for_device(device_id).await?;
        self.adb()?.installed_packages(&serial).await
    }

    // -------------------------------------------------------------- media

    /// Copy a local file to the device and make Android's gallery see it.
    ///
    /// This is the whole "no drag and drop" promise in one function, and the
    /// verification steps are the point of it:
    ///
    /// 1. push the file;
    /// 2. compare byte counts, so a truncated transfer fails here rather than
    ///    as a mystifying "app can't read the video" ten steps later;
    /// 3. ask MediaStore to index it;
    /// 4. confirm MediaStore actually did — a scan that silently no-ops is the
    ///    single most common cause of an empty gallery picker.
    ///
    /// Returns the on-device path, which is what a connector hands to the app.
    pub async fn transfer_media(
        &self,
        app: Option<&AppHandle>,
        device_id: &str,
        local: &Path,
    ) -> Result<String> {
        let serial = self.serial_for_device(device_id).await?;
        let adb = self.adb()?;
        let settings = self.settings();

        let file_name = sanitize_file_name(local);
        let remote = settings.remote_path_for(&file_name);

        self.log(app, "info", Some(device_id), format!("pushing {file_name} → {remote}"));
        adb.push(&serial, local, &remote).await?;

        let local_size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        if let Some(remote_size) = adb.remote_size(&serial, &remote).await {
            if local_size > 0 && remote_size != local_size {
                return Err(AppError::MediaTransferFailed(format!(
                    "only {remote_size} of {local_size} bytes arrived"
                )));
            }
        }

        if let Err(e) = adb.scan_media(&serial, &remote).await {
            self.log(app, "warn", Some(device_id), format!("media scan reported: {e}"));
        }
        if !adb.is_in_media_store(&serial, &remote).await {
            // One retry: the first scan after a push occasionally races the
            // filesystem, and a second request a moment later succeeds.
            tokio::time::sleep(Duration::from_millis(1200)).await;
            adb.scan_media(&serial, &remote).await.ok();
            if !adb.is_in_media_store(&serial, &remote).await {
                return Err(AppError::MediaScanFailed);
            }
        }

        self.log(app, "info", Some(device_id), "media is visible to the gallery");
        Ok(remote)
    }

    /// Remove a pushed file and un-index it. Best effort by design: a leftover
    /// file is untidy, while a failed cleanup that fails a successful publish
    /// would be actively wrong.
    pub async fn remove_media(&self, device_id: &str, remote: &str) {
        let Ok(serial) = self.serial_for_device(device_id).await else { return };
        let Ok(adb) = self.adb() else { return };
        let _ = adb
            .shell(&serial, &format!("rm -f {}", crate::ldplayer::adb::shell_quote(remote)))
            .await;
        let _ = adb.scan_media(&serial, remote).await;
    }

    // ----------------------------------------------------------- app control

    pub async fn launch_app(&self, app: Option<&AppHandle>, device_id: &str, package: &str) -> Result<()> {
        let serial = self.serial_for_device(device_id).await?;
        self.log(app, "info", Some(device_id), format!("launching {package}"));
        self.adb()?.launch_app(&serial, package).await
    }

    pub async fn stop_app(&self, device_id: &str, package: &str) -> Result<()> {
        let serial = self.serial_for_device(device_id).await?;
        self.adb()?.stop_app(&serial, package).await
    }

    // ------------------------------------------------------------ screenshots

    /// Capture the screen to a PNG under the app's data directory and return
    /// its absolute path, for the UI to render through the asset protocol.
    ///
    /// Screenshots are how a person answers "what is it stuck on?" without
    /// alt-tabbing to LDPlayer, and how a connector proves it reached the
    /// screen it thought it did.
    pub async fn screenshot(&self, device_id: &str, label: Option<&str>) -> Result<String> {
        let serial = self.serial_for_device(device_id).await?;
        let bytes = self.adb()?.screenshot(&serial).await?;

        let dir = self.screenshot_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::Internal(format!("could not create screenshot folder: {e}")))?;

        let stem = label.map(slug).unwrap_or_else(|| "screen".to_string());
        let path = dir.join(format!("{}-{}-{}.png", slug(device_id), stem, now_unix()));
        std::fs::write(&path, &bytes)
            .map_err(|e| AppError::Internal(format!("could not write screenshot: {e}")))?;

        self.prune_screenshots(&dir);
        Ok(path.display().to_string())
    }

    pub fn screenshot_dir(&self) -> PathBuf {
        self.app_data.join("screenshots")
    }

    /// Keep the newest few hundred. Verbose logging captures a screenshot per
    /// publishing step, so an unbounded folder would quietly grow to gigabytes.
    fn prune_screenshots(&self, dir: &Path) {
        const KEEP: usize = 300;
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
            .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()).map(|t| (t, e.path())))
            .collect();
        if files.len() <= KEEP {
            return;
        }
        files.sort_by_key(|(t, _)| *t);
        for (_, path) in files.iter().take(files.len() - KEEP) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `ld:3` → 3, with a clear error for anything else. Plain adb devices have no
/// index, and asking this app to boot one is a caller mistake worth naming.
fn device_index(device_id: &str) -> Result<u32> {
    match device_id.split_once(':') {
        Some(("ld", idx)) => idx
            .parse()
            .map_err(|_| AppError::InstanceNotFound(device_id.to_string())),
        _ => Err(AppError::NotAnLdplayerInstance(device_id.to_string())),
    }
}

/// A device-safe file name. Android's sdcard is FAT-derived, so a colon or a
/// question mark makes the push fail with an error nobody can act on.
fn sanitize_file_name(local: &Path) -> String {
    let raw = local
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".to_string());
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || "._- ".contains(c) { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "video.mp4".to_string()
    } else {
        cleaned
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}


/// Accept what people actually type: `5555`, `127.0.0.1:5555`, or a host and
/// port. A bare port is by far the most common, because that is the number
/// LDPlayer shows.
fn normalize_endpoint(input: &str) -> Result<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AppError::InvalidDeviceAddress(input.to_string()));
    }
    if let Ok(port) = raw.parse::<u16>() {
        return Ok(format!("127.0.0.1:{port}"));
    }
    let (host, port) = raw
        .rsplit_once(':')
        .ok_or_else(|| AppError::InvalidDeviceAddress(input.to_string()))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err(AppError::InvalidDeviceAddress(input.to_string()));
    }
    Ok(raw.to_string())
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_round_trip() {
        assert_eq!(device_index("ld:0").unwrap(), 0);
        assert_eq!(device_index("ld:12").unwrap(), 12);
        assert!(device_index("adb:emulator-5554").is_err());
        assert!(device_index("nonsense").is_err());
    }

    #[test]
    fn file_names_are_made_safe_for_the_sdcard() {
        assert_eq!(sanitize_file_name(Path::new("/x/my video.mp4")), "my video.mp4");
        assert_eq!(sanitize_file_name(Path::new("/x/a:b?c.mp4")), "a_b_c.mp4");
        assert_eq!(sanitize_file_name(Path::new("/")), "video.mp4");
    }


    #[test]
    fn endpoints_accept_what_people_actually_type() {
        assert_eq!(normalize_endpoint("5555").unwrap(), "127.0.0.1:5555");
        assert_eq!(normalize_endpoint(" 5555 ").unwrap(), "127.0.0.1:5555");
        assert_eq!(
            normalize_endpoint("127.0.0.1:5555").unwrap(),
            "127.0.0.1:5555"
        );
        assert_eq!(normalize_endpoint("192.168.1.9:5555").unwrap(), "192.168.1.9:5555");
    }

    #[test]
    fn endpoints_reject_what_cannot_be_a_device() {
        for bad in ["", "   ", "localhost", "127.0.0.1:", ":5555", "127.0.0.1:99999"] {
            assert!(normalize_endpoint(bad).is_err(), "{bad:?} should be refused");
        }
    }

    /// The layering rule from this module's header, enforced.
    ///
    /// If this fails, the platform-specific thing you just added belongs in a
    /// connector under `crate::publish::connector`, not here.
    ///
    /// Comments are stripped before scanning: this module's own header draws
    /// the layering diagram, and naming the platforms is exactly how it
    /// explains which ones must stay out of the code.
    #[test]
    fn the_device_layer_names_no_social_platform() {
        let source = include_str!("manager.rs");
        // Everything above this test, minus comment lines.
        let body: String = source
            .split("fn the_device_layer_names_no_social_platform")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();

        for name in ["facebook", "instagram", "tiktok", "youtube", "com.zhiliaoapp"] {
            assert!(
                !body.contains(name),
                "`{name}` leaked into the generic LDPlayer layer; put it in a connector"
            );
        }
    }
}
