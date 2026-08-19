//! YouTube video upload, via the Data API v3 resumable protocol.
//!
//! Two steps: start a resumable session (metadata as JSON, which returns an
//! upload URL in the `Location` header), then PUT the file bytes to that URL.
//! Resumable rather than multipart because reqwest speaks `multipart/form-data`
//! and YouTube's multipart upload wants `multipart/related` - the resumable
//! path avoids that mismatch and also handles large files.
//!
//! Uploading to the user's own channel needs the `youtube.upload` scope and no
//! app review; the token comes from the stored Google credential.

use crate::errors::{AppError, Result};

const START_URL: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

/// The channel a token will upload to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
}

/// The channel(s) the authenticated account can upload to.
///
/// A normal Google account has exactly one; the upload lands there. An account
/// with several (Brand Accounts) returns each — the one chosen during Google
/// sign-in is what uploads receive. An account with none returns empty, which
/// is worth surfacing: uploading needs a YouTube channel to exist first.
pub async fn my_channels(http: &reqwest::Client, access_token: &str) -> Result<Vec<Channel>> {
    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        items: Vec<Item>,
    }
    #[derive(serde::Deserialize)]
    struct Item {
        id: String,
        snippet: Snippet,
    }
    #[derive(serde::Deserialize)]
    struct Snippet {
        title: String,
        #[serde(default)]
        thumbnails: Option<Thumbs>,
    }
    #[derive(serde::Deserialize)]
    struct Thumbs {
        #[serde(default)]
        default: Option<Thumb>,
    }
    #[derive(serde::Deserialize)]
    struct Thumb {
        url: String,
    }

    let resp = http
        .get("https://www.googleapis.com/youtube/v3/channels")
        .query(&[("part", "snippet"), ("mine", "true")])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(parsed
        .items
        .into_iter()
        .map(|i| Channel {
            id: i.id,
            title: i.snippet.title,
            thumbnail: i.snippet.thumbnails.and_then(|t| t.default).map(|t| t.url),
        })
        .collect())
}

/// Privacy of the uploaded video.
#[derive(Debug, Clone, Copy)]
pub enum Privacy {
    Public,
    Unlisted,
    Private,
}

impl Privacy {
    fn as_str(self) -> &'static str {
        match self {
            Privacy::Public => "public",
            Privacy::Unlisted => "unlisted",
            Privacy::Private => "private",
        }
    }

    pub fn parse(s: &str) -> Privacy {
        match s {
            "public" => Privacy::Public,
            "private" => Privacy::Private,
            _ => Privacy::Unlisted,
        }
    }
}

fn guess_mime(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        _ => "video/*",
    }
}

/// Upload a video. Returns the new video id (a `watch?v=` code).
pub async fn upload_video(
    http: &reqwest::Client,
    access_token: &str,
    file_path: &std::path::Path,
    title: &str,
    description: &str,
    privacy: Privacy,
) -> Result<String> {
    let bytes = std::fs::read(file_path)
        .map_err(|e| AppError::DownloadPath(format!("could not read the video: {e}")))?;
    if bytes.is_empty() {
        return Err(AppError::NoMediaFound);
    }
    let mime = guess_mime(file_path);

    let metadata = serde_json::json!({
        "snippet": {
            "title": if title.trim().is_empty() { "Untitled" } else { title.trim() },
            "description": description,
        },
        "status": { "privacyStatus": privacy.as_str() },
    });

    // Step 1: open the resumable session.
    let start = http
        .post(START_URL)
        .bearer_auth(access_token)
        .header("X-Upload-Content-Type", mime)
        .header("X-Upload-Content-Length", bytes.len().to_string())
        .json(&metadata)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !start.status().is_success() {
        return Err(api_error(start).await);
    }
    let upload_url = start
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or(AppError::MalformedProviderResponse)?;

    // Step 2: upload the bytes in one PUT.
    let done = http
        .put(&upload_url)
        .bearer_auth(access_token)
        .header(reqwest::header::CONTENT_TYPE, mime)
        .body(bytes)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !done.status().is_success() {
        return Err(api_error(done).await);
    }

    #[derive(serde::Deserialize)]
    struct Video {
        id: String,
    }
    let video: Video = done.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(video.id)
}

/// Google API errors carry a readable `error.message`, safe to surface.
async fn api_error(resp: reqwest::Response) -> AppError {
    let status = resp.status().as_u16();
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Option<Inner>,
    }
    #[derive(serde::Deserialize)]
    struct Inner {
        message: Option<String>,
    }
    let msg = resp
        .json::<Envelope>()
        .await
        .ok()
        .and_then(|e| e.error)
        .and_then(|e| e.message);
    match msg {
        Some(m) => AppError::ProviderDenied(m),
        None => AppError::ProviderDenied(format!("YouTube returned HTTP {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mime_is_guessed_from_extension() {
        assert_eq!(guess_mime(Path::new("a.mp4")), "video/mp4");
        assert_eq!(guess_mime(Path::new("a.MOV")), "video/quicktime");
        assert_eq!(guess_mime(Path::new("a.unknown")), "video/*");
    }

    #[test]
    fn privacy_round_trips() {
        assert_eq!(Privacy::parse("public").as_str(), "public");
        assert_eq!(Privacy::parse("private").as_str(), "private");
        // Anything else is the safe default.
        assert_eq!(Privacy::parse("bogus").as_str(), "unlisted");
    }
}
