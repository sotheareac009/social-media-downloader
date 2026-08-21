//! Generic Android Debug Bridge client.
//!
//! LAYERING: this module knows about `adb` and about Android. It knows nothing
//! about LDPlayer, about accounts, and nothing whatsoever about Facebook,
//! Instagram or TikTok. Everything here would work just as well against a
//! phone on a USB cable, and that is the property that keeps the platform
//! connectors from growing their own private ways to talk to a device.
//!
//! Anything platform-specific belongs in `crate::publish::connector`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;

use crate::errors::{AppError, Result};
use crate::process::command;

/// How long a single adb invocation may run before we give up on it.
///
/// `adb push` of a large video is the outlier here, so it gets its own,
/// much longer budget: a 500 MB file over the loopback interface to a cold
/// emulator has been observed to take just over two minutes.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const PUSH_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Which MediaStore collection a file belongs to.
///
/// Generic Android, not platform-specific: `content://media/external/video`
/// and `.../images` are separate tables, and a file indexed into the wrong one
/// is invisible to any picker filtering for the other. Every app's media
/// picker filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaCollection {
    Video,
    Image,
}

impl MediaCollection {
    /// The MediaStore table to query for an indexed file.
    pub fn store_uri(self) -> &'static str {
        match self {
            MediaCollection::Video => "content://media/external/video/media",
            MediaCollection::Image => "content://media/external/images/media",
        }
    }

    /// MIME filter for a share intent. Wildcards, because the receiving app
    /// cares about the family, and naming an exact subtype only risks an app
    /// refusing a container it would otherwise have accepted.
    pub fn mime(self) -> &'static str {
        match self {
            MediaCollection::Video => "video/*",
            MediaCollection::Image => "image/*",
        }
    }

    /// Guess from a file extension. Unknown extensions are treated as video:
    /// this feature is video-first, and a wrong guess degrades to "the picker
    /// doesn't show it" rather than to a failed transfer.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" | "heif" => {
                MediaCollection::Image
            }
            _ => MediaCollection::Video,
        }
    }

    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .map(|e| Self::from_extension(&e.to_string_lossy()))
            .unwrap_or(MediaCollection::Video)
    }
}

/// A device as `adb devices -l` reports it.
#[derive(Debug, Clone, Serialize)]
pub struct AdbDevice {
    /// `emulator-5554` or `127.0.0.1:5555`. The handle every other call uses.
    pub serial: String,
    /// "device" when usable; "offline" and "unauthorized" both mean not yet.
    pub state: String,
}

impl AdbDevice {
    pub fn is_online(&self) -> bool {
        self.state == "device"
    }
}

/// Result of one adb invocation, already decoded and trimmed.
#[derive(Debug, Clone)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// adb's most annoying habit: exiting 0 while printing the failure. Any
    /// caller that cares whether the *device* accepted the command has to look
    /// at the text as well as the code.
    pub fn failed_despite_zero(&self) -> bool {
        let s = format!("{} {}", self.stdout, self.stderr).to_lowercase();
        s.contains("error:")
            || s.contains("failure")
            || s.contains("device not found")
            || s.contains("device offline")
            || s.contains("no such file or directory")
    }
}

/// Handle to one `adb` executable.
///
/// Cheap to clone; holds only the path. Each call spawns a fresh process
/// rather than keeping a connection, which is how adb is designed to be used
/// and what makes a hung emulator recoverable.
#[derive(Debug, Clone)]
pub struct Adb {
    exe: PathBuf,
}

impl Adb {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self { exe: exe.into() }
    }

    pub fn path(&self) -> &Path {
        &self.exe
    }

    /// Find an adb to use, in the order that gives the user the fewest
    /// surprises:
    ///
    /// 1. an explicit path from Settings — always wins, so a broken
    ///    auto-detection can be fixed without a rebuild;
    /// 2. the adb that ships inside LDPlayer — the one guaranteed to speak the
    ///    same protocol version as these emulators;
    /// 3. whatever is on PATH, for people who already have platform-tools.
    ///
    /// Mixing adb versions is not cosmetic: a newer adb on PATH kills the
    /// server LDPlayer started and the instances go `offline` until restarted,
    /// which is why LDPlayer's own binary is preferred over the system one.
    pub fn discover(configured: Option<&Path>, ldplayer_dir: Option<&Path>) -> Option<Self> {
        if let Some(p) = configured {
            if p.is_file() {
                return Some(Self::new(p));
            }
        }
        if let Some(dir) = ldplayer_dir {
            let bundled = dir.join(exe_name("adb"));
            if bundled.is_file() {
                return Some(Self::new(bundled));
            }
        }
        which(exe_name("adb")).map(Self::new)
    }

    async fn run_with(&self, args: &[&str], timeout: Duration) -> Result<Output> {
        let mut cmd = command(&self.exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // The timeout has to wrap the future itself, not its result: an adb
        // command against a wedged emulator never returns, and that is exactly
        // the case this exists for.
        let out = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(r) => r.map_err(|e| AppError::AdbFailed(format!("could not run adb: {e}")))?,
            Err(_) => {
                return Err(AppError::AdbFailed(format!(
                    "adb {} timed out after {}s",
                    args.first().copied().unwrap_or("command"),
                    timeout.as_secs()
                )))
            }
        };

        Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    /// Run adb with no device selected (`adb devices`, `adb connect`, …).
    pub async fn run(&self, args: &[&str]) -> Result<Output> {
        self.run_with(args, DEFAULT_TIMEOUT).await
    }

    /// Run adb against one device. Always `-s <serial>`, never the implicit
    /// "only device" form: with four emulators running, the implicit form is a
    /// coin toss that would publish to the wrong account.
    pub async fn device(&self, serial: &str, args: &[&str]) -> Result<Output> {
        let mut full: Vec<&str> = vec!["-s", serial];
        full.extend_from_slice(args);
        self.run_with(&full, DEFAULT_TIMEOUT).await
    }

    /// `adb shell <cmd>` as a single argument, so quoting stays the caller's
    /// problem exactly once instead of at every layer.
    pub async fn shell(&self, serial: &str, cmd: &str) -> Result<String> {
        let out = self.device(serial, &["shell", cmd]).await?;
        if !out.ok() {
            return Err(AppError::AdbFailed(sanitize(&out.stderr, &out.stdout)));
        }
        Ok(out.stdout)
    }

    /// Start (or reuse) the adb server. Called once before a batch so the
    /// first real command doesn't pay the daemon's startup latency and
    /// mistake it for a dead device.
    pub async fn start_server(&self) -> Result<()> {
        self.run(&["start-server"]).await.map(|_| ())
    }

    pub async fn version(&self) -> Result<String> {
        let out = self.run(&["version"]).await?;
        Ok(out
            .stdout
            .lines()
            .next()
            .unwrap_or("adb")
            .trim()
            .to_string())
    }

    pub async fn devices(&self) -> Result<Vec<AdbDevice>> {
        let out = self.run(&["devices"]).await?;
        if !out.ok() {
            return Err(AppError::AdbFailed(sanitize(&out.stderr, &out.stdout)));
        }
        Ok(parse_devices(&out.stdout))
    }

    /// Attach to a TCP endpoint. Already-connected is success, not an error —
    /// reconnecting is the normal path when the UI refreshes.
    pub async fn connect(&self, endpoint: &str) -> Result<()> {
        let out = self.run(&["connect", endpoint]).await?;
        let text = format!("{} {}", out.stdout, out.stderr).to_lowercase();
        if text.contains("connected to") {
            return Ok(());
        }
        Err(AppError::AdbFailed(format!(
            "could not connect to {endpoint}: {}",
            first_line(&sanitize(&out.stderr, &out.stdout))
        )))
    }

    pub async fn disconnect(&self, endpoint: &str) -> Result<()> {
        self.run(&["disconnect", endpoint]).await.map(|_| ())
    }

    /// Wait until `serial` answers, polling rather than using
    /// `adb wait-for-device` — that blocks forever on an emulator that never
    /// finishes booting, and a hung Publish is worse than a failed one.
    pub async fn wait_for_device(&self, serial: &str, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self
                .devices()
                .await
                .map(|d| d.iter().any(|x| x.serial == serial && x.is_online()))
                .unwrap_or(false)
                && self.is_booted(serial).await
            {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(AppError::InstanceOffline(serial.to_string()));
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    /// True once Android itself — not just the emulator shell — is up.
    ///
    /// `sys.boot_completed` flips well before the launcher is usable, so
    /// `bootanim` is checked too; starting an app during the boot animation
    /// reliably lands on a blank screen.
    pub async fn is_booted(&self, serial: &str) -> bool {
        let booted = self
            .shell(serial, "getprop sys.boot_completed")
            .await
            .map(|s| s.trim() == "1")
            .unwrap_or(false);
        if !booted {
            return false;
        }
        self.shell(serial, "getprop init.svc.bootanim")
            .await
            .map(|s| s.trim() == "stopped")
            .unwrap_or(true)
    }

    /// A human-readable model name, for the device list.
    pub async fn model(&self, serial: &str) -> Option<String> {
        self.shell(serial, "getprop ro.product.model")
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub async fn android_release(&self, serial: &str) -> Option<String> {
        self.shell(serial, "getprop ro.build.version.release")
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Every installed package, for deciding which social app an instance can
    /// actually publish to.
    pub async fn installed_packages(&self, serial: &str) -> Result<Vec<String>> {
        let out = self.shell(serial, "pm list packages").await?;
        Ok(out
            .lines()
            .filter_map(|l| l.trim().strip_prefix("package:"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub async fn is_installed(&self, serial: &str, package: &str) -> Result<bool> {
        Ok(self
            .installed_packages(serial)
            .await?
            .iter()
            .any(|p| p == package))
    }

    /// Copy a local file to the device.
    ///
    /// The long timeout is deliberate — see [`PUSH_TIMEOUT`]. adb prints
    /// progress on stdout, which we do not parse: the useful progress signal
    /// for the UI is per-job, and a push that has started essentially always
    /// finishes.
    pub async fn push(&self, serial: &str, local: &Path, remote: &str) -> Result<()> {
        if !local.is_file() {
            return Err(AppError::MediaFileMissing(
                local.file_name().map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| local.display().to_string()),
            ));
        }
        // Create the destination folder first: adb push onto a missing parent
        // fails with a bare "remote couldn't create file", which tells a user
        // nothing.
        if let Some((dir, _)) = remote.rsplit_once('/') {
            let _ = self.shell(serial, &format!("mkdir -p {}", shell_quote(dir))).await;
        }

        let local_s = local.to_string_lossy().to_string();
        let out = self
            .run_with(&["-s", serial, "push", &local_s, remote], PUSH_TIMEOUT)
            .await?;
        if !out.ok() || out.failed_despite_zero() {
            return Err(AppError::MediaTransferFailed(first_line(&sanitize(
                &out.stderr, &out.stdout,
            ))));
        }
        Ok(())
    }

    /// Byte size of a file on the device, used to verify a push landed whole.
    pub async fn remote_size(&self, serial: &str, remote: &str) -> Option<u64> {
        // `stat -c %s` is present on every Android this app targets; the
        // wc fallback covers the toybox builds where it is not.
        let quoted = shell_quote(remote);
        if let Ok(s) = self.shell(serial, &format!("stat -c %s {quoted}")).await {
            if let Ok(n) = s.trim().parse::<u64>() {
                return Some(n);
            }
        }
        self.shell(serial, &format!("wc -c < {quoted}"))
            .await
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    }

    /// Ask Android's MediaStore to index a freshly pushed file.
    ///
    /// WHY THIS IS NOT OPTIONAL: a file that exists on the filesystem but not
    /// in MediaStore is invisible to every gallery picker, which is exactly
    /// the picker a social app opens. Skipping this step produces the single
    /// most confusing failure in the whole feature — "the video is right
    /// there, but the app can't see it".
    ///
    /// Both mechanisms are tried because neither covers every Android version
    /// LDPlayer ships: the broadcast is the classic path and works through
    /// Android 9 (LDPlayer 9's Android), while `content call … scan_file` is
    /// what still works on 10+. Failures are tolerated individually; only
    /// total failure is reported, and even then the caller may choose to
    /// continue.
    pub async fn scan_media(&self, serial: &str, remote: &str) -> Result<()> {
        let quoted = shell_quote(remote);
        let mut worked = false;

        let broadcast = format!(
            "am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file://{remote}"
        );
        if let Ok(out) = self.shell(serial, &broadcast).await {
            worked |= out.contains("Broadcast completed") || out.contains("result=0");
        }

        let call = format!(
            "content call --uri content://media/external/file --method scan_file --arg {quoted}"
        );
        if let Ok(out) = self.shell(serial, &call).await {
            worked |= !out.to_lowercase().contains("error");
        }

        if worked {
            Ok(())
        } else {
            Err(AppError::MediaScanFailed)
        }
    }

    /// Whether MediaStore has actually indexed the path — the honest check
    /// that `scan_media` succeeded, rather than trusting its exit code.
    pub async fn is_in_media_store(
        &self,
        serial: &str,
        remote: &str,
        collection: MediaCollection,
    ) -> bool {
        self.media_store_id(serial, remote, collection).await.is_some()
    }

    /// MediaStore's row id for a pushed file, if it has been indexed.
    ///
    /// Needed because a `file://` URI handed to another app throws
    /// `FileUriExposedException` on Android 7 and later — every app the
    /// connectors target refuses it. A `content://media/...` URI built from
    /// this id is the supported way to hand a video to another app, and it is
    /// generic Android, not platform-specific, which is why it lives here.
    pub async fn media_store_id(
        &self,
        serial: &str,
        remote: &str,
        collection: MediaCollection,
    ) -> Option<String> {
        let q = format!(
            "content query --uri {} --projection _id --where \"_data='{}'\"",
            collection.store_uri(),
            remote.replace('\'', "")
        );
        let out = self.shell(serial, &q).await.ok()?;
        // Rows print as: Row: 0 _id=42
        out.split("_id=")
            .nth(1)?
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// The `content://` URI for a pushed video, once MediaStore knows it.
    pub async fn media_store_uri(
        &self,
        serial: &str,
        remote: &str,
        collection: MediaCollection,
    ) -> Option<String> {
        self.media_store_id(serial, remote, collection)
            .await
            .map(|id| format!("{}/{id}", collection.store_uri()))
    }

    /// Hand a media item to a specific app through Android's own share
    /// mechanism (`ACTION_SEND`), with read permission granted on the URI.
    ///
    /// Generic on purpose: `ACTION_SEND` is the Android contract every app
    /// implements, so one implementation here serves every connector. What
    /// each app *does* with the share — which composer screen opens, what the
    /// caption field is called — is the connector's business.
    ///
    /// This opens the app's own composer with the user's own session. It does
    /// not submit anything: the person still taps Post.
    pub async fn share_to(
        &self,
        serial: &str,
        package: &str,
        content_uri: &str,
        mime: &str,
        text: Option<&str>,
    ) -> Result<()> {
        // `-p package` restricts the intent to one app and lets Android
        // resolve the right receiver itself. `-n pkg/activity` would need the
        // activity name, which changes with every app release.
        let mut cmd = format!(
            "am start -a android.intent.action.SEND -t {mime} -p {pkg} \
             --grant-read-uri-permission --eu android.intent.extra.STREAM {uri}",
            mime = shell_quote(mime),
            pkg = shell_quote(package),
            uri = shell_quote(content_uri),
        );
        if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
            cmd.push_str(&format!(" --es android.intent.extra.TEXT {}", shell_quote(text)));
        }

        let out = self.shell(serial, &cmd).await?;
        let lower = out.to_lowercase();
        if lower.contains("error") || lower.contains("exception") {
            return Err(AppError::AppLaunchFailed(format!(
                "{package} did not accept the video: {}",
                first_line(&out)
            )));
        }
        Ok(())
    }

    /// Launch an app's main activity. `monkey` is used rather than a hand-built
    /// `am start` because it resolves the launcher activity itself, and those
    /// activity names change between app releases.
    pub async fn launch_app(&self, serial: &str, package: &str) -> Result<()> {
        if !self.is_installed(serial, package).await? {
            return Err(AppError::AppNotInstalled(package.to_string()));
        }
        let cmd = format!(
            "monkey -p {} -c android.intent.category.LAUNCHER 1",
            shell_quote(package)
        );
        let out = self.shell(serial, &cmd).await?;
        if out.contains("No activities found") || out.contains("Error") {
            return Err(AppError::AppLaunchFailed(package.to_string()));
        }
        Ok(())
    }

    pub async fn stop_app(&self, serial: &str, package: &str) -> Result<()> {
        self.shell(serial, &format!("am force-stop {}", shell_quote(package)))
            .await
            .map(|_| ())
    }

    /// Package name of whatever is on screen. Used to confirm an app actually
    /// came to the foreground instead of crashing back to the launcher.
    pub async fn foreground_package(&self, serial: &str) -> Option<String> {
        let out = self
            .shell(serial, "dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'")
            .await
            .ok()?;
        // Focus lines look like: mCurrentFocus=Window{... u0 com.pkg/com.pkg.Act}
        out.split_whitespace()
            .find_map(|tok| tok.split_once('/').map(|(p, _)| p.to_string()))
            .map(|p| p.trim_start_matches('{').to_string())
            .filter(|p| p.contains('.'))
    }

    /// Grab the framebuffer as PNG bytes.
    ///
    /// `exec-out` (not `shell`) matters: plain `adb shell` mangles binary
    /// output with CRLF translation on Windows, producing a corrupt PNG.
    pub async fn screenshot(&self, serial: &str) -> Result<Vec<u8>> {
        let out = command(&self.exe)
            .args(["-s", serial, "exec-out", "screencap", "-p"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| AppError::AdbFailed(format!("screencap failed to start: {e}")))?;

        if !out.status.success() || out.stdout.len() < 8 {
            return Err(AppError::AdbFailed(format!(
                "screencap failed: {}",
                first_line(&String::from_utf8_lossy(&out.stderr))
            )));
        }
        Ok(out.stdout)
    }
}

/// Quote one argument for the device's `sh`. Paths we build contain no quotes,
/// but a user-chosen filename can, and an unquoted one would end the command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Parse the body of `adb devices`. The first line is a banner, and blank
/// lines separate nothing in particular.
fn parse_devices(stdout: &str) -> Vec<AdbDevice> {
    stdout
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .chain(std::iter::empty())
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') {
                return None;
            }
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next().unwrap_or("unknown").to_string();
            Some(AdbDevice { serial, state })
        })
        .collect()
}

/// Strip anything from adb's output that isn't useful to a user, and keep the
/// result short enough to sit in a job's error field.
fn sanitize(stderr: &str, stdout: &str) -> String {
    let raw = if stderr.trim().is_empty() { stdout } else { stderr };
    let cleaned = raw.replace("adb: ", "").replace("error: ", "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "adb reported no detail".to_string()
    } else if cleaned.len() > 300 {
        format!("{}…", &cleaned[..300])
    } else {
        cleaned.to_string()
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).trim().to_string()
}

/// `name.exe` on Windows, `name` elsewhere.
pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Minimal PATH search, so we don't add a crate for one lookup.
pub fn which(name: impl AsRef<str>) -> Option<PathBuf> {
    let name = name.as_ref();
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_mixed_device_list() {
        let out = "List of devices attached\n\
                   emulator-5554\tdevice\n\
                   127.0.0.1:5555\toffline\n\
                   \n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 2);
        assert!(d[0].is_online());
        assert!(!d[1].is_online());
        assert_eq!(d[1].serial, "127.0.0.1:5555");
    }

    #[test]
    fn empty_list_is_not_an_error() {
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn daemon_banner_lines_are_ignored() {
        let out = "* daemon not running; starting now at tcp:5037\n\
                   * daemon started successfully\n\
                   List of devices attached\n\
                   emulator-5554\tdevice\n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].serial, "emulator-5554");
    }

    #[test]
    fn media_collections_are_guessed_from_the_extension() {
        for ext in ["jpg", "JPEG", "png", "gif", "webp", "heic"] {
            assert_eq!(MediaCollection::from_extension(ext), MediaCollection::Image, "{ext}");
        }
        for ext in ["mp4", "mov", "MKV", "webm"] {
            assert_eq!(MediaCollection::from_extension(ext), MediaCollection::Video, "{ext}");
        }
        // Unknown degrades to video, which is the feature's primary case.
        assert_eq!(MediaCollection::from_extension("xyz"), MediaCollection::Video);
    }

    #[test]
    fn each_collection_has_its_own_store_and_mime() {
        assert_ne!(
            MediaCollection::Video.store_uri(),
            MediaCollection::Image.store_uri(),
            "indexing into the wrong table hides the file from every picker"
        );
        assert_eq!(MediaCollection::Image.mime(), "image/*");
    }

    #[test]
    fn shell_quoting_survives_an_apostrophe() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn zero_exit_with_error_text_is_still_a_failure() {
        let o = Output { status: 0, stdout: String::new(), stderr: "error: device offline".into() };
        assert!(o.ok() && o.failed_despite_zero());
    }
}
