//! Instagram profile listing, via gallery-dl.
//!
//! WHY A SECOND TOOL. yt-dlp lists TikTok profiles and YouTube channels fine,
//! but marks its own `instagram:user` extractor CURRENTLY BROKEN, and has no
//! extractor at all for the `/reels/` tab. gallery-dl tracks Instagram's
//! listing API and does work.
//!
//! WHAT IT IS AND IS NOT USED FOR. Listing only. gallery-dl answers "which
//! posts exist"; every actual download still goes through yt-dlp on the normal
//! path, so quality selection, the H.264 compatibility pass, retries, progress
//! and cancellation all behave identically to a hand-pasted link. Adding a
//! second *downloader* would have doubled all of that.
//!
//! Output shape, from `--dump-json`: a flat array of records, each an array
//! whose first element is a kind tag.
//!
//!   `[2, {...}]`      post-level record, one per post
//!   `[3, url, {...}]` file record, one per media file within a post
//!
//! File records are the ones used here: a post with no video yields no `mp4`
//! file record, so photo-only posts drop out without a special case.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::download::ytdlp::{ProfileEntry, ProfileListing};
use crate::errors::{AppError, Result};

/// Extensions that indicate a video. Instagram serves mp4; mov is accepted in
/// case that changes.
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov"];

/// Upper bound on a single listing.
///
/// Enumeration is paginated and counts against the user's account, so a
/// profile with thousands of posts would be a long, conspicuous crawl. The
/// cap keeps the confirmation card useful rather than making someone wait
/// minutes to be told a number.
const MAX_POSTS: usize = 300;

/// How long a profile listing may run before it is given up on.
///
/// Generous, because listing 300 posts is genuinely slow: gallery-dl paginates
/// and paces itself. But bounded, because the alternative — what shipped
/// before this — is a Download button that spins forever with nothing to click
/// and no way to tell a slow listing from a dead one.
const LISTING_TIMEOUT: Duration = Duration::from_secs(240);

/// A version probe answers instantly or not at all.
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

/// What a bounded run produced.
#[derive(Debug)]
struct Ran {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run a gallery-dl command with a ceiling on how long it may take, keeping
/// whatever it printed even when it has to be killed.
///
/// Spawned and drained rather than `output()`-ed, for one reason: `output()`
/// hands back nothing at all if the future is dropped on a timeout, so a stall
/// reports silence. gallery-dl says *why* it is stuck on stderr — a login wall,
/// a rate limit, a DNS failure — and that line is the whole difference between
/// a diagnosable problem and "it spins forever".
///
/// `tokio::time::timeout` wraps the wait itself, never its result: a child that
/// never exits is precisely the case this exists for.
async fn run_bounded(mut cmd: tokio::process::Command, budget: Duration) -> Result<Ran> {
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncReadExt;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|_| AppError::ListerMissing)?;

    // Both pipes are drained concurrently. Reading one to completion while the
    // other fills its buffer is a deadlock, and Windows' pipe buffers are small
    // enough to reach it on a chatty run.
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));

    let drain = |mut pipe: Option<_>, buf: Arc<Mutex<Vec<u8>>>| {
        tokio::spawn(async move {
            let Some(pipe) = pipe.take() else { return };
            let mut pipe: tokio::process::ChildStdout = pipe;
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.lock().expect("buf").extend_from_slice(&chunk[..n]),
                }
            }
        })
    };
    let out_task = drain(child.stdout.take(), out_buf.clone());
    // stderr has a different type, so it gets its own loop rather than a
    // generic that would need a trait object for one call.
    let err_task = {
        let buf = err_buf.clone();
        let mut pipe = child.stderr.take();
        tokio::spawn(async move {
            let Some(mut pipe) = pipe.take() else { return };
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.lock().expect("buf").extend_from_slice(&chunk[..n]),
                }
            }
        })
    };

    let waited = tokio::time::timeout(budget, child.wait()).await;

    match waited {
        Ok(Ok(status)) => {
            let _ = out_task.await;
            let _ = err_task.await;
            Ok(Ran {
                success: status.success(),
                stdout: std::mem::take(&mut *out_buf.lock().expect("buf")),
                stderr: std::mem::take(&mut *err_buf.lock().expect("buf")),
            })
        }
        Ok(Err(_)) => Err(AppError::ListerMissing),
        Err(_) => {
            let _ = child.kill().await;
            out_task.abort();
            err_task.abort();
            // The last thing it said before it stopped saying anything.
            let stderr = String::from_utf8_lossy(&err_buf.lock().expect("buf")).to_string();
            Err(AppError::ListerTimedOut(last_meaningful_line(&stderr)))
        }
    }
}

/// The last line worth showing a user out of a chatty stderr.
fn last_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next_back()
        .map(|l| l.chars().take(160).collect())
        .unwrap_or_else(|| "it printed nothing before it stalled".to_string())
}

pub fn locate() -> Option<PathBuf> {
    if let Some(explicit) = crate::config::read("MEDIA_DOWNLOADER_GALLERYDL") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    let exe = if cfg!(windows) { "gallery-dl.exe" } else { "gallery-dl" };

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
            "/opt/homebrew/bin/gallery-dl",
            "/usr/local/bin/gallery-dl",
            "/usr/bin/gallery-dl",
        ] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(exe);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub async fn version() -> Option<String> {
    let mut cmd = crate::process::command(locate()?);
    cmd.arg("--version");
    let out = run_bounded(cmd, VERSION_TIMEOUT).await.ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// List the video posts on an Instagram profile.
pub async fn list_instagram_profile(
    url: &url::Url,
    cookies: Option<&Path>,
) -> Result<ProfileListing> {
    let binary = locate().ok_or(AppError::ListerMissing)?;
    let url = instagram_listing_url(url);

    let mut cmd = crate::process::command(binary);
    cmd.arg("--dump-json")
        // Metadata only; nothing is written to disk by this call.
        .arg("--simulate")
        .arg("--range")
        .arg(format!("1-{MAX_POSTS}"));

    // The same jar yt-dlp uses. gallery-dl reads Netscape format too, so the
    // session captured in the login window serves both without a second login.
    if let Some(path) = cookies {
        cmd.arg("--cookies").arg(path);
    }

    cmd.arg(url.as_str());
    let out = run_bounded(cmd, LISTING_TIMEOUT).await?;

    if !out.success {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(classify_failure(&stderr));
    }

    let records: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

    // gallery-dl exits 0 and reports the failure *inside* the JSON, as a
    // record whose first field is -1. Without reading it, a refused listing
    // looks exactly like an empty one - which is how "Instagram bounced us"
    // came out as "no video was found at that link".
    if let Some(message) = dumped_error(&records) {
        return Err(classify_failure(&message));
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<ProfileEntry> = Vec::new();
    let mut uploader: Option<String> = None;

    for record in records.as_array().into_iter().flatten() {
        let Some(parts) = record.as_array() else { continue };
        // File records carry the media; post-level records do not.
        if parts.first().and_then(|k| k.as_u64()) != Some(3) {
            continue;
        }
        let Some(meta) = parts.last().and_then(|m| m.as_object()) else {
            continue;
        };

        let is_video = meta
            .get("extension")
            .and_then(|e| e.as_str())
            .map(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if !is_video {
            continue;
        }

        let Some(post_url) = meta.get("post_url").and_then(|u| u.as_str()) else {
            continue;
        };
        // A carousel yields one file record per item; the post is queued once.
        if !seen.insert(post_url.to_string()) {
            continue;
        }

        if uploader.is_none() {
            uploader = meta
                .get("username")
                .or_else(|| meta.get("owner_username"))
                .and_then(|u| u.as_str())
                .map(str::to_string);
        }

        entries.push(ProfileEntry {
            id: meta
                .get("post_shortcode")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string(),
            url: post_url.to_string(),
            title: meta
                .get("description")
                .and_then(|d| d.as_str())
                .map(|d| d.lines().next().unwrap_or(d).trim().to_string())
                .filter(|d| !d.is_empty()),
            duration_seconds: None,
        });
    }

    if entries.is_empty() {
        // Instagram serves anonymous callers an empty list rather than an
        // error, so "nothing here" and "you are not signed in" look identical
        // from the outside. The session is the thing that tells them apart.
        return Err(if cookies.is_none() {
            AppError::MediaNotPublic
        } else {
            AppError::NoMediaFound
        });
    }

    Ok(ProfileListing {
        uploader: uploader.unwrap_or_else(|| "this profile".to_string()),
        profile_url: url.to_string(),
        count: entries.len(),
        entries,
        kind: crate::download::ytdlp::ListingKind::Profile,
    })
}

/// List an X (Twitter) profile's video tweets via gallery-dl.
///
/// yt-dlp has no X timeline extractor, so — as with Instagram — gallery-dl does
/// the enumeration and yt-dlp downloads each resulting tweet. X requires an
/// authenticated session; the same captured cookies serve both tools.
///
/// gallery-dl gives each media file its tweet metadata (`tweet_id`, `author`),
/// from which a canonical `/status/<id>` URL is built to queue for yt-dlp.
pub async fn list_x_profile(url: &url::Url, cookies: Option<&Path>) -> Result<ProfileListing> {
    let binary = locate().ok_or(AppError::ListerMissing)?;

    let mut cmd = crate::process::command(binary);
    cmd.arg("--dump-json")
        .arg("--simulate")
        .arg("--range")
        .arg(format!("1-{MAX_POSTS}"));
    if let Some(path) = cookies {
        cmd.arg("--cookies").arg(path);
    }

    cmd.arg(url.as_str());
    let out = run_bounded(cmd, LISTING_TIMEOUT).await?;

    if !out.success {
        return Err(classify_failure(&String::from_utf8_lossy(&out.stderr)));
    }

    let records: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<ProfileEntry> = Vec::new();
    let mut uploader: Option<String> = None;

    for record in records.as_array().into_iter().flatten() {
        let Some(parts) = record.as_array() else { continue };
        // Type 3 records are files (with metadata); skip directory/other records.
        if parts.first().and_then(|k| k.as_u64()) != Some(3) {
            continue;
        }
        let Some(meta) = parts.last().and_then(|m| m.as_object()) else {
            continue;
        };

        // Only video / animated-gif tweets — yt-dlp can't fetch a still photo.
        // Check the media type and, as a fallback, the file extension, so a
        // field-name difference between gallery-dl versions doesn't drop videos.
        let kind = meta.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let ext_is_video = meta
            .get("extension")
            .and_then(|e| e.as_str())
            .map(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if kind != "video" && kind != "animated_gif" && !ext_is_video {
            continue;
        }

        let Some(tweet_id) = meta
            .get("tweet_id")
            .and_then(|v| v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string)))
        else {
            continue;
        };
        if !seen.insert(tweet_id.clone()) {
            continue;
        }

        // The tweet's own author, so the /status/ URL resolves to the right
        // handle; fall back to the timeline owner, then a neutral placeholder.
        let handle = meta
            .get("author")
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .or_else(|| meta.get("user").and_then(|u| u.get("name")).and_then(|n| n.as_str()))
            .unwrap_or("i");
        if uploader.is_none() {
            uploader = meta
                .get("user")
                .and_then(|u| u.get("nick").or_else(|| u.get("name")))
                .and_then(|n| n.as_str())
                .map(str::to_string);
        }

        entries.push(ProfileEntry {
            id: tweet_id.clone(),
            url: format!("https://x.com/{handle}/status/{tweet_id}"),
            title: meta
                .get("content")
                .and_then(|c| c.as_str())
                .map(|c| c.lines().next().unwrap_or(c).trim().to_string())
                .filter(|c| !c.is_empty()),
            duration_seconds: None,
        });
    }

    if entries.is_empty() {
        return Err(AppError::NoMediaFound);
    }

    Ok(ProfileListing {
        uploader: uploader.unwrap_or_else(|| "this profile".to_string()),
        profile_url: url.to_string(),
        count: entries.len(),
        entries,
        kind: crate::download::ytdlp::ListingKind::Profile,
    })
}

/// The URL to hand gallery-dl for an Instagram profile: always `/posts/`.
///
/// Neither shape people paste works directly, for different reasons:
///
///   * `/<handle>/reels` - Instagram answers gallery-dl with a redirect to its
///     home page, which aborts the extraction outright.
///   * `/<handle>` - gallery-dl answers with a *queue* record pointing at
///     `/<handle>/posts/` rather than any files, and `--dump-json` does not
///     follow queued URLs. The listing looks empty while being perfectly
///     healthy.
///
/// `/posts/` is the one that yields file records, and it carries the reels too
/// - a reel is a video post, and the caller keeps only video posts anyway.
///
/// The query goes as well: `?hl=en` and friends are display preferences that
/// mean nothing to an extractor.
fn instagram_listing_url(url: &url::Url) -> url::Url {
    let mut out = url.clone();
    out.set_query(None);

    let segments: Vec<String> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();

    let handle = match segments.as_slice() {
        [handle] => Some(handle),
        [handle, tab] if tab.eq_ignore_ascii_case("reels") => Some(handle),
        _ => None,
    };
    if let Some(handle) = handle {
        out.set_path(&format!("/{handle}/posts/"));
    }
    out
}

/// The message from a gallery-dl error record, if the dump carries one.
///
/// Errors arrive as `[-1, {"error": ..., "message": ...}]` alongside the real
/// records, rather than on stderr with a non-zero exit.
fn dumped_error(records: &serde_json::Value) -> Option<String> {
    for record in records.as_array()? {
        let Some(parts) = record.as_array() else { continue };
        if parts.first().and_then(|k| k.as_i64()) != Some(-1) {
            continue;
        }
        let Some(meta) = parts.last().and_then(|m| m.as_object()) else {
            continue;
        };
        if let Some(message) = meta
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| meta.get("error").and_then(|e| e.as_str()))
        {
            return Some(message.to_string());
        }
    }
    None
}

fn classify_failure(stderr: &str) -> AppError {
    let lower = stderr.to_lowercase();
    // Instagram answers an anonymous listing with a bare `401 Unauthorized`
    // from its own API, naming neither login nor authentication - so matching
    // only those words left the real cause invisible and the user reading
    // "no video found" about a profile full of reels.
    const AUTH_MARKERS: &[&str] = &[
        "login",
        "authentication",
        "authenticated cookies",
        "challenge",
        "401",
        "unauthorized",
        "403",
        "forbidden",
        "not logged in",
    ];
    if AUTH_MARKERS.iter().any(|m| lower.contains(m)) {
        return AppError::MediaNotPublic;
    }
    if lower.contains("not found") || lower.contains("does not exist") || lower.contains("404") {
        return AppError::NoMediaFound;
    }
    // Instagram bounces an unauthenticated or stale session to its home page
    // rather than refusing outright.
    if lower.contains("redirect to home page") || lower.contains("abortextraction") {
        return AppError::MediaNotPublic;
    }
    if lower.contains("rate") || lower.contains("429") || lower.contains("please wait") {
        return AppError::TemporarilyUnavailable;
    }
    let detail = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error");
    AppError::EngineFailed(detail.chars().take(200).collect())
}

#[cfg(test)]
mod tests {

    /// The bug this exists for: with no ceiling, a lister that never returns
    /// leaves the Download button spinning forever, and a slow listing is
    /// indistinguishable from a dead one.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_never_finishes_is_given_up_on() {
        let mut cmd = tokio::process::Command::new("/bin/sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let err = run_bounded(cmd, Duration::from_millis(250)).await.unwrap_err();

        assert!(matches!(err, AppError::ListerTimedOut(_)), "got {err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it waited for the child instead of the budget"
        );
    }

    #[test]
    fn a_stalled_lister_reports_its_last_line() {
        let noisy = "[instagram][info] Starting\n[instagram][warning] HTTP 401\n  \n";
        assert_eq!(last_meaningful_line(noisy), "[instagram][warning] HTTP 401");
        // Silence is itself worth saying, rather than an empty message.
        assert!(last_meaningful_line("").contains("nothing"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_finishes_in_time_is_returned() {
        let mut cmd = tokio::process::Command::new("/bin/echo");
        cmd.arg("hello");
        let out = run_bounded(cmd, Duration::from_secs(10)).await.unwrap();
        assert!(out.success);
        assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
    }
    use super::*;

    #[test]
    fn a_bare_401_reads_as_needing_a_login_not_as_an_empty_profile() {
        // What Instagram actually answers an anonymous reels listing with. It
        // names neither login nor authentication, which is how this used to
        // surface as "no video was found at that link".
        let stderr = "[instagram][error] HttpError: '401 Unauthorized' for \
                      'https://www.instagram.com/api/v1/clips/user/'";
        assert!(matches!(classify_failure(stderr), AppError::MediaNotPublic));

        for line in [
            "[instagram][error] AuthRequired: authenticated cookies needed",
            "HttpError: '403 Forbidden'",
            "login required",
        ] {
            assert!(
                matches!(classify_failure(line), AppError::MediaNotPublic),
                "{line}"
            );
        }
    }

    #[test]
    fn every_profile_shape_is_asked_for_as_its_posts_tab() {
        // Instagram answers `/handle/reels` with a redirect to its home page,
        // which aborts the extraction; the profile root lists the same reels.
        let url = url::Url::parse("https://www.instagram.com/someone/reels/?hl=en").unwrap();
        let asked = instagram_listing_url(&url);
        assert_eq!(asked.path(), "/someone/posts/");
        assert_eq!(asked.query(), None, "display preferences mean nothing here");

        // A bare profile needs the same rewrite: gallery-dl answers it with a
        // queue record pointing at /posts/, and --dump-json does not follow one.
        let plain = url::Url::parse("https://www.instagram.com/someone/?hl=en").unwrap();
        assert_eq!(instagram_listing_url(&plain).path(), "/someone/posts/");

        // A post link is not a profile listing and must be left alone.
        let post = url::Url::parse("https://www.instagram.com/reel/DW4aUnSk8Wr/").unwrap();
        assert_eq!(instagram_listing_url(&post).path(), "/reel/DW4aUnSk8Wr/");
    }

    #[test]
    fn an_error_hidden_in_a_successful_dump_is_found() {
        // gallery-dl exits 0 and puts the failure in the JSON, so an aborted
        // listing is otherwise indistinguishable from an empty one.
        let raw = serde_json::json!([[
            -1,
            {
                "error": "AbortExtraction",
                "message": "HTTP redirect to home page (https://www.instagram.com/)"
            }
        ]]);
        let message = dumped_error(&raw).expect("error record");
        assert!(message.contains("redirect to home page"), "{message}");
        assert!(matches!(classify_failure(&message), AppError::MediaNotPublic));

        // A dump of real records carries no error.
        let ok = serde_json::json!([[3, {}, { "extension": "mp4" }]]);
        assert!(dumped_error(&ok).is_none());
    }

    #[test]
    fn a_missing_profile_is_still_a_missing_profile() {
        for line in ["404 Not Found", "NotFoundError: user does not exist"] {
            assert!(
                matches!(classify_failure(line), AppError::NoMediaFound),
                "{line}"
            );
        }
    }

    /// Real `--dump-json` output shape, trimmed: one reel and one video post,
    /// each with its post-level record and its file record.
    fn sample() -> serde_json::Value {
        serde_json::json!([
            [2, {"post_url": "https://www.instagram.com/reel/AAA/", "type": "reel"}],
            [3, "https://cdn/1.mp4", {
                "post_url": "https://www.instagram.com/reel/AAA/",
                "post_shortcode": "AAA", "extension": "mp4",
                "username": "ve.leo.vet", "description": "first line\nsecond"
            }],
            [2, {"post_url": "https://www.instagram.com/p/BBB/", "type": "post"}],
            [3, "https://cdn/2.mp4", {
                "post_url": "https://www.instagram.com/p/BBB/",
                "post_shortcode": "BBB", "extension": "mp4", "username": "ve.leo.vet"
            }],
            // A photo post: no video file record should survive the filter.
            [3, "https://cdn/3.jpg", {
                "post_url": "https://www.instagram.com/p/CCC/",
                "post_shortcode": "CCC", "extension": "jpg", "username": "ve.leo.vet"
            }],
            // A carousel's second video, same post - must not queue twice.
            [3, "https://cdn/4.mp4", {
                "post_url": "https://www.instagram.com/reel/AAA/",
                "post_shortcode": "AAA", "extension": "mp4", "username": "ve.leo.vet"
            }]
        ])
    }

    /// Mirrors the parsing in `list_instagram_profile`, which cannot be called
    /// without spawning the binary.
    fn parse(v: &serde_json::Value) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut urls = Vec::new();
        for record in v.as_array().into_iter().flatten() {
            let Some(parts) = record.as_array() else { continue };
            if parts.first().and_then(|k| k.as_u64()) != Some(3) {
                continue;
            }
            let Some(meta) = parts.last().and_then(|m| m.as_object()) else { continue };
            let is_video = meta
                .get("extension")
                .and_then(|e| e.as_str())
                .map(|e| VIDEO_EXTENSIONS.contains(&e))
                .unwrap_or(false);
            if !is_video {
                continue;
            }
            let Some(u) = meta.get("post_url").and_then(|u| u.as_str()) else { continue };
            if seen.insert(u.to_string()) {
                urls.push(u.to_string());
            }
        }
        urls
    }

    #[test]
    fn picks_one_url_per_video_post() {
        let urls = parse(&sample());
        assert_eq!(
            urls,
            vec![
                "https://www.instagram.com/reel/AAA/",
                "https://www.instagram.com/p/BBB/"
            ]
        );
    }

    #[test]
    fn photo_posts_are_excluded_without_a_special_case() {
        assert!(!parse(&sample()).iter().any(|u| u.contains("CCC")));
    }

    #[test]
    fn a_carousel_is_queued_once_not_once_per_item() {
        let urls = parse(&sample());
        assert_eq!(urls.iter().filter(|u| u.contains("AAA")).count(), 1);
    }

    #[test]
    fn a_login_wall_is_distinguished_from_a_missing_profile() {
        assert!(matches!(
            classify_failure("Login required to access this profile"),
            AppError::MediaNotPublic
        ));
        assert!(matches!(
            classify_failure("HTTP 404: Not Found"),
            AppError::NoMediaFound
        ));
    }
}
