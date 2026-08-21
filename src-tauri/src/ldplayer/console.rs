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
const COMMON_DIRS: &[&str] = &[
    r"C:\LDPlayer\LDPlayer9",
    r"C:\Program Files\LDPlayer\LDPlayer9",
    r"C:\Program Files (x86)\LDPlayer\LDPlayer9",
    r"D:\LDPlayer\LDPlayer9",
    r"C:\LDPlayer\LDPlayer64",
    r"C:\LDPlayer\LDPlayer4.0",
    r"C:\Program Files\LDPlayer\LDPlayer4.0",
    r"C:\ChangZhi\dnplayer2",
    r"C:\LDPlayer\dnplayer2",
];

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
        for dir in registry_dirs().iter().map(PathBuf::from) {
            if let Some(found) = console_in(&dir) {
                return Some(Self::new(found));
            }
        }
        for dir in COMMON_DIRS {
            if let Some(found) = console_in(Path::new(dir)) {
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
        let serial = out
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('*') && *l != "unknown")
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

/// Install directories recorded by LDPlayer's installer.
///
/// Shelling out to `reg` rather than adding a registry crate: this is one
/// read, on one platform, and `reg.exe` is present on every Windows.
#[cfg(windows)]
fn registry_dirs() -> Vec<String> {
    const KEYS: &[&str] = &[
        r"HKEY_LOCAL_MACHINE\SOFTWARE\leidian\LDPlayer9",
        r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\leidian\LDPlayer9",
        r"HKEY_LOCAL_MACHINE\SOFTWARE\leidian\LDPlayer",
        r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\leidian\LDPlayer",
        r"HKEY_LOCAL_MACHINE\SOFTWARE\XuanZhi\LDPlayer",
    ];
    let mut out = Vec::new();
    for key in KEYS {
        let Ok(res) = std::process::Command::new("reg")
            .args(["query", key, "/v", "InstallDir"])
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        let text = String::from_utf8_lossy(&res.stdout);
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("InstallDir") {
                if let Some(value) = rest.split_once("REG_SZ").map(|(_, v)| v.trim()) {
                    if !value.is_empty() {
                        out.push(value.to_string());
                    }
                }
            }
        }
    }
    out
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

    #[test]
    fn conventional_endpoints_step_by_two() {
        assert_eq!(conventional_endpoint(0), "127.0.0.1:5555");
        assert_eq!(conventional_endpoint(3), "127.0.0.1:5561");
    }
}
