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
        // "Refused" means the port is closed, not that the address is wrong.
        // For an emulator that is visibly running, that has one overwhelming
        // cause, and saying so beats printing a winsock error code.
        if is_refusal(&text) {
            return Err(AppError::AdbDebuggingOff(endpoint.to_string()));
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
    ///
    /// The error names which of the three gates failed. A timeout that only
    /// says "not responding" hides the difference between a device adb cannot
    /// see, one it sees as `offline`, and one that is simply still booting —
    /// and those have three different fixes.
    pub async fn wait_for_device(&self, serial: &str, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let listed = self
                .devices()
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|d| d.serial == serial);

            let reason = match &listed {
                None => AppError::InstanceOffline(serial.to_string()),
                Some(d) if !d.is_online() => {
                    AppError::DeviceNotReady(serial.to_string(), d.state.clone())
                }
                Some(_) => {
                    if self.is_booted(serial).await {
                        return Ok(());
                    }
                    AppError::AndroidNotBooted(serial.to_string())
                }
            };

            if std::time::Instant::now() >= deadline {
                return Err(reason);
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    /// True once Android itself — not just the emulator shell — is up.
    ///
    /// `sys.boot_completed` flips slightly before the launcher is usable, so
    /// the boot animation service is consulted as a second opinion — starting
    /// an app mid-animation reliably lands on a blank screen.
    ///
    /// THE TRAP: LDPlayer ships with the boot animation disabled (a large part of
    /// why it starts so fast), so `init.svc.bootanim` comes back
    /// EMPTY, not "stopped". Comparing that to "stopped" marks a
    /// fully-booted emulator as forever-booting. Only an explicit "running"
    /// means the animation is actually playing; absent means there is no such
    /// service to wait for.
    pub async fn is_booted(&self, serial: &str) -> bool {
        // Two properties, because different Android builds set different ones.
        // Either being "1" is proof enough.
        let mut booted = false;
        for prop in ["sys.boot_completed", "dev.bootcomplete"] {
            if let Ok(v) = self.shell(serial, &format!("getprop {prop}")).await {
                if v.trim() == "1" {
                    booted = true;
                    break;
                }
            }
        }
        if !booted {
            return false;
        }

        let bootanim = self
            .shell(serial, "getprop init.svc.bootanim")
            .await
            .unwrap_or_default();
        boot_animation_finished(&bootanim)
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

        // The URI MUST be quoted. A filename with a space otherwise ends the
        // argument early: `-d file:///sdcard/.../2.5M views _ 17K.mp4` reaches
        // `am` as `-d file:///sdcard/.../2.5M` with `views` swallowed as the
        // package argument, so a path that does not exist gets scanned and the
        // real file is never indexed. Downloaded social videos are full of
        // spaces, which is exactly where this bites.
        let broadcast = format!(
            "am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d {}",
            shell_quote(&format!("file://{remote}"))
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
    /// this id is the supported way to hand a video to another app.
    ///
    /// THE TRAP, found on a real device: MediaStore stores the **canonical**
    /// path. Push to `/sdcard/Movies/x.mp4` and the row reads
    /// `/storage/emulated/0/Movies/x.mp4`, because `/sdcard` is a symlink on
    /// every Android. Querying for the path we pushed to therefore never
    /// matches, and a perfectly indexed file looks unindexed.
    ///
    /// So: ask for both spellings at once, then fall back to a suffix match
    /// for any storage layout neither covers.
    pub async fn media_store_id(
        &self,
        serial: &str,
        remote: &str,
        collection: MediaCollection,
    ) -> Option<String> {
        let mut clauses = vec![format!("_data='{}'", sql_escape(remote))];
        if let Some(canonical) = self.canonical_path(serial, remote).await {
            if canonical != remote {
                clauses.push(format!("_data='{}'", sql_escape(&canonical)));
            }
        }
        if let Some(id) = self
            .query_media_id(serial, collection, &clauses.join(" OR "))
            .await
        {
            return Some(id);
        }

        // Last resort: match on the tail of the path. The folder name is ours,
        // so a false positive needs a same-named file in a same-named folder on
        // another volume — far less likely than the alternative, which is
        // reporting a successfully indexed file as missing.
        let suffix = path_suffix(remote);
        self.query_media_id(
            serial,
            collection,
            &format!("_data LIKE '%{}'", sql_escape(&suffix)),
        )
        .await
    }

    /// Run one MediaStore query and pull the first `_id` out of it.
    async fn query_media_id(
        &self,
        serial: &str,
        collection: MediaCollection,
        where_clause: &str,
    ) -> Option<String> {
        let q = format!(
            "content query --uri {} --projection _id --where \"{}\"",
            collection.store_uri(),
            where_clause
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

    /// Resolve a device path through its symlinks, the way MediaStore records
    /// it. `realpath` is present on the Androids this app targets; the shell
    /// fallback covers builds where it is not.
    async fn canonical_path(&self, serial: &str, remote: &str) -> Option<String> {
        let quoted = shell_quote(remote);
        if let Ok(out) = self.shell(serial, &format!("realpath {quoted}")).await {
            let line = out.lines().next().unwrap_or("").trim();
            if line.starts_with('/') {
                return Some(line.to_string());
            }
        }
        // `cd` into the parent and ask where that really is.
        let (dir, file) = remote.rsplit_once('/')?;
        let out = self
            .shell(serial, &format!("cd {} && pwd -P", shell_quote(dir)))
            .await
            .ok()?;
        let real_dir = out.lines().next().unwrap_or("").trim();
        real_dir
            .starts_with('/')
            .then(|| format!("{real_dir}/{file}"))
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

    /// Launch an app's main activity.
    ///
    /// Three strategies, tried in order, because no single one works on every
    /// image. `monkey` used to be the only one: it resolves the launcher
    /// activity itself, which matters because those activity names change with
    /// every app release. But LDPlayer ships builds with `/system/bin/monkey`
    /// stripped — the shell answers "monkey: inaccessible or not found" — so
    /// the resolution is now asked of the package manager first, and monkey is
    /// kept last for images where the others are cut down instead.
    pub async fn launch_app(&self, serial: &str, package: &str) -> Result<()> {
        if !self.is_installed(serial, package).await? {
            return Err(AppError::AppNotInstalled(package.to_string()));
        }

        // Every failure is collected rather than returned, so a launch that
        // exhausts all three says what each one said. Debugging this from a
        // single symptom is what cost us the monkey bug.
        let mut attempts: Vec<String> = Vec::new();

        // 1. Ask which activity the launcher would open, then open exactly it.
        if let Some(component) = self.launcher_activity(serial, package).await {
            match self
                .start_activity(serial, &format!("am start -n {}", shell_quote(&component)))
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => attempts.push(e),
            }
        }

        // 2. The same intent monkey builds, resolved by `am` instead. Covers
        //    images with no `cmd`, and packages whose resolve answers with
        //    Android's fallback handler.
        match self
            .start_activity(
                serial,
                &format!(
                    "am start -a android.intent.action.MAIN \
                     -c android.intent.category.LAUNCHER -p {}",
                    shell_quote(package)
                ),
            )
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => attempts.push(e),
        }

        // 3. The original path, for images where `am start` is restricted.
        match self
            .start_activity(
                serial,
                &format!(
                    "monkey -p {} -c android.intent.category.LAUNCHER 1",
                    shell_quote(package)
                ),
            )
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => attempts.push(e),
        }

        Err(AppError::AppLaunchFailed(format!(
            "{package} ({})",
            attempts.join("; ")
        )))
    }

    /// Launch an app on a clean task, discarding whatever it had open.
    ///
    /// WHY THIS EXISTS: `am start` on a package that already has a task
    /// RESUMES that task. A publish that stopped half way leaves Facebook's
    /// composer on top, so the next launch lands straight back in it — force
    /// stopping does not help, because the task is restored from recents.
    /// Anything that needs the app's own home screen (reading the Page list,
    /// switching profile) gets a wrong screen forever.
    ///
    /// `-S` stops the app first and `--activity-clear-task` drops the task, so
    /// the app starts where a user tapping its icon from cold would.
    ///
    /// THE COST, stated plainly: an unfinished composer is discarded. That is
    /// why this is not what ordinary publishing uses — [`Self::launch_app`]
    /// still resumes — and why callers of this are the ones that are about to
    /// drive the app themselves anyway.
    pub async fn relaunch_app(&self, serial: &str, package: &str) -> Result<()> {
        if !self.is_installed(serial, package).await? {
            return Err(AppError::AppNotInstalled(package.to_string()));
        }

        if let Some(component) = self.launcher_activity(serial, package).await {
            let cmd = format!(
                "am start -S --activity-clear-task -n {}",
                shell_quote(&component)
            );
            if self.start_activity(serial, &cmd).await.is_ok() {
                return Ok(());
            }
        }

        // No resolvable component: fall back to the ordinary launch, which at
        // least opens the app. A resumed composer is better than no app.
        self.launch_app(serial, package).await
    }

    /// The component the launcher would start for `package`, e.g.
    /// `com.facebook.katana/.LoginActivity`.
    ///
    /// Both spellings of the query are sent in one round trip: the explicit
    /// intent form is the documented one, while the bare-package form is what
    /// actually answers on some builds. The first line that names the package
    /// wins, so whichever spelling worked is the one used. The trailing
    /// `true` keeps a good answer from the first form from being thrown away
    /// when the second exits non-zero.
    async fn launcher_activity(&self, serial: &str, package: &str) -> Option<String> {
        let pkg = shell_quote(package);
        let out = self
            .shell(
                serial,
                &format!(
                    "cmd package resolve-activity --brief \
                     -a android.intent.action.MAIN \
                     -c android.intent.category.LAUNCHER -p {pkg} 2>/dev/null; \
                     cmd package resolve-activity --brief {pkg} 2>/dev/null; true"
                ),
            )
            .await
            .ok()?;
        parse_launcher_component(&out, package)
    }

    /// Run one launch command and decide whether the app actually came up.
    ///
    /// Exit status alone is not enough in either direction: `am` prints
    /// "Error: Activity not started" and still exits 0, while a missing
    /// binary fails at the shell before the command runs. Both come back as
    /// the same kind of short reason so the caller can try the next strategy.
    async fn start_activity(&self, serial: &str, cmd: &str) -> std::result::Result<(), String> {
        let out = match self.shell(serial, cmd).await {
            Ok(out) => out,
            Err(e) => return Err(first_line(&e.to_string())),
        };
        if out.contains("No activities found") || out.contains("Error") || out.contains("Exception")
        {
            return Err(first_line(&out));
        }
        Ok(())
    }

    pub async fn stop_app(&self, serial: &str, package: &str) -> Result<()> {
        self.shell(serial, &format!("am force-stop {}", shell_quote(package)))
            .await
            .map(|_| ())
    }

    /// The `package/activity` on screen, for telling one screen of an app from
    /// another. See [`parse_foreground_activity`].
    pub async fn foreground_activity(&self, serial: &str) -> Option<String> {
        let out = self
            .shell(serial, "dumpsys window | grep -E 'mCurrentFocus'")
            .await
            .ok()?;
        parse_foreground_activity(&out)
    }

    /// Package name of whatever is on screen. Used to confirm an app actually
    /// came to the foreground instead of crashing back to the launcher.
    pub async fn foreground_package(&self, serial: &str) -> Option<String> {
        let out = self
            .shell(serial, "dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'")
            .await
            .ok()?;
        parse_foreground_package(&out)
    }

    // ------------------------------------------------------ UI automation
    //
    // Generic Android input. This is the same mechanism Appium drives, and it
    // is here rather than in a connector because tapping a screen is not a
    // platform-specific idea. WHICH button to tap is - that belongs upstairs.

    /// Read the current view hierarchy.
    ///
    /// Dumps to a file and reads it back: `uiautomator dump /dev/tty` prints
    /// only its own confirmation line on the Androids this app targets, which
    /// silently yields an empty hierarchy and an automation that finds nothing.
    pub async fn ui_dump(&self, serial: &str) -> Result<Vec<UiNode>> {
        const REMOTE: &str = "/sdcard/.publisher-ui.xml";
        let mut last = String::from("no output");

        // `--compressed` first. uiautomator refuses to dump until the window
        // goes idle, and an app whose composer animates — a photo preview, a
        // shimmer, a spinner — never does. The compressed hierarchy skips
        // uninteresting nodes and succeeds on screens where the full dump
        // times out. Two attempts each, because the idle window is a moment
        // that comes and goes.
        for attempt in 0..4 {
            let flag = if attempt % 2 == 0 { "--compressed" } else { "" };
            let out = self
                .shell(
                    serial,
                    // Errors go to stdout on purpose: swallowing them is what
                    // made a screen we could not READ look like a screen
                    // showing the wrong thing.
                    &format!("uiautomator dump {flag} {REMOTE} 2>&1; echo '---'; cat {REMOTE} 2>/dev/null"),
                )
                .await?;

            if let Some((status, xml)) = out.split_once("---") {
                if xml.contains("<node") {
                    return Ok(parse_ui_nodes(xml));
                }
                let status = status.trim();
                if !status.is_empty() {
                    last = status.lines().last().unwrap_or(status).to_string();
                }
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }

        Err(AppError::UiDumpFailed(sanitize(&last, "")))
    }

    /// Poll the hierarchy until one of `any_of` appears.
    ///
    /// Returns the node found and which matcher found it, so a caller waiting
    /// on several alternatives (an app that says "Post" or "Share" or "Next"
    /// depending on version) knows which it got.
    /// `Ok(Some)` found it, `Ok(None)` the screen was readable but had no such
    /// control, `Err` the screen could not be read at all.
    ///
    /// That third case has to be separate. Collapsing it into "not found" is
    /// what made an unreadable screen report as the wrong screen — sending
    /// someone to check an app that was showing exactly the right thing.
    pub async fn wait_for_node(
        &self,
        serial: &str,
        any_of: &[Match],
        timeout: Duration,
    ) -> Result<Option<UiNode>> {
        let deadline = std::time::Instant::now() + timeout;
        let mut last_err = None;
        loop {
            match self.ui_dump(serial).await {
                Ok(nodes) => {
                    for m in any_of {
                        if let Some(n) = find_node(&nodes, m, false) {
                            return Ok(Some(n));
                        }
                    }
                    last_err = None;
                }
                Err(e) => last_err = Some(e),
            }
            if std::time::Instant::now() >= deadline {
                return match last_err {
                    Some(e) => Err(e),
                    None => Ok(None),
                };
            }
            tokio::time::sleep(Duration::from_millis(900)).await;
        }
    }

    pub async fn tap(&self, serial: &str, x: i32, y: i32) -> Result<()> {
        self.shell(serial, &format!("input tap {x} {y}")).await.map(|_| ())
    }

    /// Tap a node's centre.
    pub async fn tap_node(&self, serial: &str, node: &UiNode) -> Result<()> {
        let (x, y) = node.center();
        self.tap(serial, x, y).await
    }

    /// Drag from one point to another over `ms`.
    ///
    /// Generic on purpose: this layer knows how to move a finger, not what is
    /// being scrolled. A duration long enough to read as a drag rather than a
    /// fling matters — a fling lands somewhere the caller cannot predict, and
    /// a list that scrolled too far has skipped rows nobody will see.
    pub async fn swipe(
        &self,
        serial: &str,
        from: (i32, i32),
        to: (i32, i32),
        ms: u32,
    ) -> Result<()> {
        let ((x1, y1), (x2, y2)) = (from, to);
        self.shell(serial, &format!("input swipe {x1} {y1} {x2} {y2} {ms}"))
            .await
            .map(|_| ())
    }

    pub async fn press_back(&self, serial: &str) -> Result<()> {
        self.shell(serial, "input keyevent 4").await.map(|_| ())
    }

    /// Type into whatever field has focus.
    ///
    /// LIMITATION, and it is a real one: `input text` speaks ASCII only. A
    /// Khmer, Thai or emoji caption cannot be typed this way at all — Android
    /// needs a custom IME for that, which would mean installing an APK into
    /// the user's emulator, so this refuses instead of typing mojibake.
    /// [`can_type`] lets the caller check first and tell the user plainly.
    pub async fn type_text(&self, serial: &str, text: &str) -> Result<()> {
        if !can_type(text) {
            return Err(AppError::CaptionNotTypeable);
        }
        // `input text` takes spaces as %s, and treats several characters as
        // syntax. Chunked because very long arguments get truncated.
        for chunk in chunk_text(text, 200) {
            let escaped = escape_input_text(&chunk);
            self.shell(serial, &format!("input text {}", shell_quote(&escaped)))
                .await?;
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        Ok(())
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

/// Pull the package name out of a `dumpsys window` focus line.
///
/// Lines look like:
/// `mCurrentFocus=Window{2983e33 u0 com.pkg/com.pkg.SomeActivity}`
///
/// The first token containing a `/` is the `package/activity` pair; the window
/// id before it never contains one.
fn parse_foreground_package(out: &str) -> Option<String> {
    out.split_whitespace()
        .find_map(|tok| tok.split_once('/').map(|(p, _)| p.to_string()))
        .map(|p| p.trim_start_matches('{').to_string())
        .filter(|p| p.contains('.'))
}

/// The full `package/activity` on screen, e.g.
/// `com.facebook.katana/com.facebook.composer.activity.ComposerActivity`.
///
/// Worth having separately from [`parse_foreground_package`]: WHICH screen of
/// an app is in front cannot be answered by labels alone. Facebook's feed
/// carries "What's on your mind?" in its status box, exactly like its
/// composer does, so a label test says "a post is open" while the user is
/// looking at their feed. The activity name does not have that problem.
fn parse_foreground_activity(out: &str) -> Option<String> {
    out.split_whitespace()
        .find(|tok| tok.contains('/'))
        .map(|tok| tok.trim_start_matches('{').trim_end_matches('}').to_string())
        .filter(|c| c.contains('.'))
}

/// Escape a value for a SQL string literal in a `content query --where`.
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// The last two path components, for a suffix match: specific enough to
/// identify the file, short enough not to depend on the storage root.
fn path_suffix(remote: &str) -> String {
    let tail: Vec<&str> = remote.rsplit('/').take(2).collect();
    format!("/{}", tail.into_iter().rev().collect::<Vec<_>>().join("/"))
}

/// Whether the boot animation is done — or was never running.
///
/// Anything other than an explicit "running" counts as finished. An empty or
/// absent property means the device has no boot animation service at all,
/// which is the normal state on LDPlayer and must not be read as "still
/// booting".
fn boot_animation_finished(value: &str) -> bool {
    !value.trim().eq_ignore_ascii_case("running")
}

/// Whether adb's failure text describes a closed port rather than a bad
/// address. `10061` is winsock's WSAECONNREFUSED, which is how this presents
/// on Windows; the wording differs per platform, so both are matched.
fn is_refusal(lower: &str) -> bool {
    lower.contains("refused")
        || lower.contains("10061")
        || lower.contains("cannot connect to")
        || lower.contains("connection reset")
}

/// Whether a string looks like an adb serial at all.
///
/// `ldconsole` prints adb's own errors on stdout, so "error: no devices" would
/// otherwise be adopted as a serial and every later `adb -s error:` command
/// would fail for a reason nobody could trace back to here.
pub fn looks_like_serial(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 64 || s.contains(char::is_whitespace) {
        return false;
    }
    let lower = s.to_lowercase();
    if lower.contains("error") || lower.contains("unknown") || lower.contains("daemon") {
        return false;
    }
    // Either `emulator-5554` or `host:port`; both are what LDPlayer produces.
    s.starts_with("emulator-")
        || s.rsplit_once(':').is_some_and(|(h, p)| {
            !h.is_empty() && p.parse::<u16>().is_ok()
        })
        || s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One element of the on-screen view hierarchy, as `uiautomator dump` reports it.
///
/// Generic Android: this is the same hierarchy Appium and Espresso read. It
/// carries no knowledge of any particular app.
#[derive(Debug, Clone, PartialEq)]
pub struct UiNode {
    pub text: String,
    pub resource_id: String,
    pub content_desc: String,
    pub class: String,
    pub clickable: bool,
    pub enabled: bool,
    /// Screen rectangle: (left, top, right, bottom).
    pub bounds: (i32, i32, i32, i32),
}

impl UiNode {
    /// Point to tap: the centre of the node.
    pub fn center(&self) -> (i32, i32) {
        let (l, t, r, b) = self.bounds;
        ((l + r) / 2, (t + b) / 2)
    }

    /// A zero-area node cannot be tapped, and matching one would make the
    /// automation tap the corner of the screen.
    pub fn is_tappable(&self) -> bool {
        let (l, t, r, b) = self.bounds;
        self.enabled && r > l && b > t
    }
}

/// How to recognise a node.
#[derive(Debug, Clone)]
pub enum Match {
    /// Exact visible label, case-insensitive.
    Text(String),
    /// Visible label contains this, case-insensitive.
    TextContains(String),
    /// Accessibility label, case-insensitive exact.
    Desc(String),
    /// Accessibility label contains this, case-insensitive.
    ///
    /// For labels that carry state in them: Facebook's profile switcher is
    /// "Open profile switcher" until a Page has a notification, and then it is
    /// "Open profile switcher, you have notifications". An exact match finds
    /// it on a quiet account and loses it on a busy one.
    DescContains(String),
    /// The tail of a `resource-id`, e.g. `id/post_button`. Matched on the
    /// suffix so the package prefix does not have to be hard-coded.
    ResourceId(String),
    /// Any node of this widget class, e.g. `android.widget.EditText`.
    Class(String),
}

impl Match {
    /// Whether one node satisfies this matcher.
    ///
    /// Public because recognising a screen is not always about tapping it:
    /// deciding which composer an app opened is a read, and going through
    /// [`find_node`] for that would drag in a tappability rule that has
    /// nothing to do with the question.
    pub fn matches(&self, n: &UiNode) -> bool {
        match self {
            Match::Text(v) => n.text.eq_ignore_ascii_case(v),
            Match::TextContains(v) => n.text.to_lowercase().contains(&v.to_lowercase()),
            Match::Desc(v) => n.content_desc.eq_ignore_ascii_case(v),
            Match::DescContains(v) => n.content_desc.to_lowercase().contains(&v.to_lowercase()),
            Match::ResourceId(v) => n.resource_id.ends_with(v.as_str()),
            Match::Class(v) => n.class == *v,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Match::Text(v) | Match::TextContains(v) => format!("\u{201c}{v}\u{201d}"),
            Match::Desc(v) | Match::DescContains(v) => format!("the {v} control"),
            Match::ResourceId(v) => format!("`{v}`"),
            Match::Class(v) => format!("a {v}"),
        }
    }
}

/// Find the first node satisfying a matcher.
///
/// `clickable_only` matters: apps routinely put the visible label in a
/// non-clickable `TextView` inside a clickable container. Tapping the label's
/// centre usually still works, so this prefers a clickable node and falls back
/// to any tappable one rather than giving up.
pub fn find_node(nodes: &[UiNode], m: &Match, clickable_only: bool) -> Option<UiNode> {
    let candidates = nodes.iter().filter(|n| n.is_tappable() && m.matches(n));
    let mut fallback = None;
    for n in candidates {
        if n.clickable {
            return Some(n.clone());
        }
        if fallback.is_none() {
            fallback = Some(n.clone());
        }
    }
    if clickable_only {
        fallback.filter(|_| true)
    } else {
        fallback
    }
}

/// Parse a `uiautomator dump` document into a flat node list.
///
/// A hand-rolled scan rather than an XML crate: the format is one self-closing
/// tag shape with fixed attributes, the documents run to hundreds of KB, and
/// adding a parser dependency for `attr="value"` is not worth it. Unknown or
/// malformed nodes are skipped, never fatal — a partial hierarchy still lets
/// the automation find its button.
pub fn parse_ui_nodes(xml: &str) -> Vec<UiNode> {
    let mut out = Vec::new();
    for chunk in xml.split("<node ").skip(1) {
        let head = chunk.split('>').next().unwrap_or("");
        let Some(bounds) = attr(head, "bounds").and_then(|b| parse_bounds(&b)) else {
            continue;
        };
        out.push(UiNode {
            text: attr(head, "text").unwrap_or_default(),
            resource_id: attr(head, "resource-id").unwrap_or_default(),
            content_desc: attr(head, "content-desc").unwrap_or_default(),
            class: attr(head, "class").unwrap_or_default(),
            clickable: attr(head, "clickable").as_deref() == Some("true"),
            enabled: attr(head, "enabled").as_deref() != Some("false"),
            bounds,
        });
    }
    out
}

/// Read one `name="value"` attribute out of a tag head.
fn attr(head: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = head.find(&key)? + key.len();
    let rest = &head[start..];
    let end = rest.find('"')?;
    Some(unescape_xml(&rest[..end]))
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// `[0,66][1080,2148]` -> (0, 66, 1080, 2148).
fn parse_bounds(raw: &str) -> Option<(i32, i32, i32, i32)> {
    let nums: Vec<i32> = raw
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (nums.len() == 4).then(|| (nums[0], nums[1], nums[2], nums[3]))
}

/// Whether `input text` can carry this string at all — see [`Adb::type_text`].
pub fn can_type(text: &str) -> bool {
    text.chars().all(|c| c.is_ascii() && (c == '\n' || !c.is_control()))
}

/// `input text` reads a space as an argument separator and `%s` as a space;
/// several other characters are shell/`input` syntax.
fn escape_input_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%s"),
            '\n' => out.push_str("%s"),
            '(' | ')' | '<' | '>' | '|' | ';' | '&' | '*' | '\\' | '~' | '"' | '`' | '$' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Split into pieces small enough for one `input text` call.
fn chunk_text(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if cur.len() >= max {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Pick the component out of `cmd package resolve-activity --brief`.
///
/// The brief output still carries a `priority=… isDefault=true` line ahead of
/// the component, and an intent nothing handles resolves to Android's own
/// fallback (`com.android.fallback/.Fallback`). Accepting only a bare line
/// under the package we asked about rejects both, and rejecting is the right
/// answer: the caller then falls through to a strategy that does not need the
/// activity name at all.
fn parse_launcher_component(out: &str, package: &str) -> Option<String> {
    let prefix = format!("{package}/");
    out.lines()
        .map(str::trim)
        .find(|line| line.starts_with(&prefix) && !line.contains(char::is_whitespace))
        .map(str::to_string)
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

    /// The bug this test exists for: LDPlayer disables the boot animation, so
    /// the property is empty. Reading that as "still booting" makes every
    /// Connect time out against a perfectly healthy emulator.
    /// The bug this exists for: MediaStore records
    /// `/storage/emulated/0/...` for a file pushed to `/sdcard/...`, so an
    /// exact query on the pushed path finds nothing. The suffix is what both
    /// spellings share.
    #[test]
    fn a_path_suffix_is_independent_of_the_storage_root() {
        assert_eq!(
            path_suffix("/sdcard/Movies/SocialPublisher/clip.mp4"),
            "/SocialPublisher/clip.mp4"
        );
        assert_eq!(
            path_suffix("/storage/emulated/0/Movies/SocialPublisher/clip.mp4"),
            "/SocialPublisher/clip.mp4",
            "both spellings of the same file must share a suffix"
        );
    }

    /// Captured verbatim from a real Android 9 emulator. The second case is
    /// what a not-signed-in app actually produces — Play Services' add-account
    /// screen — and reading that as "the app is ready" would send someone to a
    /// composer that never opened.
    #[test]
    fn the_foreground_package_is_read_from_a_real_focus_line() {
        assert_eq!(
            parse_foreground_package(
                "  mCurrentFocus=Window{2983e33 u0 com.google.android.youtube/com.google.android.youtube.HomeActivity}"
            )
            .as_deref(),
            Some("com.google.android.youtube")
        );
        assert_eq!(
            parse_foreground_package(
                "  mCurrentFocus=Window{2983e33 u0 com.google.android.gms/com.google.android.gms.auth.uiflows.addaccount.AccountAddedActivity}"
            )
            .as_deref(),
            Some("com.google.android.gms"),
            "a login prompt is not the app we asked for"
        );
        assert_eq!(parse_foreground_package("  mCurrentFocus=null"), None);
        assert_eq!(parse_foreground_package(""), None);
    }

    #[test]
    fn sql_literals_are_escaped() {
        assert_eq!(sql_escape("it's.mp4"), "it''s.mp4");
        assert_eq!(sql_escape("plain.mp4"), "plain.mp4");
    }

    #[test]
    fn an_absent_boot_animation_counts_as_finished() {
        assert!(boot_animation_finished(""), "LDPlayer reports no bootanim at all");
        assert!(boot_animation_finished("   "));
        assert!(boot_animation_finished("stopped"));
        assert!(boot_animation_finished("restarting"));
        assert!(!boot_animation_finished("running"));
        assert!(!boot_animation_finished("RUNNING"));
    }

    #[test]
    fn adb_error_text_is_not_mistaken_for_a_serial() {
        for bad in [
            "error: no devices/emulators found",
            "* daemon not running",
            "unknown",
            "",
            "   ",
            "cannot connect to 127.0.0.1:5555",
        ] {
            assert!(!looks_like_serial(bad), "{bad:?} should be refused");
        }
        for good in ["emulator-5554", "127.0.0.1:5555", "ABCD1234"] {
            assert!(looks_like_serial(good), "{good:?} should be accepted");
        }
    }

    #[test]
    fn a_refused_connection_is_recognised_on_every_platform() {
        assert!(is_refusal("cannot connect to 127.0.0.1:5555: no connection could be made because the target machine actively refused it. (10061)"));
        assert!(is_refusal("failed to connect to '127.0.0.1:5555': connection refused"));
        assert!(!is_refusal("connected to 127.0.0.1:5555"));
    }

    /// A real filename from a downloaded social video. Unquoted, `am` reads
    /// only up to the first space and treats the next word as the package.
    /// Captured verbatim from a real Android 9 emulator.
    #[test]
    fn a_real_ui_dump_is_parsed() {
        let xml = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.android.settings" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2148]"><node index="0" text="Post" resource-id="com.facebook.katana:id/post_button" class="android.widget.Button" package="com.facebook.katana" content-desc="Post" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[880,120][1040,200]"/></node></hierarchy>"#;

        let nodes = parse_ui_nodes(xml);
        assert_eq!(nodes.len(), 2);

        let button = find_node(&nodes, &Match::Text("post".into()), true).unwrap();
        assert!(button.clickable);
        assert_eq!(button.center(), (960, 160));
        assert_eq!(
            find_node(&nodes, &Match::ResourceId("id/post_button".into()), true).unwrap(),
            button
        );
        assert_eq!(find_node(&nodes, &Match::Desc("Post".into()), true).unwrap(), button);
        assert!(find_node(&nodes, &Match::Text("Publish".into()), true).is_none());
    }

    #[test]
    fn a_zero_area_node_is_never_tapped() {
        let xml = r#"<node text="Post" resource-id="" class="x" content-desc="" clickable="true" enabled="true" bounds="[0,0][0,0]"/>"#;
        let nodes = parse_ui_nodes(xml);
        assert_eq!(nodes.len(), 1);
        assert!(!nodes[0].is_tappable(), "an invisible node would tap the screen corner");
        assert!(find_node(&nodes, &Match::Text("Post".into()), true).is_none());
    }

    #[test]
    fn xml_entities_in_labels_are_decoded() {
        let xml = r#"<node text="Tom &amp; Jerry" resource-id="" class="x" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#;
        assert_eq!(parse_ui_nodes(xml)[0].text, "Tom & Jerry");
    }

    #[test]
    fn only_ascii_captions_can_be_typed() {
        assert!(can_type("Check out my new video!"));
        assert!(!can_type("សូមស្វាគមន៍"), "Khmer needs a custom IME, not input text");
        assert!(!can_type("nice 🔥"), "emoji cannot go through input text");
    }

    #[test]
    fn input_text_escaping_handles_spaces_and_syntax() {
        assert_eq!(escape_input_text("a b"), "a%sb");
        assert_eq!(escape_input_text("hi (you)"), r"hi%s\(you\)");
        assert_eq!(escape_input_text("a\nb"), "a%sb");
    }

    #[test]
    fn a_scan_uri_with_spaces_stays_one_argument() {
        let remote = "/sdcard/Movies/SocialPublisher/2.5M views _ One Hit_.mp4";
        let arg = shell_quote(&format!("file://{remote}"));
        assert!(arg.starts_with('\'') && arg.ends_with('\''), "must be quoted: {arg}");
        assert!(
            arg.contains("2.5M views _ One Hit_.mp4"),
            "the whole path must survive: {arg}"
        );
    }

    /// The bug this exists for: telling one screen of an app from another was
    /// being done with labels, and Facebook's FEED carries "What's on your
    /// mind?" in its status box exactly like its composer does. Reading Pages
    /// then refused to start, reporting an unfinished post, on an account
    /// whose Facebook was sitting idle on the feed. The activity name is not
    /// ambiguous the way a label is.
    #[test]
    fn the_screen_in_front_is_identified_by_activity_not_by_label() {
        let composer = "mCurrentFocus=Window{76f4cd7 u0                         com.facebook.katana/com.facebook.composer.activity.ComposerActivity}";
        let feed = "mCurrentFocus=Window{aac95ae u0                     com.facebook.katana/com.facebook.katana.LoginActivity}";

        let seen = parse_foreground_activity(composer).unwrap();
        assert!(seen.to_lowercase().contains("composer"), "{seen}");

        let seen = parse_foreground_activity(feed).unwrap();
        assert!(!seen.to_lowercase().contains("composer"), "the feed is not a composer");

        // Both still name the same app, so the package read is unaffected.
        assert_eq!(
            parse_foreground_package(composer).as_deref(),
            Some("com.facebook.katana")
        );

        // A dialog window carries no component at all — captured from a real
        // "More options" sheet. Guessing a screen from that is worse than
        // admitting we do not know.
        assert_eq!(parse_foreground_activity("mCurrentFocus=Window{1ca9ce3 u0 More options}"), None);
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

    /// The bug this exists for: LDPlayer images ship without
    /// `/system/bin/monkey`, so the only launch path we had died at the shell
    /// with "monkey: inaccessible or not found". Resolving the activity is
    /// what replaced it, and the resolve output is not just the component.
    #[test]
    fn the_launcher_component_is_read_past_the_priority_line() {
        let out = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\n\
                   com.facebook.katana/.LoginActivity\n";
        assert_eq!(
            parse_launcher_component(out, "com.facebook.katana").as_deref(),
            Some("com.facebook.katana/.LoginActivity")
        );
    }

    #[test]
    fn androids_fallback_handler_is_not_mistaken_for_the_app() {
        // Resolving an intent nothing handles answers with this, and starting
        // it would open a "no app can do that" dialog instead of failing over.
        let out = "priority=0 preferredOrder=0\ncom.android.fallback/.Fallback\n";
        assert_eq!(parse_launcher_component(out, "com.facebook.katana"), None);
    }

    #[test]
    fn no_resolution_is_no_component() {
        assert_eq!(parse_launcher_component("No activity found\n", "com.facebook.katana"), None);
        assert_eq!(parse_launcher_component("", "com.facebook.katana"), None);
    }
}
