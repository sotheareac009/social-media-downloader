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

/// Find gallery-dl using the same strategy as [`crate::download::ytdlp::locate`]:
/// a GUI app does not inherit the shell PATH that `brew install` relies on.
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
    let out = crate::process::command(locate()?)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// List the video posts on an Instagram profile.
pub async fn list_instagram_profile(
    url: &url::Url,
    cookies: Option<&Path>,
) -> Result<ProfileListing> {
    let binary = locate().ok_or(AppError::ListerMissing)?;

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

    let out = cmd
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|_| AppError::ListerMissing)?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(classify_failure(&stderr));
    }

    let records: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| AppError::MalformedProviderResponse)?;

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

    let out = cmd
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|_| AppError::ListerMissing)?;

    if !out.status.success() {
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

fn classify_failure(stderr: &str) -> AppError {
    let lower = stderr.to_lowercase();
    if lower.contains("login") || lower.contains("authentication") || lower.contains("challenge") {
        return AppError::MediaNotPublic;
    }
    if lower.contains("not found") || lower.contains("does not exist") || lower.contains("404") {
        return AppError::NoMediaFound;
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
    use super::*;

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
