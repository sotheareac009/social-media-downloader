//! Post a photo or video to X (Twitter) via the v2 media-upload API.
//!
//! X's v2 media upload is a small REST sequence (not the old v1.1 `command=`
//! form). Each step is its own endpoint under `/2/media/upload`:
//!
//!   initialize   POST /2/media/upload/initialize          (JSON metadata)
//!   append       POST /2/media/upload/{id}/append         (multipart chunk)
//!   finalize     POST /2/media/upload/{id}/finalize       (starts processing)
//!   status       GET  /2/media/upload/{id}                (poll for video)
//!
//! Then a tweet is created referencing the media id. Media upload needs the
//! `media.write` scope; tweeting needs `tweet.write`.

use std::path::Path;

use crate::errors::{AppError, Result};

const MEDIA_BASE: &str = "https://api.x.com/2/media/upload";
const TWEET_ENDPOINT: &str = "https://api.twitter.com/2/tweets";

/// 4 MB — comfortably under X's 5 MB per-APPEND ceiling.
const CHUNK: usize = 4 * 1024 * 1024;
const POLL_ATTEMPTS: usize = 30;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Processing status X returns for video, both from finalize and status.
#[derive(serde::Deserialize)]
struct Processing {
    state: String,
    #[serde(default)]
    check_after_secs: Option<u64>,
}

/// Upload `file_path` and post it with `caption`. Returns the new tweet id.
pub async fn post_media(
    http: &reqwest::Client,
    access_token: &str,
    file_path: &Path,
    caption: &str,
) -> Result<String> {
    let bytes = std::fs::read(file_path)
        .map_err(|e| AppError::DownloadPath(format!("could not read the file: {e}")))?;
    if bytes.is_empty() {
        return Err(AppError::NoMediaFound);
    }
    let (mime, category) = classify(file_path);

    let media_id = initialize(http, access_token, bytes.len(), &mime, category).await?;
    append_all(http, access_token, &media_id, &bytes).await?;
    let processing = finalize(http, access_token, &media_id).await?;
    if let Some(p) = processing {
        if p.state != "succeeded" {
            wait_for_processing(http, access_token, &media_id).await?;
        }
    }
    create_tweet(http, access_token, caption, &media_id).await
}

fn classify(path: &Path) -> (String, &'static str) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => ("image/jpeg".into(), "tweet_image"),
        "png" => ("image/png".into(), "tweet_image"),
        "webp" => ("image/webp".into(), "tweet_image"),
        "gif" => ("image/gif".into(), "tweet_gif"),
        "mov" => ("video/quicktime".into(), "tweet_video"),
        _ => ("video/mp4".into(), "tweet_video"),
    }
}

/// INIT: JSON metadata, returns the media id.
async fn initialize(
    http: &reqwest::Client,
    token: &str,
    total_bytes: usize,
    mime: &str,
    category: &str,
) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: Data,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        id: String,
    }

    let body = serde_json::json!({
        "media_category": category,
        "media_type": mime,
        "total_bytes": total_bytes,
    });
    let resp = http
        .post(format!("{MEDIA_BASE}/initialize"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| AppError::Network)?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|_| AppError::Network)?;
    if !status.is_success() {
        return Err(AppError::ProviderDenied(describe(status.as_u16(), &bytes, "media initialize")));
    }
    let env: Envelope =
        serde_json::from_slice(&bytes).map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(env.data.id)
}

/// APPEND: one multipart request per chunk.
async fn append_all(http: &reqwest::Client, token: &str, media_id: &str, bytes: &[u8]) -> Result<()> {
    for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
        let form = reqwest::multipart::Form::new()
            .text("segment_index", i.to_string())
            .part("media", reqwest::multipart::Part::bytes(chunk.to_vec()));
        let resp = http
            .post(format!("{MEDIA_BASE}/{media_id}/append"))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .map_err(|_| AppError::Network)?;
        let st = resp.status();
        if !st.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(AppError::ProviderDenied(describe(st.as_u16(), &body, "media chunk")));
        }
    }
    Ok(())
}

/// FINALIZE: returns processing info when X needs to transcode (video).
async fn finalize(http: &reqwest::Client, token: &str, media_id: &str) -> Result<Option<Processing>> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<Data>,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(default)]
        processing_info: Option<Processing>,
    }

    let resp = http
        .post(format!("{MEDIA_BASE}/{media_id}/finalize"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| AppError::Network)?;
    let st = resp.status();
    let body = resp.bytes().await.map_err(|_| AppError::Network)?;
    if !st.is_success() {
        return Err(AppError::ProviderDenied(describe(st.as_u16(), &body, "media finalize")));
    }
    let env: Envelope = serde_json::from_slice(&body).unwrap_or(Envelope { data: None });
    Ok(env.data.and_then(|d| d.processing_info))
}

/// Poll status until X finishes processing the video.
async fn wait_for_processing(http: &reqwest::Client, token: &str, media_id: &str) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<Data>,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(default)]
        processing_info: Option<Processing>,
    }

    for _ in 0..POLL_ATTEMPTS {
        let resp = http
            .get(format!("{MEDIA_BASE}/{media_id}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| AppError::Network)?;
        let st = resp.status();
        let body = resp.bytes().await.map_err(|_| AppError::Network)?;
        if !st.is_success() {
            return Err(AppError::ProviderDenied(describe(st.as_u16(), &body, "media status")));
        }
        let env: Envelope = serde_json::from_slice(&body).unwrap_or(Envelope { data: None });
        match env.data.and_then(|d| d.processing_info) {
            None => return Ok(()),
            Some(p) => match p.state.as_str() {
                "succeeded" => return Ok(()),
                "failed" => {
                    return Err(AppError::ProviderDenied("X could not process the video".into()))
                }
                _ => {
                    let wait = p.check_after_secs.map(std::time::Duration::from_secs);
                    tokio::time::sleep(wait.unwrap_or(POLL_INTERVAL)).await;
                }
            },
        }
    }
    Err(AppError::ProviderDenied(
        "X is still processing the video — try again in a moment".into(),
    ))
}

/// Create the tweet carrying the uploaded media.
async fn create_tweet(
    http: &reqwest::Client,
    token: &str,
    caption: &str,
    media_id: &str,
) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: TweetData,
    }
    #[derive(serde::Deserialize)]
    struct TweetData {
        id: String,
    }

    let mut body = serde_json::json!({ "media": { "media_ids": [media_id] } });
    let text = caption.trim();
    if !text.is_empty() {
        body["text"] = serde_json::Value::String(text.chars().take(280).collect());
    }

    let resp = http
        .post(TWEET_ENDPOINT)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| AppError::Network)?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|_| AppError::Network)?;
    if !status.is_success() {
        return Err(AppError::ProviderDenied(describe(status.as_u16(), &bytes, "post")));
    }
    let env: Envelope =
        serde_json::from_slice(&bytes).map_err(|_| AppError::MalformedProviderResponse)?;
    Ok(env.data.id)
}

/// Pull a human-readable reason out of X's error body so a 4xx says *why*.
/// Handles the v2 `{title,detail,errors:[{message}]}` shape.
fn describe(status: u16, body: &[u8], what: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Err2 {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        errors: Option<Vec<ErrItem>>,
    }
    #[derive(serde::Deserialize)]
    struct ErrItem {
        #[serde(default)]
        message: Option<String>,
    }
    let detail = serde_json::from_slice::<Err2>(body).ok().and_then(|e| {
        e.errors
            .and_then(|v| v.into_iter().next())
            .and_then(|i| i.message)
            .or(e.detail)
            .or(e.title)
    });
    let clean = detail
        .map(|d| d.chars().filter(|c| *c != '\n' && *c != '\r').take(200).collect::<String>())
        .filter(|d| !d.trim().is_empty());
    match clean {
        Some(d) => format!("X {what} failed (HTTP {status}): {d}"),
        None => format!("X {what} returned HTTP {status}"),
    }
}
