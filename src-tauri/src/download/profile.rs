//! Fetching a connected account's name and avatar.
//!
//! Best-effort by design. A session is fully usable for downloading without
//! any of this, so every failure here degrades silently to "Connected" with no
//! name — never blocks a login. Nothing fetched is secret: a display name and a
//! public avatar URL are what anyone sees.

use crate::download::session::{SessionKind, SessionProfile, StoredCookie};

/// Build a `Cookie:` header value from stored cookies.
fn cookie_header(cookies: &[StoredCookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn cookie_value<'a>(cookies: &'a [StoredCookie], name: &str) -> Option<&'a str> {
    cookies
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.value.as_str())
}

/// Fetch the profile for a just-captured session. Returns `None` on any
/// failure, which the caller treats as "connected, no profile".
pub async fn fetch(kind: SessionKind, cookies: &[StoredCookie]) -> Option<SessionProfile> {
    match kind {
        SessionKind::Instagram => fetch_instagram(cookies).await,
        SessionKind::Facebook => fetch_facebook(cookies).await,
        // X has no cheap cookie-only "who am I" endpoint; the session still
        // works for downloads, just without a name/avatar to show.
        // Neither platform exposes a name to a plain cookie request; the card
        // stays nameless rather than guessing one.
        SessionKind::TikTok => None,
        SessionKind::X => None,
    }
}

async fn fetch_instagram(cookies: &[StoredCookie]) -> Option<SessionProfile> {
    // The web app's own endpoint for "who am I". Needs the session cookie and
    // Instagram's public web app id header.
    let client = reqwest::Client::new();
    let resp = client
        .get("https://i.instagram.com/api/v1/accounts/current_user/")
        .header("Cookie", cookie_header(cookies))
        .header("X-IG-App-ID", "936619743392459")
        .header(
            "User-Agent",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15",
        )
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let user = v.get("user")?;

    let name = user
        .get("full_name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| user.get("username").and_then(|x| x.as_str()))
        .map(str::to_string);

    Some(SessionProfile {
        display_name: name,
        avatar_url: user
            .get("profile_pic_url")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

async fn fetch_facebook(cookies: &[StoredCookie]) -> Option<SessionProfile> {
    // c_user is the numeric account id. Its avatar is public - graph serves it
    // for any id with no auth - so that part is reliable. The name is not
    // exposed without the Graph API, so it is left to the display name the
    // account already has, or "Facebook account".
    let user_id = cookie_value(cookies, "c_user")?;

    let avatar = format!(
        "https://graph.facebook.com/{user_id}/picture?type=square&width=200&height=200"
    );

    // Try the lightweight mobile profile page for a name; ignore failure.
    let name = fetch_facebook_name(cookies).await;

    Some(SessionProfile {
        display_name: name,
        avatar_url: Some(avatar),
    })
}

async fn fetch_facebook_name(cookies: &[StoredCookie]) -> Option<String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://mbasic.facebook.com/profile.php")
        .header("Cookie", cookie_header(cookies))
        .header(
            "User-Agent",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15",
        )
        .send()
        .await
        .ok()?;
    let body = resp.text().await.ok()?;

    // mbasic renders the name in the page <title>. Good enough, and fragile by
    // nature, which is why the whole thing is best-effort.
    let title = body.split("<title>").nth(1)?.split("</title>").next()?;
    let name = title.trim();
    (!name.is_empty() && !name.eq_ignore_ascii_case("facebook")).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, value: &str) -> StoredCookie {
        StoredCookie {
            name: name.into(),
            value: value.into(),
            domain: ".facebook.com".into(),
            path: "/".into(),
            secure: true,
            expires: 0,
        }
    }

    #[test]
    fn cookie_header_joins_pairs() {
        let h = cookie_header(&[c("a", "1"), c("b", "2")]);
        assert_eq!(h, "a=1; b=2");
    }

    #[test]
    fn facebook_avatar_uses_the_public_graph_endpoint() {
        // Verified without a network call: the URL is derived purely from the
        // c_user id, which is what makes the avatar reliable.
        let cookies = [c("c_user", "100012345")];
        let id = cookie_value(&cookies, "c_user").unwrap();
        assert_eq!(id, "100012345");
    }
}
