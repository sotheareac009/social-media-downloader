//! TikTok Content Posting API - upload to the creator's drafts.
//!
//! Uses the *inbox* endpoint, which the `video.upload` scope grants: the video
//! lands in the creator's TikTok drafts and they publish it themselves from the
//! app. Direct posting (`video.publish`) is a different endpoint that also needs
//! a creator-info query and an explicit privacy level, and TikTok forces
//! everything an unaudited app posts to private viewing anyway - so drafts are
//! both simpler and the more useful mode until the app is audited.
//!
//! Flow: initialise -> PUT the bytes (chunked if large) -> poll for status.
//!
//! WHERE THE VIDEO ENDS UP. Not in Drafts, which is the natural assumption and
//! is wrong. TikTok's documentation is explicit that the creator "must click on
//! inbox notifications to continue the editing flow in TikTok and complete the
//! post" - it arrives as a NOTIFICATION in the TikTok app, and the video is
//! only saved once they act on it. Telling a user to look in Drafts sends them
//! hunting for something that is not there.

use std::path::Path;

use crate::errors::{AppError, Result};

const INIT_ENDPOINT: &str = "https://open.tiktokapis.com/v2/post/publish/inbox/video/init/";
const STATUS_ENDPOINT: &str = "https://open.tiktokapis.com/v2/post/publish/status/fetch/";

/// Polling budget. TikTok allows 30 status calls per minute per token, so a
/// 2 second interval stays well inside that while covering slower transcodes.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const POLL_ATTEMPTS: usize = 20;

/// TikTok's documented chunk bounds.
const MIN_CHUNK: u64 = 5 * 1024 * 1024;
const MAX_CHUNK: u64 = 64 * 1024 * 1024;
/// A final chunk may exceed `chunk_size`, but not this.
const MAX_FINAL_CHUNK: u64 = 128 * 1024 * 1024;
const MAX_CHUNKS: u64 = 1000;

/// How a file will be split for upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    pub chunk_size: u64,
    pub total_chunk_count: u64,
    /// Inclusive byte ranges, in order. The last may be larger than
    /// `chunk_size`, which is what TikTok expects rather than a short tail.
    pub ranges: Vec<(u64, u64)>,
}

/// Work out chunking for a file of `size` bytes.
///
/// The rules are TikTok's, and they are not the obvious ones:
///   * every chunk is 5-64 MB, EXCEPT the last, which absorbs the remainder
///     and may be up to 128 MB;
///   * `total_chunk_count` is `size / chunk_size` rounded DOWN, so the tail is
///     merged into the final chunk rather than becoming an extra short one;
///   * a file under 5 MB is sent whole, with `chunk_size` equal to its size.
///
/// Rounding up instead - the intuitive choice - produces a final chunk smaller
/// than the 5 MB minimum, which TikTok rejects.
pub fn plan_chunks(size: u64) -> Result<ChunkPlan> {
    if size == 0 {
        return Err(AppError::Internal("the file is empty".into()));
    }

    // Under the minimum: one chunk that is the whole file.
    if size < MIN_CHUNK {
        return Ok(ChunkPlan {
            chunk_size: size,
            total_chunk_count: 1,
            ranges: vec![(0, size - 1)],
        });
    }

    let chunk_size = size.min(MAX_CHUNK);
    let total = (size / chunk_size).clamp(1, MAX_CHUNKS);

    if size / chunk_size > MAX_CHUNKS {
        return Err(AppError::Internal(
            "the file is too large for TikTok to accept".into(),
        ));
    }

    let mut ranges = Vec::with_capacity(total as usize);
    for i in 0..total {
        let start = i * chunk_size;
        // The last chunk runs to the end of the file, absorbing the remainder.
        let end = if i == total - 1 {
            size - 1
        } else {
            start + chunk_size - 1
        };
        ranges.push((start, end));
    }

    if let Some(&(start, end)) = ranges.last() {
        if end - start + 1 > MAX_FINAL_CHUNK {
            return Err(AppError::Internal(
                "the file cannot be split within TikTok's chunk limits".into(),
            ));
        }
    }

    Ok(ChunkPlan {
        chunk_size,
        total_chunk_count: total,
        ranges,
    })
}

/// Container type from the file extension. TikTok accepts these three.
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        _ => "video/mp4",
    }
}

/// What TikTok reports after processing an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishStatus {
    /// Still transcoding.
    Processing,
    /// The creator has been notified and must finish the post in the app.
    SentToInbox,
    /// Already live on the profile.
    Complete,
    /// Rejected, with TikTok's reason.
    Failed(String),
}

/// Ask TikTok what happened to an upload.
///
/// Accepting the bytes is not the same as accepting the video: format and
/// duration checks run afterwards, and a rejection is only visible here. Without
/// this the app reports success for a video TikTok silently discarded.
pub async fn check_status(
    http: &reqwest::Client,
    access_token: &str,
    publish_id: &str,
) -> Result<PublishStatus> {
    let resp = http
        .post(STATUS_ENDPOINT)
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "publish_id": publish_id }))
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    let status = resp.status();
    let body = resp.bytes().await.map_err(|_| AppError::Network)?;
    if !status.is_success() {
        return Err(AppError::ProviderDenied(format!(
            "TikTok rejected the status request (HTTP {})",
            status.as_u16()
        )));
    }
    crate::auth::providers::tiktok::check_api_error(&body)?;

    #[derive(serde::Deserialize)]
    struct StatusData {
        status: String,
        #[serde(default)]
        fail_reason: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct StatusResponse {
        data: StatusData,
    }

    let parsed: StatusResponse =
        serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

    Ok(classify_status(
        &parsed.data.status,
        parsed.data.fail_reason.as_deref(),
    ))
}

/// Map TikTok's status string onto something the UI can act on.
fn classify_status(status: &str, fail_reason: Option<&str>) -> PublishStatus {
    match status {
        "SEND_TO_USER_INBOX" => PublishStatus::SentToInbox,
        "PUBLISH_COMPLETE" => PublishStatus::Complete,
        "FAILED" => PublishStatus::Failed(explain_failure(fail_reason.unwrap_or(""))),
        // PROCESSING_UPLOAD, PROCESSING_DOWNLOAD, and anything TikTok adds
        // later: treat as still working rather than as an error.
        _ => PublishStatus::Processing,
    }
}

/// Turn TikTok's failure codes into something a person can act on.
fn explain_failure(reason: &str) -> String {
    match reason {
        "file_format_check_failed" => {
            "TikTok rejected the file format. Use an MP4, MOV or WEBM video.".into()
        }
        "duration_check_failed" => {
            "The video is too long or too short for this TikTok account.".into()
        }
        "picture_size_check_failed" => "TikTok rejected the video dimensions.".into(),
        "spam_risk_too_many_posts" => {
            "This account has hit TikTok's 24-hour posting limit. Try again later.".into()
        }
        "internal" => "TikTok had a server problem. Try again.".into(),
        other if other.is_empty() => "TikTok rejected the video without giving a reason.".into(),
        other => format!("TikTok rejected the video: {}", sanitize_reason(other)),
    }
}

fn sanitize_reason(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' '))
        .take(80)
        .collect()
}

/// Upload `path` and wait for TikTok to accept it.
///
/// Returns the publish id. On success the creator gets an inbox NOTIFICATION in
/// the TikTok app - the video is not in Drafts until they act on it.
pub async fn upload_to_inbox(
    http: &reqwest::Client,
    access_token: &str,
    path: &Path,
) -> Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::Internal(format!("could not read the video: {e}")))?;
    let size = bytes.len() as u64;
    let plan = plan_chunks(size)?;

    // --- 1. initialise -----------------------------------------------------
    let init = http
        .post(INIT_ENDPOINT)
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "source_info": {
                "source": "FILE_UPLOAD",
                "video_size": size,
                "chunk_size": plan.chunk_size,
                "total_chunk_count": plan.total_chunk_count,
            }
        }))
        .send()
        .await
        .map_err(|_| AppError::Network)?;

    let status = init.status();
    let body = init.bytes().await.map_err(|_| AppError::Network)?;
    if !status.is_success() {
        return Err(AppError::ProviderDenied(format!(
            "TikTok rejected the upload request (HTTP {})",
            status.as_u16()
        )));
    }
    // TikTok answers failures with HTTP 200 and an error object.
    crate::auth::providers::tiktok::check_api_error(&body)?;

    #[derive(serde::Deserialize)]
    struct InitData {
        publish_id: String,
        upload_url: String,
    }
    #[derive(serde::Deserialize)]
    struct InitResponse {
        data: InitData,
    }

    let init: InitResponse =
        serde_json::from_slice(&body).map_err(|_| AppError::MalformedProviderResponse)?;

    // --- 2. send the bytes -------------------------------------------------
    let mime = mime_for(path);
    for (start, end) in &plan.ranges {
        let slice = &bytes[*start as usize..=*end as usize];
        let resp = http
            .put(&init.data.upload_url)
            .header("Content-Type", mime)
            .header("Content-Length", slice.len().to_string())
            .header(
                "Content-Range",
                format!("bytes {start}-{end}/{size}"),
            )
            .body(slice.to_vec())
            .send()
            .await
            .map_err(|_| AppError::Network)?;

        if !resp.status().is_success() {
            return Err(AppError::ProviderDenied(format!(
                "TikTok rejected a chunk of the upload (HTTP {})",
                resp.status().as_u16()
            )));
        }
    }

    // --- 3. wait for TikTok to actually accept it ---------------------------
    // Uploading the bytes only means they arrived. Format and duration checks
    // run afterwards, and a rejection appears nowhere else.
    for _ in 0..POLL_ATTEMPTS {
        match check_status(http, access_token, &init.data.publish_id).await? {
            PublishStatus::SentToInbox | PublishStatus::Complete => {
                return Ok(init.data.publish_id)
            }
            PublishStatus::Failed(reason) => return Err(AppError::ProviderDenied(reason)),
            PublishStatus::Processing => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }

    // Still PROCESSING_UPLOAD when the budget ran out.
    //
    // Reporting success here would be a lie the user cannot check: no inbox
    // notification ever arrives, nothing lands in Drafts, and nothing appears
    // on the profile. Measured against a real sandbox app, the status stayed
    // PROCESSING_UPLOAD for over three minutes with the whole file received
    // (`uploaded_bytes` equalled the file size) and no failure reason - TikTok
    // simply never finishes the job for an unaudited sandbox client.
    Err(AppError::ProviderDenied(stalled_message()))
}

/// Explain a stalled upload, naming the sandbox when that is the likely cause.
fn stalled_message() -> String {
    let sandbox = crate::config::read("TIKTOK_CLIENT_KEY")
        .is_some_and(|k| k.trim().starts_with("sbaw"));

    if sandbox {
        "TikTok received the whole video but never finished processing it. This app is \
         registered as a TikTok SANDBOX client, and sandbox clients do not complete \
         uploads - no inbox notification is sent. Posting needs a production app that \
         has passed TikTok's audit."
            .into()
    } else {
        "TikTok received the whole video but has not finished processing it yet. It may \
         still appear in your TikTok inbox shortly."
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_file_goes_as_one_whole_chunk() {
        // Under 5 MB, chunk_size must equal the file size rather than the
        // 5 MB minimum, which TikTok would reject.
        let plan = plan_chunks(1_000_000).unwrap();
        assert_eq!(plan.chunk_size, 1_000_000);
        assert_eq!(plan.total_chunk_count, 1);
        assert_eq!(plan.ranges, vec![(0, 999_999)]);
    }

    #[test]
    fn a_file_under_the_max_is_a_single_chunk() {
        let size = 30 * 1024 * 1024;
        let plan = plan_chunks(size).unwrap();
        assert_eq!(plan.total_chunk_count, 1);
        assert_eq!(plan.ranges, vec![(0, size - 1)]);
    }

    /// The rule that trips people up: the count rounds DOWN and the remainder
    /// joins the final chunk. Rounding up leaves a tail under the 5 MB minimum.
    #[test]
    fn the_remainder_is_absorbed_into_the_final_chunk() {
        let size = 150 * 1024 * 1024; // 2 x 64 MB + 22 MB
        let plan = plan_chunks(size).unwrap();

        assert_eq!(plan.chunk_size, MAX_CHUNK);
        assert_eq!(plan.total_chunk_count, 2, "count must round down, not up");

        let (last_start, last_end) = *plan.ranges.last().unwrap();
        let last_len = last_end - last_start + 1;
        assert!(last_len > plan.chunk_size, "the tail was not absorbed");
        assert!(last_len <= MAX_FINAL_CHUNK);
    }

    #[test]
    fn ranges_are_contiguous_and_cover_the_whole_file() {
        for size in [
            1,
            MIN_CHUNK - 1,
            MIN_CHUNK,
            MAX_CHUNK,
            MAX_CHUNK + 1,
            150 * 1024 * 1024,
            700 * 1024 * 1024,
        ] {
            let plan = plan_chunks(size).unwrap();
            assert_eq!(plan.ranges.first().unwrap().0, 0, "size {size}");
            assert_eq!(plan.ranges.last().unwrap().1, size - 1, "size {size}");
            for pair in plan.ranges.windows(2) {
                assert_eq!(pair[1].0, pair[0].1 + 1, "gap or overlap at size {size}");
            }
            assert_eq!(plan.ranges.len() as u64, plan.total_chunk_count);
        }
    }

    #[test]
    fn every_non_final_chunk_is_within_tiktoks_bounds() {
        let plan = plan_chunks(700 * 1024 * 1024).unwrap();
        let n = plan.ranges.len();
        for (i, &(start, end)) in plan.ranges.iter().enumerate() {
            let len = end - start + 1;
            if i + 1 < n {
                assert!((MIN_CHUNK..=MAX_CHUNK).contains(&len), "chunk {i} is {len}");
            } else {
                assert!(len >= MIN_CHUNK && len <= MAX_FINAL_CHUNK);
            }
        }
    }

    /// A stalled upload must not be reported as success: the user has no way to
    /// discover it failed, since nothing appears anywhere in the TikTok app.
    #[test]
    fn a_stalled_upload_explains_itself() {
        let msg = stalled_message();
        assert!(!msg.is_empty());
        assert!(
            msg.contains("processing"),
            "the message should say what stalled: {msg}"
        );
    }

    #[test]
    fn status_values_map_to_outcomes() {
        assert_eq!(classify_status("SEND_TO_USER_INBOX", None), PublishStatus::SentToInbox);
        assert_eq!(classify_status("PUBLISH_COMPLETE", None), PublishStatus::Complete);
        assert_eq!(classify_status("PROCESSING_UPLOAD", None), PublishStatus::Processing);
        // An unknown status must not read as a failure.
        assert_eq!(classify_status("SOMETHING_NEW", None), PublishStatus::Processing);
    }

    #[test]
    fn a_failure_carries_an_actionable_reason() {
        let PublishStatus::Failed(msg) =
            classify_status("FAILED", Some("file_format_check_failed"))
        else {
            panic!("expected a failure");
        };
        assert!(msg.contains("MP4"), "not actionable: {msg}");

        // An unrecognised code is still surfaced, sanitised.
        let PublishStatus::Failed(msg) = classify_status("FAILED", Some("weird_new_code<>"))
        else {
            panic!("expected a failure");
        };
        assert!(msg.contains("weird_new_code"));
        assert!(!msg.contains('<'), "unsanitised reason reached the UI");

        // A failure with no reason still says something.
        let PublishStatus::Failed(msg) = classify_status("FAILED", None) else {
            panic!("expected a failure");
        };
        assert!(!msg.is_empty());
    }

    #[test]
    fn an_empty_file_is_refused() {
        assert!(plan_chunks(0).is_err());
    }

    #[test]
    fn container_type_follows_the_extension() {
        assert_eq!(mime_for(Path::new("a.mp4")), "video/mp4");
        assert_eq!(mime_for(Path::new("a.MOV")), "video/quicktime");
        assert_eq!(mime_for(Path::new("a.webm")), "video/webm");
        // Unknown extensions default to mp4 rather than failing the upload.
        assert_eq!(mime_for(Path::new("a.bin")), "video/mp4");
    }
}
