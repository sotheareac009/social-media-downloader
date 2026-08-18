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
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Facebook => "facebook",
            Source::TikTok => "tiktok",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Source::Facebook => "Facebook",
            Source::TikTok => "TikTok",
        }
    }
}

/// Hosts accepted for each source, compared exactly after stripping a leading
/// `www.` / `m.` / `web.` prefix.
const FACEBOOK_HOSTS: &[&str] = &["facebook.com", "fb.watch", "fb.com", "facebook.net"];
const TIKTOK_HOSTS: &[&str] = &["tiktok.com", "vm.tiktok.com", "vt.tiktok.com"];

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
        Ok((Source::Facebook, parsed))
    } else if TIKTOK_HOSTS.contains(&host) {
        Ok((Source::TikTok, parsed))
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
        for raw in ["https://www.youtube.com/watch?v=abc", "https://vimeo.com/1"] {
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
    fn garbage_is_an_error_not_a_panic() {
        for raw in ["", "   ", "not a url", "https://"] {
            assert!(classify(raw).is_err());
        }
    }
}
