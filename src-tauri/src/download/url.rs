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

/// Facebook paths that are a permalink to one post rather than a page name.
///
/// A bare first segment is otherwise read as a username, so every one of these
/// has to be listed or `facebook.com/watch/?v=1` would be refused as a profile.
const FACEBOOK_RESERVED: &[&str] = &[
    "watch", "reel", "reels", "video", "photo", "share", "groups", "media",
    "plugins", "marketplace", "events", "gaming", "pages", "p", "login",
    "help", "story", "stories", "permalink", "l", "privacy", "policies",
];

/// Facebook tabs that list a page's posts rather than naming one.
const FACEBOOK_TABS: &[&str] = &[
    "videos", "reels", "photos", "posts", "live", "about", "reviews", "shop",
    "events", "albums", "photos_albums", "community", "followers", "friends",
];

/// Whether a Facebook link points at a person or page instead of one post.
///
/// Neither yt-dlp nor gallery-dl can enumerate a Facebook page - yt-dlp has no
/// listing extractor for it, and gallery-dl's Facebook support covers photos
/// and albums, with no reels tab at all. So this is refused at the door with a
/// reason, rather than queued to fail later as "no video found", which reads
/// as "those reels do not exist".
///
/// `host` is checked by the caller: this must never run for `fb.watch`, whose
/// single path segment is a share code, not a username.
fn is_facebook_profile(url: &Url) -> bool {
    // `profile.php?id=…` and `/people/<name>/<id>` are the id-based spellings
    // of a profile, whatever tab they carry.
    let path = url.path().trim_end_matches('/');
    if path.eq_ignore_ascii_case("/profile.php") {
        return true;
    }

    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    let first = segments.first().copied().unwrap_or("");
    if first.eq_ignore_ascii_case("people") {
        return true;
    }

    // `?sk=reels_tab`, `?sk=videos` - the tab is named in the query, so the
    // path alone would look like a plain profile.
    if url.query_pairs().any(|(k, _)| k == "sk") {
        return true;
    }

    let reserved = |s: &str| {
        let s = s.to_ascii_lowercase();
        FACEBOOK_RESERVED.contains(&s.as_str()) || s.ends_with(".php")
    };

    match segments.as_slice() {
        // A bare page or username.
        [handle] => !handle.is_empty() && !reserved(handle),
        // `<page>/videos` is the tab; `<page>/videos/<id>` is one video, and
        // `<page>/posts/<id>` one post, so only the two-segment form matches.
        [handle, tab] => {
            !reserved(handle) && FACEBOOK_TABS.contains(&tab.to_ascii_lowercase().as_str())
        }
        _ => false,
    }
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
        // Left as the channel home on purpose. The caller expands it into the
        // feeds below, because "everything they posted" is Videos *plus*
        // Shorts plus past streams - rewriting to `/videos` here would quietly
        // drop every Short a channel has.
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

/// The tabs that together hold everything a channel has posted.
///
/// YouTube splits uploads across three feeds and a channel's home page lists
/// none of them directly: asking yt-dlp for `youtube.com/@NASA` returns the
/// channel's *tabs* as entries with no URL, which cannot be queued. Naming the
/// feeds explicitly is what turns a channel link into a list of videos.
const YOUTUBE_CHANNEL_FEEDS: &[&str] = &["videos", "shorts", "streams"];

/// Expand a channel home page into its upload feeds.
///
/// `None` for anything else, including a link that already names one feed:
/// pasting `/@handle/shorts` is a choice, and answering it with the long-form
/// uploads as well would ignore what was asked for.
pub fn youtube_channel_feeds(url: &Url) -> Option<Vec<Url>> {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    let base = match segments.as_slice() {
        [handle] if handle.starts_with('@') && handle.len() > 1 => format!("/{handle}"),
        [kind, name] if matches!(*kind, "channel" | "c" | "user") && !name.is_empty() => {
            format!("/{kind}/{name}")
        }
        _ => return None,
    };

    Some(
        YOUTUBE_CHANNEL_FEEDS
            .iter()
            .map(|feed| {
                let mut u = url.clone();
                u.set_path(&format!("{base}/{feed}"));
                // A channel link often carries `?si=` or a `pp=` tracking blob;
                // neither means anything on a feed URL.
                u.set_query(None);
                u
            })
            .collect(),
    )
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
/// Parse a link the way it arrives from a paste.
///
/// Browsers and mobile share sheets hand out `youtube.com/watch?v=...` with no
/// scheme, and copying from the address bar drops it too. A strict parse turns
/// that into "unsupported link", which reads as the app not supporting YouTube
/// rather than as a missing `https://`, so a scheme-less string is retried as
/// https.
///
/// Only `RelativeUrlWithoutBase` — the error meaning "no scheme at all" — takes
/// that path. Anything that did name a scheme keeps it and faces the https-only
/// check below, so `file:///etc/passwd` is still refused.
fn parse_pasted(raw: &str) -> Result<Url> {
    let raw = raw.trim();
    match Url::parse(raw) {
        Ok(u) => Ok(u),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            Url::parse(&format!("https://{raw}")).map_err(|_| AppError::UnsupportedUrl)
        }
        Err(_) => Err(AppError::UnsupportedUrl),
    }
}

pub fn classify(raw: &str) -> Result<(Source, Url)> {
    let parsed = parse_pasted(raw)?;

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
        // `fb.watch/<code>` is a share link, not a username, so the profile
        // shapes are only meaningful on facebook.com itself.
        if host != "fb.watch" && is_facebook_profile(&parsed) {
            return Err(AppError::FacebookProfileUnsupported);
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
    fn a_paste_with_no_scheme_is_treated_as_https() {
        // What the address bar and the mobile share sheet actually hand over.
        for raw in [
            "youtube.com/watch?v=uY6N9nWim4g&list=RDuY6N9nWim4g&start_radio=1",
            "www.tiktok.com/@user/video/7300000000000000000",
            "youtu.be/ApXoWvfEYVU?si=WJ6fGva7BBFeqTbD",
        ] {
            let (_, url) = classify(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(url.scheme(), "https", "{raw}");
        }
    }

    #[test]
    fn a_scheme_that_is_not_http_stays_refused() {
        // The https-only rule is what keeps the paste box from reading local
        // files; filling in a missing scheme must not weaken it.
        for raw in [
            "file:///etc/passwd",
            "file://youtube.com/watch?v=1",
            "javascript:alert(1)",
            "data:text/html,<script>",
        ] {
            assert!(classify(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn facebook_profiles_and_tabs_are_refused_with_their_own_reason() {
        // Nothing can list a Facebook page, so the refusal has to say that
        // rather than surfacing later as "no video found at that link".
        for raw in [
            "https://www.facebook.com/profile.php?id=61558591106716&sk=reels_tab",
            "https://www.facebook.com/people/Boss-KLOD/61558591106716/?sk=reels_tab",
            "https://www.facebook.com/BossKLOD",
            "https://www.facebook.com/BossKLOD/reels",
            "https://www.facebook.com/BossKLOD/videos",
            "https://www.facebook.com/BossKLOD/photos",
            "https://m.facebook.com/BossKLOD/posts",
        ] {
            match classify(raw) {
                Err(AppError::FacebookProfileUnsupported) => {}
                other => panic!("{raw}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_facebook_links_that_do_work_are_untouched() {
        // The refusal above must not swallow a single post: a share code is
        // not a username, and `<page>/videos/<id>` is one video, not the tab.
        for raw in [
            "https://www.facebook.com/watch/?v=123456",
            "https://www.facebook.com/reel/1234567890",
            "https://fb.watch/aBcDeFg/",
            "https://www.facebook.com/share/r/199xesnx3h/",
            "https://www.facebook.com/share/v/1CkYu6tToZ/",
            "https://m.facebook.com/story.php?story_fbid=1&id=2",
            "https://www.facebook.com/BossKLOD/videos/1234567890",
            "https://www.facebook.com/BossKLOD/posts/1234567890",
        ] {
            let (source, _) = classify(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(source, Source::Facebook, "{raw}");
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
            "https://www.facebook.com/share/r/199xesnx3h/",
        ];
        for raw in single {
            let (_, _, kind) = classify_target(raw).unwrap();
            assert_eq!(kind, TargetKind::Single, "{raw}");
        }

        // Facebook has no profile form at all - nothing can list a page - so
        // its tabs are refused rather than queued as a single post that then
        // fails with a misleading "no video found".
        assert!(matches!(
            classify_target("https://www.facebook.com/nasa/videos"),
            Err(AppError::FacebookProfileUnsupported)
        ));
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
    fn a_bare_youtube_channel_expands_into_all_three_upload_feeds() {
        // Without this, yt-dlp lists the channel's *tabs* - entries with no
        // URL - and the profile card would show nothing to download. Listing
        // only `/videos`, which is what this used to do, silently skipped
        // every Short on channels that post them.
        let (_, url, kind) = classify_target("https://www.youtube.com/@NASA").unwrap();
        assert_eq!(kind, TargetKind::Profile);
        assert_eq!(url.path(), "/@NASA");

        let feeds = youtube_channel_feeds(&url).expect("channel home expands");
        let paths: Vec<&str> = feeds.iter().map(|u| u.path()).collect();
        assert_eq!(paths, ["/@NASA/videos", "/@NASA/shorts", "/@NASA/streams"]);
    }

    #[test]
    fn the_older_channel_url_shapes_expand_too() {
        for raw in [
            "https://www.youtube.com/channel/UCLA_DiR1FfKNvjuUpBHmylQ",
            "https://www.youtube.com/c/NASA",
            "https://www.youtube.com/user/NASAtelevision",
        ] {
            let (_, url, _) = classify_target(raw).unwrap();
            let feeds = youtube_channel_feeds(&url).unwrap_or_else(|| panic!("{raw}"));
            assert_eq!(feeds.len(), 3, "{raw}");
            assert!(feeds[0].path().ends_with("/videos"), "{raw}");
        }
    }

    #[test]
    fn a_link_that_already_names_one_feed_is_not_expanded() {
        // Pasting `/shorts` is a choice; answering it with the long-form
        // uploads as well would ignore what was asked for.
        for raw in [
            "https://www.youtube.com/@NASA/videos",
            "https://www.youtube.com/@NASA/shorts",
            "https://www.youtube.com/playlist?list=PL123",
            "https://www.youtube.com/watch?v=abc",
        ] {
            let (_, url, _) = classify_target(raw).unwrap();
            assert!(youtube_channel_feeds(&url).is_none(), "{raw}");
        }
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
