//! The extraction engine.
//!
//! Why yt-dlp rather than our own extractor: neither Facebook nor TikTok has a
//! public API that returns a media file. Both serve short-lived, signed CDN
//! URLs embedded in page state that changes without notice. yt-dlp tracks that
//! churn full-time; a hand-rolled extractor would be broken within weeks.
//!
//! SECURITY. Every invocation passes:
//!
//!   * `--ignore-config`, so a `yt-dlp.conf` sitting in the user's home cannot
//!     inject flags we did not choose (including cookie and netrc flags).
//!   * `--no-cookies-from-browser`, unconditionally. The app may use a session
//!     it captured in its own login window, but it must never read the user's
//!     browser profile, which would sweep in every site they are signed into.
//!
//! Cookies are otherwise a per-call decision: `None` becomes `--no-cookies`,
//! which is what YouTube, Facebook and TikTok always get. Only Instagram is
//! ever passed a jar, and only one the user explicitly captured - see
//! [`crate::download::session`] for why that exception exists and what it costs.
//!
//! A `.netrc` needs no flag of its own: yt-dlp only consults one when `--netrc`
//! is passed, and `--ignore-config` stops a config file from passing it. (There
//! is no `--no-netrc` option - yt-dlp rejects it outright.)
//!
//! No OAuth credential is passed here, and there is no code path that could:
//! this module has no access to the keychain and takes no token argument. The
//! Instagram jar arrives as a plain file path chosen by the caller, so this
//! module cannot reach a stored session on its own either.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use url::Url;

use crate::download::quality::Quality;
use crate::errors::{AppError, Result};

/// Marker prefix for our machine-readable progress lines, chosen so it cannot
/// collide with yt-dlp's ordinary human output.
const PROGRESS_PREFIX: &str = "MDPROGRESS";

/// Marker for the engine's report of where the finished file landed.
const PATH_PREFIX: &str = "MDPATH ";

const PROFILE_LIST_ATTEMPTS: usize = 3;
const PROFILE_LIST_BACKOFF_SECONDS: &[u64] = &[5, 15];

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
    /// False when the post carries no video stream at all.
    ///
    /// TikTok photo/slideshow posts are the common case: images plus a music
    /// track, presented in the app exactly like a video, but exposing a single
    /// audio-only format. There is no video to fetch, so the download is an
    /// mp3 - which looks like a bug unless the app says what happened.
    pub has_video: bool,
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
/// `None` is yt-dlp's default. YouTube's anti-bot layer intermittently answers
/// its media URLs with `HTTP 403`, so a fallback chain is needed - but the
/// order is quality-critical, and getting it wrong is not a subtle failure.
///
/// Measured on one video, downloading and probing the result:
///
/// | client         | result |
/// |---|---|
/// | default        | 1280p  |
/// | `tv_embedded`  | 1280p  |
/// | `web_embedded` | 1280p  |
/// | `android_vr`   | 1280p  |
/// | `mweb`         | 640p   |
/// | `web`, `ios`   | no usable format |
///
/// `mweb` is last precisely because it serves only format 18. An earlier
/// version of this chain put it second, so every 403 - which is common -
/// silently downgraded the download to 360p no matter what quality the user
/// had chosen. Anything added here must be checked for the same trap: a client
/// that "works" while quietly capping quality is worse than one that fails.
pub const YOUTUBE_CLIENTS: &[Option<&str>] = &[
    None,
    Some("tv_embedded"),
    Some("web_embedded"),
    Some("android_vr"),
    Some("mweb"),
];

/// Apply a player-client override, if one is being tried.
fn apply_client(cmd: &mut Command, client: Option<&str>) {
    if let Some(c) = client {
        cmd.arg("--extractor-args")
            .arg(format!("youtube:player_client={c}"));
    }
}



/// The engine's own version string, for the UI's diagnostics panel.
pub async fn version() -> Result<String> {
    let out = crate::process::command(engine_path()?)
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
/// Flags that hold for *every* invocation, session or not.
///
/// `--no-cookies-from-browser` is the one that never becomes conditional: the
/// app may use a session it captured in its own login window, but it must
/// never read the user's browser profile, which would sweep in every site they
/// are signed into.
pub const HARDENED_FLAGS: &[&str] = &["--ignore-config", "--no-cookies-from-browser"];

fn hardened_base(cmd: &mut Command, cookies: Option<&Path>) {
    cmd.args(HARDENED_FLAGS);

    // Either a jar this app captured, or none at all. There is no third case,
    // and no source other than Instagram is ever given one.
    match cookies {
        Some(path) => {
            cmd.arg("--cookies").arg(path);
        }
        None => {
            cmd.arg("--no-cookies");
        }
    }

    cmd.arg("--socket-timeout")
        .arg("20")
        .arg("--retries")
        .arg("3")
        // Never let the engine prompt; a GUI child process has no console to
        // answer on and would hang forever.
        .stdin(Stdio::null());
}

/// Read metadata without downloading. Cheap enough to run on paste.
pub async fn probe(url: &Url, client: Option<&str>, cookies: Option<&Path>) -> Result<MediaInfo> {
    let mut cmd = crate::process::command(engine_path()?);
    hardened_base(&mut cmd, cookies);
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

    media_info_from(&v)
}

/// Shape a probe response into [`MediaInfo`].
fn media_info_from(v: &serde_json::Value) -> Result<MediaInfo> {
    // A URL that resolves to a playlist still yields entries; take the first,
    // since `--no-playlist` means anything else is a shape we didn't ask for.
    let v = v
        .get("entries")
        .and_then(|e| e.get(0))
        .filter(|_| v.get("id").is_none())
        .unwrap_or(v);

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
        has_video: has_video_stream(v),
    })
}

/// Whether any format in the response carries a video track.
fn has_video_stream(v: &serde_json::Value) -> bool {
    let usable = |val: &serde_json::Value| {
        matches!(val.get("vcodec").and_then(|c| c.as_str()), Some(c) if c != "none")
    };

    if usable(v) {
        return true;
    }
    v.get("formats")
        .and_then(|f| f.as_array())
        .map(|arr| arr.iter().any(usable))
        // No format list and no top-level vcodec: assume video rather than
        // mislabelling an ordinary post as a slideshow.
        .unwrap_or(true)
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
    let mut last = AppError::NoMediaFound;

    for attempt in 0..PROFILE_LIST_ATTEMPTS {
        match list_profile_once(url).await {
            Ok(listing) => return Ok(listing),
            Err(AppError::TemporarilyUnavailable) if attempt + 1 < PROFILE_LIST_ATTEMPTS => {
                last = AppError::TemporarilyUnavailable;
                let delay = PROFILE_LIST_BACKOFF_SECONDS
                    .get(attempt)
                    .copied()
                    .unwrap_or(15);
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
            Err(e) => return Err(e),
        }
    }

    Err(last)
}

async fn list_profile_once(url: &Url) -> Result<ProfileListing> {
    let mut cmd = crate::process::command(engine_path()?);
    hardened_base(&mut cmd, None);
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

/// One quality tier a video actually offers.
#[derive(Debug, Clone, Serialize)]
pub struct VideoFormat {
    /// The tier as the platform names it - "1080p", "4320p".
    pub label: String,
    /// The numeric tier, for matching against a [`Quality`] cap.
    pub tier: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Metadata plus the quality tiers a specific link offers.
#[derive(Debug, Clone, Serialize)]
pub struct FormatReport {
    pub info: MediaInfo,
    /// Highest tier first.
    pub formats: Vec<VideoFormat>,
    pub best_label: Option<String>,
}

/// Read the tier a format belongs to.
///
/// `format_note` is the authority here, not `height`. An ultrawide 8K video is
/// 7680x3200, so its height is 3200 and calling it "3200p" would be wrong -
/// yt-dlp labels that same format `4320p`, which is what a person recognises.
/// Height is only the fallback for formats with no note.
fn format_tier(f: &serde_json::Value) -> Option<(u32, String)> {
    // Skip audio-only entries; they have no quality tier to offer.
    if f.get("vcodec").and_then(|v| v.as_str()) == Some("none") {
        return None;
    }

    if let Some(note) = f.get("format_note").and_then(|v| v.as_str()) {
        // "1080p60" and "1080p" are the same tier at different frame rates.
        let digits: String = note.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(tier) = digits.parse::<u32>() {
            if tier > 0 {
                return Some((tier, format!("{tier}p")));
            }
        }
    }

    let height = f.get("height").and_then(|v| v.as_u64())? as u32;
    (height > 0).then(|| (height, format!("{height}p")))
}

/// Probe a link for its metadata and the quality tiers it offers.
///
/// Tries each player client in turn: YouTube's anti-bot layer can refuse one
/// client's metadata while another answers, exactly as it does for media.
pub async fn inspect_formats(
    url: &Url,
    clients: &[Option<&str>],
    cookies: Option<&Path>,
) -> Result<FormatReport> {
    let mut last = AppError::NoMediaFound;

    for client in clients {
        match inspect_formats_once(url, *client, cookies).await {
            Ok(report) => return Ok(report),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn inspect_formats_once(
    url: &Url,
    client: Option<&str>,
    cookies: Option<&Path>,
) -> Result<FormatReport> {
    let mut cmd = crate::process::command(engine_path()?);
    hardened_base(&mut cmd, cookies);
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

    let mut seen: std::collections::BTreeMap<u32, VideoFormat> = Default::default();
    for f in v.get("formats").and_then(|f| f.as_array()).into_iter().flatten() {
        if let Some((tier, label)) = format_tier(f) {
            seen.entry(tier).or_insert(VideoFormat {
                label,
                tier,
                width: f.get("width").and_then(|w| w.as_u64()).map(|w| w as u32),
                height: f.get("height").and_then(|h| h.as_u64()).map(|h| h as u32),
            });
        }
    }

    let formats: Vec<VideoFormat> = seen.into_values().rev().collect();
    let best_label = formats.first().map(|f| f.label.clone());

    Ok(FormatReport {
        info: media_info_from(&v)?,
        formats,
        best_label,
    })
}

/// A download in flight. Dropping this does not stop the child; call
/// [`Running::kill`].
pub struct Running {
    child: Child,
    /// Filled by the stdout reader when the engine reports its final path.
    output_path: Arc<Mutex<Option<String>>>,
}

impl Running {
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// Where the finished file actually landed, as reported by the engine.
    ///
    /// Worth asking for rather than guessing: picking "the newest file in the
    /// folder" mis-attributes whenever two downloads finish close together,
    /// which is exactly what a queue of large videos does.
    pub fn output_path(&self) -> Option<String> {
        self.output_path.lock().ok().and_then(|p| p.clone())
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
    quality: Quality,
    cookies: Option<&Path>,
    prefer_compatible: bool,
) -> Result<Running> {
    let template = format!(
        "{PROGRESS_PREFIX} %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s"
    );

    let mut cmd = crate::process::command(engine_path()?);
    hardened_base(&mut cmd, cookies);
    let ffmpeg = locate_ffmpeg();
    apply_client(&mut cmd, client);
    cmd.arg("--no-playlist")
        .arg("-f")
        .arg(quality.format_selector(ffmpeg.is_some(), prefer_compatible))
        .arg("-o")
        // Byte-truncated so a long caption cannot exceed the filesystem's
        // name limit; the id keeps two posts with the same title distinct.
        .arg(dest_dir.join("%(title).100B [%(id)s].%(ext)s"))
        .arg("--newline")
        .arg("--progress-template")
        .arg(&template)
        .arg("--no-warnings")
        // Ask the engine to name its own output rather than inferring it.
        //
        // `--print` implies BOTH `--simulate` and `--quiet`. `--no-simulate`
        // keeps the download; `--progress --no-quiet` keeps the progress
        // stream, without which this whole command downloads in silence. That
        // is not hypothetical - it shipped that way and killed every progress
        // bar, which is why the two negations below are load-bearing.
        .arg("--no-simulate")
        .arg("--progress")
        .arg("--no-quiet")
        .arg("--print")
        .arg(format!("after_move:{PATH_PREFIX}%(filepath)s"))
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

    let output_path = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&output_path);

    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(path) = line.trim().strip_prefix(PATH_PREFIX) {
                if let Ok(mut slot) = sink.lock() {
                    *slot = Some(path.to_string());
                }
                continue;
            }
            if let Some(p) = parse_progress(&line) {
                // A closed receiver means the job is gone; stop parsing.
                if tx.send(p).is_err() {
                    break;
                }
            }
        }
    });

    Ok(Running { child, output_path })
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
        "unexpected response from webpage request",
        "unexpected respone from webpage request",
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
    fn a_photo_post_is_recognised_as_having_no_video() {
        // Exactly the shape TikTok returns for a slideshow: one audio format.
        let slideshow = serde_json::json!({
            "id": "7664982070095056149",
            "ext": "mp3",
            "vcodec": "none",
            "acodec": "mp3",
            "formats": [{"format_id": "audio", "vcodec": "none", "acodec": "mp3"}]
        });
        assert!(!has_video_stream(&slideshow));

        let ordinary = serde_json::json!({
            "id": "1", "vcodec": "h264",
            "formats": [{"vcodec": "none", "acodec": "mp4a"}, {"vcodec": "h264"}]
        });
        assert!(has_video_stream(&ordinary));
    }

    #[test]
    fn an_absent_format_list_is_assumed_to_be_video() {
        // Mislabelling a normal post as a slideshow would be worse than the
        // occasional missed one.
        let sparse = serde_json::json!({"id": "1", "title": "x"});
        assert!(has_video_stream(&sparse));
    }

    #[test]
    fn a_tier_comes_from_the_note_not_the_pixel_height() {
        // A real 8K ultrawide format: 7680x3200. Calling this "3200p" would be
        // wrong and unrecognisable; yt-dlp itself labels it 4320p.
        let f = serde_json::json!({
            "vcodec": "vp9", "width": 7680, "height": 3200, "format_note": "4320p"
        });
        assert_eq!(format_tier(&f), Some((4320, "4320p".to_string())));
    }

    #[test]
    fn frame_rate_does_not_split_a_tier() {
        // "1080p60" and "1080p" are one entry in the picker, not two.
        let sixty = serde_json::json!({"vcodec": "avc1", "height": 1080, "format_note": "1080p60"});
        let plain = serde_json::json!({"vcodec": "avc1", "height": 1080, "format_note": "1080p"});
        assert_eq!(format_tier(&sixty), format_tier(&plain));
    }

    #[test]
    fn height_is_the_fallback_when_a_note_is_missing_or_unhelpful() {
        let no_note = serde_json::json!({"vcodec": "avc1", "height": 720});
        assert_eq!(format_tier(&no_note), Some((720, "720p".to_string())));

        // Some formats carry a descriptive note rather than a resolution.
        let worded = serde_json::json!({"vcodec": "avc1", "height": 480, "format_note": "tiny"});
        assert_eq!(format_tier(&worded), Some((480, "480p".to_string())));
    }

    #[test]
    fn audio_only_formats_are_not_quality_tiers() {
        let audio = serde_json::json!({"vcodec": "none", "acodec": "mp4a", "format_note": "medium"});
        assert_eq!(format_tier(&audio), None);
    }

    #[test]
    fn the_quality_capping_client_is_the_last_resort() {
        // `mweb` only ever serves 360p. If it moves up the chain, a routine
        // 403 downgrades every download regardless of the quality setting -
        // which is exactly the bug this ordering fixes.
        let idx = YOUTUBE_CLIENTS
            .iter()
            .position(|c| *c == Some("mweb"))
            .expect("mweb must stay in the chain as a last resort");
        assert_eq!(
            idx,
            YOUTUBE_CLIENTS.len() - 1,
            "mweb caps at 360p and must be tried last: {YOUTUBE_CLIENTS:?}"
        );
        assert_eq!(YOUTUBE_CLIENTS[0], None, "the default client offers the most");
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
            "ERROR: [tiktok] {videoid}: Unexpected response from webpage request; please report this issue",
            "ERROR: [tiktok] {videoid}: Unexpected respone from webpage request; please report this issue",
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
