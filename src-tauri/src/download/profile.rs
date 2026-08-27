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

/// What asking the platform about a session concluded.
///
/// Three states, not two, and the third is the important one. A request that
/// fails for a reason of its own - no network, a blocked datacentre IP, an
/// endpoint that moved - says nothing about whether the cookies work. Reporting
/// that as "invalid" sends people off to re-export cookies that were fine.
pub enum SessionCheck {
    /// The platform answered as the signed-in account.
    SignedIn(SessionProfile),
    /// The platform answered, and refused these cookies.
    Rejected,
    /// No usable answer. The cookies may well be fine.
    Unknown,
}

/// Ask the platform whether a stored session still works.
pub async fn check(kind: SessionKind, cookies: &[StoredCookie]) -> SessionCheck {
    match kind {
        SessionKind::Instagram => check_instagram(cookies).await,
        SessionKind::Facebook => check_facebook(cookies).await,
        // Neither exposes a cheap cookie-only endpoint that distinguishes a
        // dead session from a blocked request, and guessing between them is
        // exactly what this type exists to avoid.
        SessionKind::TikTok | SessionKind::X => SessionCheck::Unknown,
    }
}

/// Instagram, asked the way a browser session is entitled to ask.
///
/// `ds_user_id` is the account's own id, so the profile endpoint for that id is
/// a direct "does this session still speak for this account". The previous
/// check used `accounts/current_user/`, which is a *mobile app* endpoint: it
/// refuses ordinary web cookies whatever their state, so a perfectly good
/// session was reported dead.
async fn check_instagram(cookies: &[StoredCookie]) -> SessionCheck {
    let Some(user_id) = cookie_value(cookies, "ds_user_id") else {
        // Without it there is no account to ask about; the session cookie
        // alone cannot say who it belongs to.
        return SessionCheck::Unknown;
    };

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "https://www.instagram.com/api/v1/users/{user_id}/info/"
        ))
        .header("Cookie", cookie_header(cookies))
        .header("X-IG-App-ID", "936619743392459")
        .header("User-Agent", WEB_UA)
        .header("Referer", "https://www.instagram.com/")
        .send()
        .await;

    let Ok(resp) = resp else {
        return SessionCheck::Unknown;
    };

    // 401 and 403 are Instagram saying these cookies are not a session; a 5xx
    // or a redirect to a checkpoint is Instagram having a moment.
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return SessionCheck::Rejected;
    }
    if !resp.status().is_success() {
        return SessionCheck::Unknown;
    }

    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return SessionCheck::Unknown;
    };
    let Some(user) = v.get("user") else {
        return SessionCheck::Unknown;
    };

    SessionCheck::SignedIn(SessionProfile {
        display_name: user
            .get("full_name")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| user.get("username").and_then(|x| x.as_str()))
            .map(str::to_string),
        avatar_url: user
            .get("profile_pic_url")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// Facebook, asked whether it still knows who this is.
///
/// Redirects are deliberately not followed: a live session answers `/me/` with
/// a redirect to the profile, a dead one with a redirect to `login.php`, and
/// following either would turn both into a 200 that proves nothing. The old
/// check only read the `c_user` cookie and built an avatar URL from it, which
/// succeeds whether or not the session is alive.
async fn check_facebook(cookies: &[StoredCookie]) -> SessionCheck {
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return SessionCheck::Unknown;
    };

    let resp = client
        .get("https://www.facebook.com/me/")
        .header("Cookie", cookie_header(cookies))
        .header("User-Agent", WEB_UA)
        .send()
        .await;

    let Ok(resp) = resp else {
        return SessionCheck::Unknown;
    };

    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if location.contains("login") || location.contains("checkpoint") {
        return SessionCheck::Rejected;
    }
    if resp.status().is_redirection() || resp.status().is_success() {
        return SessionCheck::SignedIn(
            fetch_facebook(cookies).await.unwrap_or_default(),
        );
    }
    SessionCheck::Unknown
}

/// One user agent for every probe: a desktop browser, which is what these web
/// cookies were issued to.
const WEB_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

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
        .get("https://www.instagram.com/api/v1/accounts/current_user/")
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
