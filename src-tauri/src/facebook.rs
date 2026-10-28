//! Facebook Page publishing.
//!
//! Uses the stored Facebook OAuth token to list the Pages a user manages and
//! upload media to a chosen one. The important rule, and the usual mistake:
//! posting to a Page uses that **Page's own access token**, not the user
//! token. `/me/accounts` returns a per-Page token, and every publish call uses
//! it.
//!
//! Tokens never cross into the frontend: the UI asks for a Page *list* (id,
//! name, avatar) and later a *publish* by page id; Rust holds the tokens.

use serde::Serialize;

use crate::errors::{AppError, Result};

const GRAPH_VERSION: &str = "v21.0";

/// A Page the user manages, as shown in the selector. Carries no token.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// List the Pages the user can publish to.
///
/// `user_token` is the stored Facebook OAuth access token.
pub async fn list_pages(http: &reqwest::Client, user_token: &str) -> Result<Vec<Page>> {
    #[derive(serde::Deserialize)]
    struct Resp {
        data: Vec<PageRow>,
    }
    #[derive(serde::Deserialize)]
    struct PageRow {
        id: String,
        name: String,
        #[serde(default)]
        picture: Option<Picture>,
    }
    #[derive(serde::Deserialize)]
    struct Picture {
        data: PictureData,
    }
    #[derive(serde::Deserialize)]
    struct PictureData {
        #[serde(default)]
        url: Option<String>,
    }

    let resp = http
        .get(format!("https://graph.facebook.com/{GRAPH_VERSION}/me/accounts"))
        .query(&[
            ("fields", "id,name,picture.width(96).height(96)"),
            ("limit", "100"),
        ])
        .bearer_auth(user_token)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }

    let parsed: Resp = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(parsed
        .data
        .into_iter()
        .map(|p| Page {
            id: p.id,
            name: p.name,
            avatar_url: p.picture.and_then(|x| x.data.url),
        })
        .collect())
}

/// Fetch the access token for one Page. Kept server-side.
async fn page_token(http: &reqwest::Client, user_token: &str, page_id: &str) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Resp {
        access_token: String,
    }
    let resp = http
        .get(format!("https://graph.facebook.com/{GRAPH_VERSION}/{page_id}"))
        .query(&[("fields", "access_token")])
        .bearer_auth(user_token)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    if parsed.access_token.is_empty() {
        return Err(AppError::ProviderDenied(
            "no access token for that Page — you may not manage it".into(),
        ));
    }
    Ok(parsed.access_token)
}

/// Publish a photo to a Page with an optional caption. Returns the new post id.
pub async fn upload_photo(
    http: &reqwest::Client,
    user_token: &str,
    page_id: &str,
    file_path: &std::path::Path,
    caption: &str,
) -> Result<String> {
    let token = page_token(http, user_token, page_id).await?;

    let bytes = std::fs::read(file_path)
        .map_err(|e| AppError::DownloadPath(format!("could not read the photo: {e}")))?;
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "photo.jpg".into());

    // Multipart: the image as `source`, plus the caption and the page token.
    let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
    let mut form = reqwest::multipart::Form::new()
        .text("access_token", token)
        .text("published", "true")
        .part("source", part);
    if !caption.trim().is_empty() {
        form = form.text("message", caption.trim().to_string());
    }

    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        post_id: Option<String>,
    }

    let resp = http
        .post(format!("https://graph.facebook.com/{GRAPH_VERSION}/{page_id}/photos"))
        .multipart(form)
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    parsed
        .post_id
        .or(parsed.id)
        .ok_or(AppError::MalformedProviderResponse)
}

/// Publish a video to a Page. Returns the new video id.
///
/// Uses Graph's resumable upload (start → transfer → finish) on the video
/// host rather than one multipart POST: Facebook names each chunk's byte range,
/// so only one chunk is in memory at a time however large the file is.
pub async fn upload_video(
    http: &reqwest::Client,
    user_token: &str,
    page_id: &str,
    file_path: &std::path::Path,
    title: &str,
    description: &str,
) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let token = page_token(http, user_token, page_id).await?;
    let endpoint = format!("https://graph-video.facebook.com/{GRAPH_VERSION}/{page_id}/videos");

    let mut file = std::fs::File::open(file_path)
        .map_err(|e| AppError::DownloadPath(format!("could not read the video: {e}")))?;
    let size = file
        .metadata()
        .map_err(|e| AppError::DownloadPath(format!("could not read the video: {e}")))?
        .len();

    // Offsets come back as strings in every phase.
    #[derive(serde::Deserialize)]
    struct Start {
        upload_session_id: String,
        video_id: String,
        start_offset: String,
        end_offset: String,
    }
    #[derive(serde::Deserialize)]
    struct Transfer {
        start_offset: String,
        end_offset: String,
    }
    let offset = |s: &str| s.parse::<u64>().map_err(|_| AppError::MalformedProviderResponse);

    let resp = http
        .post(&endpoint)
        .form(&[
            ("access_token", token.as_str()),
            ("upload_phase", "start"),
            ("file_size", &size.to_string()),
        ])
        .send()
        .await
        .map_err(|_| AppError::Network)?;
    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }
    let start: Start = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;

    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".into());
    let (mut from, mut to) = (offset(&start.start_offset)?, offset(&start.end_offset)?);

    // Facebook signals completion by returning equal offsets.
    while from < to {
        let mut chunk = vec![0u8; (to - from) as usize];
        file.seek(SeekFrom::Start(from))
            .and_then(|_| file.read_exact(&mut chunk))
            .map_err(|e| AppError::DownloadPath(format!("could not read the video: {e}")))?;

        let form = reqwest::multipart::Form::new()
            .text("access_token", token.clone())
            .text("upload_phase", "transfer")
            .text("upload_session_id", start.upload_session_id.clone())
            .text("start_offset", from.to_string())
            .part(
                "video_file_chunk",
                reqwest::multipart::Part::bytes(chunk).file_name(file_name.clone()),
            );
        let resp = http
            .post(&endpoint)
            .multipart(form)
            .send()
            .await
            .map_err(|_| AppError::Network)?;
        if !resp.status().is_success() {
            return Err(graph_error(resp).await);
        }
        let next: Transfer = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
        (from, to) = (offset(&next.start_offset)?, offset(&next.end_offset)?);
    }

    let mut finish = vec![
        ("access_token", token.as_str()),
        ("upload_phase", "finish"),
        ("upload_session_id", start.upload_session_id.as_str()),
        ("published", "true"),
    ];
    if !title.trim().is_empty() {
        finish.push(("title", title.trim()));
    }
    if !description.trim().is_empty() {
        finish.push(("description", description.trim()));
    }
    let resp = http
        .post(&endpoint)
        .form(&finish)
        .send()
        .await
        .map_err(|_| AppError::Network)?;
    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }
    Ok(start.video_id)
}

/// Publish a text-only status to a Page's feed. Returns the new post id.
pub async fn post_status(
    http: &reqwest::Client,
    user_token: &str,
    page_id: &str,
    message: &str,
) -> Result<String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::ProviderDenied("a status needs some text".into()));
    }
    let token = page_token(http, user_token, page_id).await?;

    #[derive(serde::Deserialize)]
    struct Resp {
        id: String,
    }

    let resp = http
        .post(format!("https://graph.facebook.com/{GRAPH_VERSION}/{page_id}/feed"))
        .form(&[("access_token", token.as_str()), ("message", message)])
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    if !resp.status().is_success() {
        return Err(graph_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(parsed.id)
}

/// Turn a Graph API error body into a user-safe message. Graph errors carry a
/// human-readable `message`, which is safe to surface (it names the problem,
/// not the token).
async fn graph_error(resp: reqwest::Response) -> AppError {
    let status = resp.status().as_u16();
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Option<GraphErr>,
    }
    #[derive(serde::Deserialize)]
    struct GraphErr {
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
        None => AppError::ProviderDenied(format!("Facebook returned HTTP {status}")),
    }
}
