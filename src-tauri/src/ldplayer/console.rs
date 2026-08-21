//! Wrapper around LDPlayer's own command-line tool, `ldconsole.exe`.
//!
//! LAYERING: this module knows about LDPlayer and nothing else. It does not
//! push files, does not know what a social app is, and deliberately does not
//! duplicate anything in [`crate::ldplayer::adb`] — the one place the two meet
//! is [`Self::serial`], which asks LDPlayer what adb serial an instance owns.
//!
//! PLATFORM: LDPlayer is Windows-only. On macOS and Linux every call here
//! reports [`AppError::LdPlayerMissing`], and the app falls back to plain ADB
//! against whatever emulator or device is attached — which is what makes this
//! feature developable on a Mac even though it ships for Windows.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;

use crate::errors::{AppError, Result};
use crate::ldplayer::adb::exe_name;
use crate::process::command;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Where LDPlayer installs itself when nobody changes the defaults. Checked in
/// newest-first order so a machine with both 9 and 4 gets the modern one.
/// Where LDPlayer installs itself when nobody changes the defaults.
///
/// The registry is asked first and is authoritative; this list only catches
/// installs the registry lost track of (a repaired install, a folder moved by
/// hand). Ordered newest-first so a machine with both 9 and 4 gets the modern
/// one.
const COMMON_SUBDIRS: &[&str] = &[
    r"LDPlayer\LDPlayer9",
    r"LDPlayer9",
    r"Program Files\LDPlayer\LDPlayer9",
    r"Program Files (x86)\LDPlayer\LDPlayer9",
    r"LDPlayer\LDPlayer64",
    r"LDPlayer\LDPlayer4.0",
    r"LDPlayer4.0",
    r"Program Files\LDPlayer\LDPlayer4.0",
    r"ChangZhi\dnplayer2",
    r"LDPlayer\dnplayer2",
    r"dnplayer2",
];

/// Drives to try each subdirectory under.
///
/// People routinely install emulators on a second drive precisely because they
/// are large, so checking only C: is why "installed but not found" happens.
const DRIVES: &[&str] = &["C:", "D:", "E:", "F:"];

/// The console binary's name changed between major versions; both are still in
/// the wild, and an installation only ever has one of them.
const CONSOLE_NAMES: &[&str] = &["ldconsole", "dnconsole"];

/// One emulator instance, exactly as `list2` describes it.
#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    /// LDPlayer's own index. Stable for the life of an instance and the handle
    /// every other console call takes.
    pub index: u32,
    /// The name shown in LDPlayer's Multi-Instance Manager.
    pub name: String,
    /// True when Android inside the instance has started.
    pub running: bool,
    /// Player process id, when running. Diagnostics only.
    pub pid: Option<u32>,
}

/// Handle to one LDPlayer installation.
#[derive(Debug, Clone)]
pub struct LdConsole {
    exe: PathBuf,
    dir: PathBuf,
}

impl LdConsole {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        let exe = exe.into();
        let dir = exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self { exe, dir }
    }

    pub fn path(&self) -> &Path {
        &self.exe
    }

    /// The install directory — where LDPlayer's bundled `adb.exe` also lives.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Locate an installation: the configured path first (a user who moved
    /// LDPlayer must be able to say so), then the registry, then the usual
    /// directories, then PATH.
    pub fn discover(configured: Option<&Path>) -> Option<Self> {
        if let Some(p) = configured {
            // Accept either the console binary itself or the folder holding it,
            // because both are things a person reasonably pastes into a path box.
            if p.is_file() {
                return Some(Self::new(p));
            }
            if p.is_dir() {
                if let Some(found) = console_in(p) {
                    return Some(Self::new(found));
                }
            }
        }
        if !cfg!(windows) {
            return None;
        }
        for dir in candidate_dirs() {
            if let Some(found) = console_in(&dir) {
                return Some(Self::new(found));
            }
        }
        CONSOLE_NAMES
            .iter()
            .find_map(|n| crate::ldplayer::adb::which(exe_name(n)))
            .map(Self::new)
    }

    async fn run(&self, args: &[&str]) -> Result<String> {
        let out = tokio::time::timeout(
            TIMEOUT,
            command(&self.exe)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| AppError::LdPlayerFailed("ldconsole timed out".into()))?
        .map_err(|e| AppError::LdPlayerFailed(format!("could not run ldconsole: {e}")))?;

        // ldconsole writes in the system code page, not UTF-8. Instance names
        // are usually ASCII, so lossy decoding is right here: a mangled
        // character in a name is survivable, refusing to list instances is not.
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(AppError::LdPlayerFailed(if stderr.is_empty() {
                format!("ldconsole {} failed", args.first().copied().unwrap_or(""))
            } else {
                stderr
            }));
        }
        Ok(stdout)
    }

    /// Every instance LDPlayer knows about, running or not.
    pub async fn list(&self) -> Result<Vec<Instance>> {
        let out = self.run(&["list2"]).await?;
        Ok(parse_list2(&out))
    }

    pub async fn get(&self, index: u32) -> Result<Instance> {
        self.list()
            .await?
            .into_iter()
            .find(|i| i.index == index)
            .ok_or_else(|| AppError::InstanceNotFound(index.to_string()))
    }

    /// Boot an instance. Returns as soon as LDPlayer accepts the request —
    /// Android takes another 20–60 seconds, which the caller waits for through
    /// [`crate::ldplayer::adb::Adb::wait_for_device`].
    pub async fn launch(&self, index: u32) -> Result<()> {
        self.run(&["launch", "--index", &index.to_string()])
            .await
            .map(|_| ())
    }

    /// Shut an instance down cleanly. The social app keeps its session; this is
    /// an ordinary Android shutdown, not a data wipe.
    pub async fn quit(&self, index: u32) -> Result<()> {
        self.run(&["quit", "--index", &index.to_string()])
            .await
            .map(|_| ())
    }

    pub async fn is_running(&self, index: u32) -> Result<bool> {
        let out = self
            .run(&["isrunning", "--index", &index.to_string()])
            .await?;
        Ok(out.trim().eq_ignore_ascii_case("running"))
    }

    /// Ask LDPlayer which adb serial an instance answers on.
    ///
    /// WHY NOT COMPUTE IT: the folklore formula (`5555 + index * 2`) is right
    /// only on a machine where instances were never deleted and re-created.
    /// Getting it wrong doesn't fail loudly — it publishes to somebody else's
    /// account. Asking LDPlayer costs one process and cannot be wrong.
    /// [`crate::ldplayer::manager`] falls back to the formula only when this
    /// call fails outright.
    pub async fn serial(&self, index: u32) -> Result<String> {
        let out = self
            .run(&[
                "adb",
                "--index",
                &index.to_string(),
                "--command",
                "get-serialno",
            ])
            .await?;
        // Validate rather than take the first non-empty line: ldconsole prints
        // adb's own errors here, and adopting "error: no devices/emulators
        // found" as a serial makes every later command fail for a reason
        // nobody could trace back to this function.
        let serial = out
            .lines()
            .map(str::trim)
            .find(|l| crate::ldplayer::adb::looks_like_serial(l))
            .ok_or_else(|| AppError::InstanceOffline(index.to_string()))?;
        Ok(serial.to_string())
    }
}

/// The adb endpoint LDPlayer conventionally assigns to an instance. Fallback
/// only — see [`LdConsole::serial`] for why it is not the primary path.
pub fn conventional_endpoint(index: u32) -> String {
    format!("127.0.0.1:{}", 5555 + index * 2)
}

fn console_in(dir: &Path) -> Option<PathBuf> {
    CONSOLE_NAMES
        .iter()
        .map(|n| dir.join(exe_name(n)))
        .find(|p| p.is_file())
}

/// Every folder detection will look in, in order.
///
/// Public so the Settings page can show it. "We looked in these twelve places
/// and found nothing" is an actionable message; "not found" is not, and it is
/// what turns a five-minute fix into a support conversation.
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = registry_dirs().into_iter().map(PathBuf::from).collect();
    for drive in DRIVES {
        for sub in COMMON_SUBDIRS {
            dirs.push(PathBuf::from(format!(r"{drive}\{sub}")));
        }
    }
    dirs.dedup();
    dirs
}

/// Install directories recorded by LDPlayer's installer.
///
/// Three sources, because no single one is reliable across versions: the
/// vendor key (present on a clean install), the same key per-user (present
/// when installed without admin rights), and Windows' own uninstall entry
/// (present whenever the installer registered at all, and the one that
/// survives when the vendor key does not).
///
/// Shelling out to `reg` rather than adding a registry crate: a few reads, on
/// one platform, and `reg.exe` is on every Windows.
#[cfg(windows)]
fn registry_dirs() -> Vec<String> {
    /// (key, value name)
    const SOURCES: &[(&str, &str)] = &[
        (r"HKLM\SOFTWARE\leidian\LDPlayer9", "InstallDir"),
        (r"HKLM\SOFTWARE\WOW6432Node\leidian\LDPlayer9", "InstallDir"),
        (r"HKLM\SOFTWARE\leidian\LDPlayer", "InstallDir"),
        (r"HKLM\SOFTWARE\WOW6432Node\leidian\LDPlayer", "InstallDir"),
        (r"HKLM\SOFTWARE\XuanZhi\LDPlayer", "InstallDir"),
        (r"HKCU\SOFTWARE\leidian\LDPlayer9", "InstallDir"),
        (r"HKCU\SOFTWARE\leidian\LDPlayer", "InstallDir"),
        (
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\LDPlayer9",
            "InstallLocation",
        ),
        (
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\LDPlayer9",
            "InstallLocation",
        ),
        (
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\LDPlayer",
            "InstallLocation",
        ),
    ];

    let mut out = Vec::new();
    for (key, value) in SOURCES {
        let mut cmd = std::process::Command::new("reg");
        cmd.args(["query", key, "/v", value]).stdin(Stdio::null());
        hide_console(&mut cmd);
        let Ok(res) = cmd.output() else { continue };

        let text = String::from_utf8_lossy(&res.stdout);
        for path in parse_reg_values(&text, value) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// Pull the data out of `reg query` output lines like:
///
/// ```text
///     InstallDir    REG_SZ    C:\LDPlayer\LDPlayer9\
/// ```
#[cfg(windows)]
fn parse_reg_values(text: &str, value_name: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(value_name)?;
            let (_, data) = rest.split_once("REG_SZ")?;
            let data = data.trim().trim_end_matches('\\');
            (!data.is_empty()).then(|| data.to_string())
        })
        .collect()
}

/// Keep `reg query` from flashing a console window at startup.
#[cfg(windows)]
fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn registry_dirs() -> Vec<String> {
    Vec::new()
}

/// Parse `ldconsole list2`, whose CSV is:
///
/// ```text
/// index,name,topWindowHandle,bindWindowHandle,androidStarted,pid,vboxPid[,width,height,dpi]
/// ```
///
/// Field count varies by version, so only the first six are read and the rest
/// ignored rather than validated — a future LDPlayer adding a column must not
/// make the instance list go blank.
fn parse_list2(stdout: &str) -> Vec<Instance> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 6 {
                return None;
            }
            let index = f[0].trim().parse::<u32>().ok()?;
            let pid = f[5].trim().parse::<u32>().ok().filter(|p| *p != u32::MAX);
            Some(Instance {
                index,
                name: f[1].trim().to_string(),
                // "1" means Android has started. Anything else — including
                // LDPlayer's "-1 means no" convention — is not running.
                running: f[4].trim() == "1",
                pid,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list2_with_a_running_and_a_stopped_instance() {
        let out = "0,LDPlayer,67144,67146,1,10992,11036\n\
                   1,LDPlayer-1,0,0,0,-1,-1\n";
        let list = parse_list2(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "LDPlayer");
        assert!(list[0].running);
        assert_eq!(list[0].pid, Some(10992));
        assert!(!list[1].running);
        assert_eq!(list[1].pid, None);
    }

    #[test]
    fn tolerates_extra_columns_from_newer_builds() {
        let out = "2,Instagram Box,0,0,1,4242,4243,900,1600,320\n";
        let list = parse_list2(out);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, 2);
        assert_eq!(list[0].name, "Instagram Box");
        assert!(list[0].running);
    }

    #[test]
    fn ignores_junk_lines() {
        assert!(parse_list2("\n\nsome banner text\n").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn reg_query_output_is_parsed() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\leidian\\LDPlayer9\r\n    \
                   InstallDir    REG_SZ    C:\\LDPlayer\\LDPlayer9\\\r\n\r\n";
        assert_eq!(
            parse_reg_values(out, "InstallDir"),
            vec![r"C:\LDPlayer\LDPlayer9".to_string()],
            "the trailing separator must be trimmed or every join gets a double slash"
        );
    }

    #[cfg(windows)]
    #[test]
    fn candidates_cover_more_than_the_c_drive() {
        let dirs = candidate_dirs();
        assert!(dirs.iter().any(|d| d.starts_with("D:")), "second drives are common");
        assert!(dirs.len() >= COMMON_SUBDIRS.len() * DRIVES.len());
    }

    #[test]
    fn conventional_endpoints_step_by_two() {
        assert_eq!(conventional_endpoint(0), "127.0.0.1:5555");
        assert_eq!(conventional_endpoint(3), "127.0.0.1:5561");
    }
}
