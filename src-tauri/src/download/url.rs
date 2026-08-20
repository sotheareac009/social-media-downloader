//! Recognising the links this build is willing to fetch.
//!
//! Two jobs, both security-relevant:
//!
//!   1. **Allowlist the host.** yt-dlp supports well over a thousand sites. If
//!      an arbitrary string were handed to it, a typo or a pasted tracking link
//!      would silently reach some unrelated extractor. Only Facebook and TikTok
//!      hosts are accepted here, matched against a fixed set - never by
//!      substring, because `tiktok.com.evil.test` contains `tiktok.com`.
//!
//!   2. **Reject anything that isn't https.** yt-dlp would happily accept
//!      `file://`, which would turn a paste box into a local file reader.

use url::{Host, Url};

use crate::errors::{AppError, Result};

/// Which platform a link belongs to. Deliberately separate from
/// [`crate::auth::ProviderId`]: that enum answers "who can I sign in as", this
/// one answers "who can I fetch public media from". Facebook is in both; they
/// are not the same question and are not required to stay in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Facebook,
    TikTok,
    YouTube,
    Instagram,
    X,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Facebook => "facebook",
            Source::TikTok => "tiktok",
            Source::YouTube => "youtube",
            Source::Instagram => "instagram",
            Source::X => "x",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Source::Facebook => "Facebook",
            Source::TikTok => "TikTok",
            Source::YouTube => "YouTube",
            Source::Instagram => "Instagram",
            Source::X => "X",
        }
    }
}

/// What a link points at: one video, or a creator's whole feed.
///
/// Facebook has no `Profile` form: yt-dlp has no page-listing extractor for it
/// - `facebook.com/<page>/videos` is answered with "Unsupported URL" - so every
/// accepted Facebook link is a single post.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Single,
    Profile,
}

/// Hosts accepted for each source, compared exactly after stripping a leading
/// `www.` / `m.` / `web.` prefix.
const FACEBOOK_HOSTS: &[&str] = &["facebook.com", "fb.watch", "fb.com", "facebook.net"];
const TIKTOK_HOSTS: &[&str] = &["tiktok.com", "vm.tiktok.com", "vt.tiktok.com"];
const YOUTUBE_HOSTS: &[&str] = &["youtube.com", "youtu.be", "youtube-nocookie.com"];
const INSTAGRAM_HOSTS: &[&str] = &["instagram.com", "instagr.am", "ddinstagram.com"];
const X_HOSTS: &[&str] = &["x.com", "twitter.com"];

/// Strip the subdomains that are merely presentational, so `m.facebook.com`
/// and `web.facebook.com` resolve to the same allowlist entry. Anything else
/// (`cdn.facebook.com.example.test`) keeps its full host and fails to match.
fn normalise_host(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    for prefix in ["www.", "m.", "web.", "mobile.", "l."] {
        if let Some(rest) = host.strip_prefix(prefix) {
            return rest;
        }
    }
    host
}

/// Instagram Stories, which yt-dlp's `InstagramStoryIE` handles with a session:
/// `/stories/<user>`, `/stories/<user>/<id>`, `/stories/highlights/<id>`.
fn is_instagram_story(url: &Url) -> bool {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    matches!(segments.as_slice(), ["stories", rest @ ..] if !rest.is_empty())
}

/// Instagram post shapes that carry media: `/reel/<code>`, `/reels/<code>`,
/// `/p/<code>`, `/tv/<code>`, and the same nested under a handle.
fn is_instagram_post(url: &Url) -> bool {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    const KINDS: &[&str] = &["reel", "reels", "p", "tv"];
    match segments.as_slice() {
        // instagram.com/reel/<code>
        [kind, code, ..] if KINDS.contains(kind) => !code.is_empty(),
        // instagram.com/<handle>/reel/<code>
        [_, kind, code, ..] if KINDS.contains(kind) => !code.is_empty(),
        _ => false,
    }
}

/// Instagram paths that are *not* a handle, so `/explore/...` is never
/// mistaken for a user called "explore".
const INSTAGRAM_RESERVED: &[&str] = &[
    "p", "reel", "reels", "tv", "explore", "accounts", "stories", "direct",
    "about", "developer", "legal", "privacy", "session",
];

/// A profile, or its reels tab: `/<handle>` or `/<handle>/reels`.
///
/// Listed by gallery-dl rather than yt-dlp - see
/// [`crate::download::gallerydl`] for why.
fn is_instagram_profile(url: &Url) -> bool {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    let handle_ok = |h: &&str| !h.is_empty() && !INSTAGRAM_RESERVED.contains(&h.to_ascii_lowercase().as_str());

    match segments.as_slice() {
        [handle] => handle_ok(handle),
        [handle, tab] if tab.eq_ignore_ascii_case("reels") => handle_ok(handle),
        _ => false,
    }
}

/// Classify a pasted link and say whether it names a video or a whole profile.
/// Path first-segments on x.com that are features, not user handles.
const X_RESERVED: &[&str] = &[
    "home", "explore", "notifications", "messages", "i", "search", "settings",
    "compose", "hashtag", "login", "signup", "tos", "privacy", "about",
    "download", "intent",
];

/// A whole-profile X link: `x.com/<handle>` or `x.com/<handle>/media`. A
/// `/status/<id>` link is a single post and deliberately does not match.
fn is_x_profile(url: &Url) -> bool {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    let handle_ok = |h: &str| {
        !h.is_empty() && !X_RESERVED.contains(&h.to_ascii_lowercase().as_str())
    };
    match segments.as_slice() {
        [handle] => handle_ok(handle),
        [handle, tab] => handle_ok(handle) && matches!(*tab, "media" | "with_replies"),
        _ => false,
    }
}

pub fn classify_target(raw: &str) -> Result<(Source, Url, TargetKind)> {
    let (source, url) = classify(raw)?;
    match source {
        Source::TikTok if is_tiktok_profile(&url) => Ok((source, url, TargetKind::Profile)),
        Source::YouTube => Ok(classify_youtube(url)),
        Source::Instagram if is_instagram_story(&url) => Ok((source, url, TargetKind::Single)),
        Source::Instagram if is_instagram_profile(&url) => {
            Ok((source, url, TargetKind::Profile))
        }
        Source::X if is_x_profile(&url) => Ok((source, url, TargetKind::Profile)),
        _ => Ok((source, url, TargetKind::Single)),
    }
}

/// YouTube needs its own pass, because a channel URL is not directly listable.
///
/// Asking yt-dlp for `youtube.com/@NASA` returns the channel's *tabs* -
/// "NASA - Videos", "NASA - Live" - as entries with no URL, which is not a
/// list of videos and cannot be queued. Appending `/videos` is what turns it
/// into the uploads feed, so that normalisation happens here rather than
/// surfacing as an empty listing.
fn classify_youtube(mut url: Url) -> (Source, Url, TargetKind) {
    let segments: Vec<String> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();

    let first = segments.first().map(String::as_str).unwrap_or("");

    // A playlist is already a listing; nothing to normalise.
    if first == "playlist" {
        return (Source::YouTube, url, TargetKind::Profile);
    }

    let is_channel_root = (first.starts_with('@') && first.len() > 1)
        || ((first == "channel" || first == "c" || first == "user") && segments.len() == 2);

    if is_channel_root {
        // `@handle/videos` and `@handle/shorts` are already feeds; a bare
        // handle is the channel home and needs the uploads tab.
        if segments.len() == 1 || (first.starts_with('@') && segments.len() == 1) {
            url.set_path(&format!("/{first}/videos"));
        }
        return (Source::YouTube, url, TargetKind::Profile);
    }

    // A channel tab that already names a feed.
    if first.starts_with('@') && matches!(segments.get(1).map(String::as_str), Some("videos" | "shorts" | "streams")) {
        return (Source::YouTube, url, TargetKind::Profile);
    }

    // Everything else - /watch?v=, /shorts/ID, youtu.be/ID, /live/ID - is one
    // video.
    (Source::YouTube, url, TargetKind::Single)
}

/// A TikTok profile is `/@handle` and nothing more.
///
/// `/@handle/video/123` is one post, and `/@handle/live` is a stream rather
/// than a feed, so both stay `Single`. Matching on the *shape* of the path
/// rather than on the absence of "video" keeps future TikTok sub-pages out by
/// default instead of letting them fall through as profiles.
fn is_tiktok_profile(url: &Url) -> bool {
    let mut segments = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()))
        .into_iter()
        .flatten();

    let Some(first) = segments.next() else {
        return false;
    };
    // A handle is at least one character after the `@`.
    first.starts_with('@') && first.len() > 1 && segments.next().is_none()
}

/// Classify a pasted link, or explain why it was refused.
pub fn classify(raw: &str) -> Result<(Source, Url)> {
    let parsed = Url::parse(raw.trim()).map_err(|_| AppError::UnsupportedUrl)?;

    // `https` only. `http` is upgraded rather than rejected, because people
    // paste it constantly and the redirect would happen anyway.
    let parsed = match parsed.scheme() {
        "https" => parsed,
        "http" => {
            let mut up = parsed;
            up.set_scheme("https").map_err(|_| AppError::UnsupportedUrl)?;
            up
        }
        _ => return Err(AppError::UnsupportedUrl),
    };

    // A domain host, not a bare IP - no allowlisted site is reachable by IP.
    let host = match parsed.host() {
        Some(Host::Domain(d)) => d.to_ascii_lowercase(),
        _ => return Err(AppError::UnsupportedUrl),
    };
    let host = normalise_host(&host);

    if FACEBOOK_HOSTS.contains(&host) {
        // Ephemeral Stories (`/stories/<id>/<token>/`) are a different feature
        // from a permanent `story.php?story_fbid=` post. No extractor - yt-dlp
        // or gallery-dl - supports them, so refuse with a clear reason rather
        // than letting the engine emit "Unsupported URL".
        if parsed
            .path_segments()
            .map(|mut s| s.next() == Some("stories"))
            .unwrap_or(false)
        {
            return Err(AppError::FacebookStoriesUnsupported);
        }
        Ok((Source::Facebook, parsed))
    } else if TIKTOK_HOSTS.contains(&host) {
        Ok((Source::TikTok, parsed))
    } else if YOUTUBE_HOSTS.contains(&host) {
        Ok((Source::YouTube, parsed))
    } else if INSTAGRAM_HOSTS.contains(&host) {
        // Only single-post shapes are accepted. yt-dlp's `instagram:user`
        // extractor is marked CURRENTLY BROKEN, so a bare profile link is
        // refused here rather than queued to fail later with a worse message.
        if is_instagram_post(&parsed) || is_instagram_story(&parsed) {
            Ok((Source::Instagram, parsed))
        } else if is_instagram_profile(&parsed) {
            Ok((Source::Instagram, parsed))
        } else {
            // Stories, explore pages, direct messages: no listing path exists
            // for these, so refuse rather than queue work that cannot run.
            Err(AppError::InstagramProfileUnsupported)
        }
    } else if X_HOSTS.contains(&host) {
        // A single post: yt-dlp's Twitter extractor handles x.com and
        // twitter.com `/status/<id>` links (video, GIF, or image).
        Ok((Source::X, parsed))
    } else {
        Err(AppError::UnsupportedUrl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_shapes_people_actually_paste() {
        let cases = [
            ("https://www.facebook.com/watch/?v=123456", Source::Facebook),
            ("https://www.facebook.com/reel/1234567890", Source::Facebook),
            ("https://fb.watch/aBcDeFg/", Source::Facebook),
            // The form the mobile app's "Copy link" button produces, which
            // redirects to the real reel/video page.
            ("https://www.facebook.com/share/r/199xesnx3h/", Source::Facebook),
            ("https://www.facebook.com/share/v/1CkYu6tToZ/", Source::Facebook),
            ("https://m.facebook.com/story.php?story_fbid=1&id=2", Source::Facebook),
            ("https://www.tiktok.com/@user/video/7300000000000000000", Source::TikTok),
            ("https://vm.tiktok.com/ZMabcdef/", Source::TikTok),
            ("https://vt.tiktok.com/ZSabcdef/", Source::TikTok),
        ];
        for (raw, want) in cases {
            let (got, _) = classify(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(got, want, "{raw}");
        }
    }

    #[test]
    fn http_is_upgraded_to_https() {
        let (_, url) = classify("http://www.tiktok.com/@u/video/7").unwrap();
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn lookalike_hosts_are_refused() {
        // The whole point of matching exactly rather than by `contains`.
        for raw in [
            "https://tiktok.com.evil.test/@u/video/7",
            "https://notfacebook.com/watch/?v=1",
            "https://facebook.com.attacker.test/reel/1",
            "https://evil.test/?u=https://tiktok.com/@u/video/7",
        ] {
            assert!(classify(raw).is_err(), "should have refused {raw}");
        }
    }

    #[test]
    fn other_supported_sites_are_still_out_of_scope() {
        // yt-dlp could fetch these; this build deliberately will not.
        for raw in ["https://vimeo.com/1", "https://www.dailymotion.com/video/x1"] {
            assert!(classify(raw).is_err(), "should have refused {raw}");
        }
    }

    #[test]
    fn non_http_schemes_cannot_reach_the_engine() {
        for raw in [
            "file:///etc/passwd",
            "ftp://facebook.com/x",
            "javascript:alert(1)",
        ] {
            assert!(classify(raw).is_err(), "should have refused {raw}");
        }
    }

    #[test]
    fn a_bare_handle_is_a_profile_and_a_post_is_not() {
        let profile = [
            "https://www.tiktok.com/@raimqqq",
            "https://www.tiktok.com/@raimqqq/",
            "https://tiktok.com/@khaby.lame",
        ];
        for raw in profile {
            let (_, _, kind) = classify_target(raw).unwrap();
            assert_eq!(kind, TargetKind::Profile, "{raw}");
        }

        let single = [
            "https://www.tiktok.com/@raimqqq/video/7674870647071296789",
            "https://www.tiktok.com/@raimqqq/live",
            "https://vm.tiktok.com/ZMabcdef/",
            // Facebook has no profile form at all - yt-dlp cannot list a page.
            "https://www.facebook.com/nasa/videos",
            "https://www.facebook.com/share/r/199xesnx3h/",
        ];
        for raw in single {
            let (_, _, kind) = classify_target(raw).unwrap();
            assert_eq!(kind, TargetKind::Single, "{raw}");
        }
    }

    #[test]
    fn facebook_ephemeral_stories_are_refused_clearly() {
        let err = classify("https://www.facebook.com/stories/122103258638400320/UzpfSVND=/?view_single=1").unwrap_err();
        assert!(matches!(err, AppError::FacebookStoriesUnsupported), "{err}");
        // A permanent story.php post is a different thing and still accepted.
        let (src, _) = classify("https://www.facebook.com/story.php?story_fbid=1&id=2").unwrap();
        assert_eq!(src, Source::Facebook);
    }

    #[test]
    fn instagram_post_shapes_are_accepted() {
        for raw in [
            "https://www.instagram.com/reel/Cabc123XYZ/",
            "https://instagram.com/reels/Cabc123XYZ/",
            "https://www.instagram.com/p/Cabc123XYZ/",
            "https://www.instagram.com/tv/Cabc123XYZ/",
            "https://www.instagram.com/someone/reel/Cabc123XYZ/",
        ] {
            let (source, _, kind) = classify_target(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(source, Source::Instagram, "{raw}");
            assert_eq!(kind, TargetKind::Single, "{raw}");
        }
    }

    #[test]
    fn instagram_stories_are_single_downloads() {
        for raw in [
            "https://www.instagram.com/stories/fruits_zipper/3570766765028588805/",
            "https://www.instagram.com/stories/fruits_zipper",
            "https://www.instagram.com/stories/highlights/18090946048123978/",
        ] {
            let (source, _, kind) = classify_target(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(source, Source::Instagram, "{raw}");
            assert_eq!(kind, TargetKind::Single, "{raw}");
        }
    }

    #[test]
    fn instagram_profiles_and_reel_tabs_are_listable() {
        for raw in [
            "https://www.instagram.com/ve.leo.vet/",
            "https://www.instagram.com/ve.leo.vet",
            "https://www.instagram.com/ve.leo.vet/reels/",
        ] {
            let (source, _, kind) = classify_target(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(source, Source::Instagram, "{raw}");
            assert_eq!(kind, TargetKind::Profile, "{raw}");
        }
    }

    #[test]
    fn instagram_pages_that_are_not_profiles_are_refused() {
        // `/explore/...` must not be read as a user named "explore", and
        // stories have no listing path at all.
        for raw in [
            "https://www.instagram.com/",
            "https://www.instagram.com/explore/tags/cats/",
            "https://www.instagram.com/accounts/login/",
        ] {
            let err = classify_target(raw).unwrap_err();
            assert!(
                matches!(err, AppError::InstagramProfileUnsupported),
                "{raw} should be refused: {err}"
            );
        }
    }

    #[test]
    fn a_reel_link_is_still_a_single_post_not_a_profile() {
        let (_, _, kind) = classify_target("https://www.instagram.com/reel/DcM1QVGvMPJ/").unwrap();
        assert_eq!(kind, TargetKind::Single);
    }

    #[test]
    fn youtube_videos_and_shorts_are_single() {
        for raw in [
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
            "https://youtu.be/jNQXAC9IVRw",
            "https://www.youtube.com/shorts/abc123XYZ",
            "https://m.youtube.com/watch?v=jNQXAC9IVRw",
            "https://www.youtube.com/live/abc123",
        ] {
            let (source, _, kind) = classify_target(raw).unwrap();
            assert_eq!(source, Source::YouTube, "{raw}");
            assert_eq!(kind, TargetKind::Single, "{raw}");
        }
    }

    #[test]
    fn a_bare_youtube_channel_is_normalised_to_its_uploads_feed() {
        // Without this, yt-dlp lists the channel's *tabs* - entries with no
        // URL - and the profile card would show nothing to download.
        let (_, url, kind) = classify_target("https://www.youtube.com/@NASA").unwrap();
        assert_eq!(kind, TargetKind::Profile);
        assert_eq!(url.path(), "/@NASA/videos");
    }

    #[test]
    fn youtube_feeds_that_are_already_specific_are_left_alone() {
        for (raw, want_path) in [
            ("https://www.youtube.com/@NASA/videos", "/@NASA/videos"),
            ("https://www.youtube.com/@NASA/shorts", "/@NASA/shorts"),
        ] {
            let (_, url, kind) = classify_target(raw).unwrap();
            assert_eq!(kind, TargetKind::Profile, "{raw}");
            assert_eq!(url.path(), want_path, "{raw}");
        }
        let (_, _, kind) = classify_target("https://www.youtube.com/playlist?list=PL123").unwrap();
        assert_eq!(kind, TargetKind::Profile);
    }

    #[test]
    fn an_empty_handle_is_not_a_profile() {
        // `tiktok.com/@` would otherwise enumerate nothing, slowly.
        let (_, _, kind) = classify_target("https://www.tiktok.com/@").unwrap();
        assert_eq!(kind, TargetKind::Single);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        for raw in ["", "   ", "not a url", "https://"] {
            assert!(classify(raw).is_err());
        }
    }
}
