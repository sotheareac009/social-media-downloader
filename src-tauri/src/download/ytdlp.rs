//! The extraction engine.
//!
//! Why yt-dlp rather than our own extractor: neither Facebook nor TikTok has a
//! public API that returns a media file. Both serve short-lived, signed CDN
//! URLs embedded in page state that changes without notice. yt-dlp tracks that
//! churn full-time; a hand-rolled extractor would be broken within weeks.
//!
//! SECURITY - this module is the reason the feature can honestly be called
//! "public only". Every invocation passes:
//!
//!   * `--ignore-config`, so a `yt-dlp.conf` sitting in the user's home cannot
//!     inject flags we did not choose (including cookie and netrc flags).
//!   * `--no-cookies` and `--no-cookies-from-browser`, so the engine cannot
//!     read the browser session that would let it reach private posts.
//!
//! A `.netrc` needs no flag of its own: yt-dlp only consults one when `--netrc`
//! is passed, and `--ignore-config` stops a config file from passing it. (There
//! is no `--no-netrc` option - yt-dlp rejects it outright.)
//!
//! No OAuth credential is passed here, and there is no code path that could:
//! this module has no access to the keychain and takes no token argument. A
//! post that is not publicly visible therefore fails as
//! [`AppError::MediaNotPublic`] rather than quietly succeeding.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use url::Url;

use crate::errors::{AppError, Result};

/// Marker prefix for our machine-readable progress lines, chosen so it cannot
/// collide with yt-dlp's ordinary human output.
const PROGRESS_PREFIX: &str = "MDPROGRESS";

/// Emitted for every progress line the engine prints.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub downloaded_bytes: u64,
    /// Absent until the server declares a length, and for some live streams.
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<u64>,
    /// 0.0-1.0, only when a total is known.
    pub fraction: Option<f64>,
}

/// What a probe learned about a link, before any bytes are fetched.
#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    /// Best guess at the finished size; often absent before download starts.
    pub estimated_bytes: Option<u64>,
    pub extension: Option<String>,
}

/// Absolute path to a usable yt-dlp, resolved once per call.
///
/// Order: explicit override, then PATH, then the handful of locations package
/// managers actually use. The last step matters because a GUI app launched
/// from Finder or the Start menu does not inherit the shell PATH that
/// `brew install yt-dlp` relies on.
pub fn locate() -> Option<PathBuf> {
    if let Some(explicit) = crate::config::read("MEDIA_DOWNLOADER_YTDLP") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    let exe = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let fallbacks: &[&str] = if cfg!(windows) {
        &[]
    } else {
        &[
            "/opt/homebrew/bin/yt-dlp", // Apple silicon Homebrew
            "/usr/local/bin/yt-dlp",    // Intel Homebrew, manual installs
            "/usr/bin/yt-dlp",          // Linux distro packages
            "/snap/bin/yt-dlp",
        ]
    };
    for candidate in fallbacks {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }

    // `pipx`/`pip --user` installs, which are common and rarely on a GUI PATH.
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(exe);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn engine_path() -> Result<PathBuf> {
    locate().ok_or(AppError::EngineMissing)
}

/// Find FFmpeg, using the same search strategy as [`locate`].
///
/// Optional, but it decides how good a YouTube download can be. YouTube serves
/// video and audio as separate streams above 360p, so without a merger the
/// best *single* file available is 360p. Measured on one video: 360p
/// progressive versus 1080p merged. Facebook and TikTok serve progressive
/// files, so they are unaffected either way.
pub fn locate_ffmpeg() -> Option<PathBuf> {
    if let Some(explicit) = crate::config::read("MEDIA_DOWNLOADER_FFMPEG") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if !cfg!(windows) {
        for candidate in [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
            "/snap/bin/ffmpeg",
        ] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Player clients to try for YouTube, in order.
///
/// `None` is yt-dlp's default, which offers the full format ladder up to 1080p
/// and beyond. YouTube's anti-bot layer increasingly answers its media URLs
/// with `HTTP 403`, and when it does, `mweb` still serves - but only format 18,
/// which is 360p. So the order matters: best quality first, then the client
/// that actually works.
///
/// Measured on one video: default → 1080p offered but 403 on fetch; `mweb` →
/// 360p, downloaded fine. `tv`, `ios` and `web_safari` offered no usable
/// progressive format at all.
pub const YOUTUBE_CLIENTS: &[Option<&str>] = &[None, Some("mweb")];

/// Apply a player-client override, if one is being tried.
fn apply_client(cmd: &mut Command, client: Option<&str>) {
    if let Some(c) = client {
        cmd.arg("--extractor-args")
            .arg(format!("youtube:player_client={c}"));
    }
}

/// The format selector to ask for, given whether a merger is available.
///
/// With FFmpeg: best video plus best audio, preferring mp4/m4a so the merge is
/// a remux rather than a re-encode. Without it: the best *single* file, since
/// a merge-only format would download in full and then fail at the last step.
fn format_selector(has_ffmpeg: bool) -> &'static str {
    if has_ffmpeg {
        "bv*[ext=mp4]+ba[ext=m4a]/bv*+ba/b[ext=mp4]/b"
    } else {
        "b[ext=mp4]/b[ext=mov]/b"
    }
}

/// The engine's own version string, for the UI's diagnostics panel.
pub async fn version() -> Result<String> {
    let out = Command::new(engine_path()?)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| AppError::EngineMissing)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Flags applied to every invocation. See the module note: the cookie and
/// config flags are what keep this to public media.
///
/// Public so `tests/engine_contract.rs` can hand the list to the real yt-dlp
/// and prove every one is accepted. A flag yt-dlp does not recognise makes it
/// exit before downloading anything, which is a total outage of the feature,
/// so "these options exist" is worth asserting rather than assuming.
pub const HARDENED_FLAGS: &[&str] = &[
    "--ignore-config",
    "--no-cookies",
    "--no-cookies-from-browser",
];

fn hardened_base(cmd: &mut Command) {
    cmd.args(HARDENED_FLAGS)
        .arg("--socket-timeout")
        .arg("20")
        .arg("--retries")
        .arg("3")
        // Never let the engine prompt; a GUI child process has no console to
        // answer on and would hang forever.
        .stdin(Stdio::null());
}

/// Read metadata without downloading. Cheap enough to run on paste.
pub async fn probe(url: &Url, client: Option<&str>) -> Result<MediaInfo> {
    let mut cmd = Command::new(engine_path()?);
    hardened_base(&mut cmd);
    apply_client(&mut cmd, client);
    cmd.arg("--no-playlist")
        .arg("--dump-single-json")
        .arg("--no-warnings")
        .arg(url.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = cmd.output().await.map_err(|_| AppError::EngineMissing)?;

    if !out.status.success() {
        return Err(classify_failure(&String::from_utf8_lossy(&out.stderr)));
    }

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

    // A URL that resolves to a playlist still yields entries; take the first,
    // since `--no-playlist` means anything else is a shape we didn't ask for.
    let v = v
        .get("entries")
        .and_then(|e| e.get(0))
        .filter(|_| v.get("id").is_none())
        .unwrap_or(&v);

    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or_default();
    if id.is_empty() {
        return Err(AppError::NoMediaFound);
    }

    Ok(MediaInfo {
        id: id.to_string(),
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Untitled")
            .to_string(),
        uploader: str_field(v, "uploader").or_else(|| str_field(v, "channel")),
        duration_seconds: v.get("duration").and_then(|x| x.as_f64()),
        thumbnail_url: str_field(v, "thumbnail"),
        estimated_bytes: v
            .get("filesize")
            .and_then(|x| x.as_u64())
            .or_else(|| v.get("filesize_approx").and_then(|x| x.as_u64())),
        extension: str_field(v, "ext"),
    })
}

fn str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty() && *s != "NA")
        .map(str::to_string)
}

/// One video in a creator's feed, as listed without visiting its page.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileEntry {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub duration_seconds: Option<f64>,
}

/// A creator's feed: who they are, and every post found.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileListing {
    /// The handle as the platform spells it.
    pub uploader: String,
    pub profile_url: String,
    pub count: usize,
    pub entries: Vec<ProfileEntry>,
}

/// List every video on a profile, without downloading or even opening any.
///
/// `--flat-playlist` is what makes this affordable: yt-dlp reads the feed
/// listing and stops, rather than resolving each post's formats. Enumerating
/// 133 videos takes a few seconds instead of several minutes.
///
/// TikTok's feed endpoint is genuinely flaky when hit anonymously and
/// intermittently answers with "Unable to extract secondary user ID" - the
/// same profile succeeds moments later - so this retries a little harder than
/// a single-video probe does.
pub async fn list_profile(url: &Url) -> Result<ProfileListing> {
    let mut cmd = Command::new(engine_path()?);
    hardened_base(&mut cmd);
    cmd.arg("--yes-playlist")
        .arg("--flat-playlist")
        .arg("--dump-single-json")
        .arg("--no-warnings")
        .arg(url.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = cmd.output().await.map_err(|_| AppError::EngineMissing)?;
    if !out.status.success() {
        return Err(classify_failure(&String::from_utf8_lossy(&out.stderr)));
    }

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

    let entries: Vec<ProfileEntry> = v
        .get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    // Without a URL there is nothing to queue, so an entry
                    // missing one is skipped rather than failing the listing.
                    let url = str_field(e, "url").or_else(|| str_field(e, "webpage_url"))?;
                    Some(ProfileEntry {
                        id: str_field(e, "id").unwrap_or_default(),
                        url,
                        title: str_field(e, "title"),
                        duration_seconds: e.get("duration").and_then(|d| d.as_f64()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if entries.is_empty() {
        return Err(AppError::NoMediaFound);
    }

    Ok(ProfileListing {
        uploader: str_field(&v, "title")
            .or_else(|| str_field(&v, "uploader"))
            .or_else(|| str_field(&v, "channel"))
            .unwrap_or_else(|| "this profile".to_string()),
        profile_url: url.to_string(),
        count: entries.len(),
        entries,
    })
}

/// A download in flight. Dropping this does not stop the child; call
/// [`Running::kill`].
pub struct Running {
    child: Child,
}

impl Running {
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

/// Start a download, streaming progress into `tx`.
///
/// Returns once the process has been spawned; the caller awaits
/// [`wait`] for the outcome.
pub fn start(
    url: &Url,
    dest_dir: &Path,
    tx: mpsc::UnboundedSender<Progress>,
    client: Option<&str>,
) -> Result<Running> {
    let template = format!(
        "{PROGRESS_PREFIX} %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s"
    );

    let mut cmd = Command::new(engine_path()?);
    hardened_base(&mut cmd);
    let ffmpeg = locate_ffmpeg();
    apply_client(&mut cmd, client);
    cmd.arg("--no-playlist")
        .arg("-f")
        .arg(format_selector(ffmpeg.is_some()))
        .arg("-o")
        // Byte-truncated so a long caption cannot exceed the filesystem's
        // name limit; the id keeps two posts with the same title distinct.
        .arg(dest_dir.join("%(title).100B [%(id)s].%(ext)s"))
        .arg("--newline")
        .arg("--progress-template")
        .arg(&template)
        .arg("--no-warnings")
        .arg(url.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Point the engine at the exact binary we found, so it doesn't depend on
    // the PATH a GUI process inherited, and ask for an mp4 container so a
    // merged file plays everywhere.
    if let Some(ffmpeg) = &ffmpeg {
        cmd.arg("--ffmpeg-location").arg(ffmpeg);
        cmd.arg("--merge-output-format").arg("mp4");
    }

    let mut child = cmd.spawn().map_err(|_| AppError::EngineMissing)?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Internal("engine stdout unavailable".into()))?;

    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(p) = parse_progress(&line) {
                // A closed receiver means the job is gone; stop parsing.
                if tx.send(p).is_err() {
                    break;
                }
            }
        }
    });

    Ok(Running { child })
}

/// Await completion, mapping a non-zero exit to a specific error.
///
/// Takes `&mut` rather than ownership so a caller can `select!` this against a
/// cancellation signal and still reach [`Running::kill`] on the losing branch.
pub async fn wait(running: &mut Running) -> Result<()> {
    let mut stderr_text = String::new();
    if let Some(stderr) = running.child.stderr.take() {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Keep only the tail; a long warning stream is not worth holding.
            if stderr_text.len() < 4096 {
                stderr_text.push_str(&line);
                stderr_text.push('\n');
            }
        }
    }

    let status = running
        .child
        .wait()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if status.success() {
        Ok(())
    } else {
        Err(classify_failure(&stderr_text))
    }
}

/// One progress line into a [`Progress`].
///
/// yt-dlp writes `NA` for any field it does not know yet, so every numeric
/// parse here is allowed to fail without failing the line.
fn parse_progress(line: &str) -> Option<Progress> {
    let rest = line.trim().strip_prefix(PROGRESS_PREFIX)?;
    let mut parts = rest.split_whitespace();

    let downloaded: u64 = parts.next()?.parse().ok()?;
    let total = parts.next().and_then(|s| s.parse::<u64>().ok());
    let speed = parts.next().and_then(|s| s.parse::<f64>().ok());
    let eta = parts
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f.max(0.0) as u64);

    Some(Progress {
        downloaded_bytes: downloaded,
        total_bytes: total,
        speed_bps: speed,
        eta_seconds: eta,
        // Clamped: yt-dlp's estimate can be smaller than the real total, which
        // would otherwise drive a progress bar past 100%.
        fraction: total
            .filter(|t| *t > 0)
            .map(|t| (downloaded as f64 / t as f64).clamp(0.0, 1.0)),
    })
}

/// Turn engine stderr into a specific, honest error.
///
/// The distinction that matters to a user is "this needs a login" versus
/// "this is broken", because only the first one is their fault and only the
/// second one is worth retrying.
fn classify_failure(stderr: &str) -> AppError {
    let lower = stderr.to_lowercase();

    const LOGIN_MARKERS: &[&str] = &[
        "login required",
        "log in",
        "sign in",
        "requires authentication",
        "private video",
        "this video is private",
        "only available to",
        "not available in your",
        "age-restricted",
        "cookies",
        "authentication",
    ];
    if LOGIN_MARKERS.iter().any(|m| lower.contains(m)) {
        return AppError::MediaNotPublic;
    }

    // Checked BEFORE the missing-media markers, because the strings overlap:
    // "unable to extract universal data for rehydration" is TikTok throttling
    // a perfectly good video, and matching it as "no video found" both lies to
    // the user and skips the retry that would have worked.
    // A refusal from the media CDN. Waiting changes nothing; the caller
    // responds by asking again as a different player client.
    const REFUSAL_MARKERS: &[&str] = &["http error 403", "403: forbidden"];
    if REFUSAL_MARKERS.iter().any(|m| lower.contains(m)) {
        return AppError::ClientRefused;
    }

    const RETRYABLE_MARKERS: &[&str] = &[
        "universal data for rehydration",
        "webpage video data",
        "rate limit",
        "rate-limit",
        "too many requests",
        "429",
        "temporarily unavailable",
        "try again later",
        "unable to download webpage: http error 5",
    ];
    if RETRYABLE_MARKERS.iter().any(|m| lower.contains(m)) {
        return AppError::TemporarilyUnavailable;
    }

    const MISSING_MARKERS: &[&str] = &[
        "unsupported url",
        "no video",
        "unable to extract",
        "video unavailable",
        "not found",
        "404",
        "has been removed",
        "no media found",
    ];
    if MISSING_MARKERS.iter().any(|m| lower.contains(m)) {
        return AppError::NoMediaFound;
    }

    // Fall back to the engine's own last meaningful line. yt-dlp does not put
    // credentials in its errors, and none were supplied to it in the first
    // place, so this is safe to surface.
    let detail = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error");
    AppError::EngineFailed(truncate(detail, 200))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_depends_on_whether_a_merger_is_available() {
        // Without FFmpeg the selector must never ask for `video+audio`: that
        // format downloads in full and then fails at the merge step.
        let without = format_selector(false);
        assert!(!without.contains('+'), "{without}");

        // With it, the merged form must be preferred over the progressive
        // fallback, or YouTube silently caps at 360p.
        let with = format_selector(true);
        assert!(with.starts_with("bv*"), "{with}");
        assert!(with.contains('+'), "{with}");
        assert!(with.ends_with("/b"), "a single-file fallback must remain: {with}");
    }

    #[test]
    fn parses_a_complete_progress_line() {
        let p = parse_progress("MDPROGRESS 1048576 4194304 524288.0 6").unwrap();
        assert_eq!(p.downloaded_bytes, 1_048_576);
        assert_eq!(p.total_bytes, Some(4_194_304));
        assert_eq!(p.speed_bps, Some(524_288.0));
        assert_eq!(p.eta_seconds, Some(6));
        assert_eq!(p.fraction, Some(0.25));
    }

    #[test]
    fn tolerates_the_na_fields_yt_dlp_emits_early() {
        // Before headers arrive, everything but the byte counter can be NA.
        let p = parse_progress("MDPROGRESS 4096 NA NA NA").unwrap();
        assert_eq!(p.downloaded_bytes, 4096);
        assert!(p.total_bytes.is_none());
        assert!(p.speed_bps.is_none());
        assert!(p.fraction.is_none(), "no total means no percentage");
    }

    #[test]
    fn ignores_ordinary_engine_chatter() {
        for line in [
            "[download] Destination: video.mp4",
            "[facebook] Extracting URL: https://fb.watch/x/",
            "",
            "MDPROGRES 1 2 3 4",
        ] {
            assert!(parse_progress(line).is_none(), "{line}");
        }
    }

    #[test]
    fn fraction_never_exceeds_one() {
        // The estimate can undershoot the real size; the bar must not overflow.
        let p = parse_progress("MDPROGRESS 900 800 NA NA").unwrap();
        assert_eq!(p.fraction, Some(1.0));
    }

    #[test]
    fn login_walls_are_reported_as_such() {
        for stderr in [
            "ERROR: [facebook] 123: Login required to view this video",
            "ERROR: [tiktok] Sign in to confirm you're not a bot",
            "ERROR: This video is private",
        ] {
            assert!(
                matches!(classify_failure(stderr), AppError::MediaNotPublic),
                "{stderr}"
            );
        }
    }

    #[test]
    fn a_cdn_refusal_asks_for_a_different_client() {
        // YouTube's anti-bot layer. The video exists and the metadata parsed;
        // only the media fetch was refused.
        for stderr in [
            "ERROR: unable to download video data: HTTP Error 403: Forbidden",
            "ERROR: [youtube] abc: HTTP Error 403: Forbidden",
        ] {
            assert!(
                matches!(classify_failure(stderr), AppError::ClientRefused),
                "{stderr}"
            );
        }
    }

    #[test]
    fn throttling_is_not_reported_as_a_missing_video() {
        // The exact string TikTok returns when asked for many videos quickly.
        // It was previously classified as "no video found", which told users
        // their video didn't exist when a retry would have fetched it.
        for stderr in [
            "ERROR: [TikTok] 7668241190671764757: Unable to extract universal data for rehydration; please report this issue",
            "ERROR: [TikTok] 123: Unable to extract webpage video data",
            "ERROR: HTTP Error 429: Too Many Requests",
        ] {
            assert!(
                matches!(classify_failure(stderr), AppError::TemporarilyUnavailable),
                "{stderr}"
            );
        }
    }

    #[test]
    fn missing_media_is_distinguished_from_a_login_wall() {
        for stderr in [
            "ERROR: Unsupported URL: https://www.facebook.com/somebody",
            "ERROR: [tiktok] 7: Video unavailable",
        ] {
            assert!(
                matches!(classify_failure(stderr), AppError::NoMediaFound),
                "{stderr}"
            );
        }
    }

    #[test]
    fn an_unrecognised_failure_keeps_the_engines_last_line() {
        let err = classify_failure("warning: something\nERROR: the disk is on fire\n");
        match err {
            AppError::EngineFailed(d) => assert_eq!(d, "ERROR: the disk is on fire"),
            other => panic!("unexpected: {other}"),
        }
    }
}
