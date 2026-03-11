//! Douyin content publishing helpers.
//!
//! Implements a two-step video publish flow:
//!   1. `upload_video()` — upload the raw file, get back a video_id.
//!   2. `create_video()` — publish the video with text caption.

use serde_json::{Value, json};
use tracing::{debug, info};

use crate::error::{AdapterError, Result};

/// Upload a video file (step 1 of 2).
///
/// Returns the encrypted `video_id` needed for `create_video`.
///
/// # Endpoint
/// `POST https://open.douyin.com/api/douyin/v1/video/upload_video/?open_id={open_id}`
pub async fn upload_video(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    file_path: &str,
) -> Result<String> {
    debug!(file_path, "uploading Douyin video");

    let bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: format!("read file '{file_path}': {e}"),
        })?;

    let file_name = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("video.mp4")
        .to_string();

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name)
        .mime_str("video/mp4")
        .map_err(|e| AdapterError::Internal(format!("multipart mime: {e}")))?;

    let form = reqwest::multipart::Form::new().part("video", part);

    let url = format!("{api_base}/api/douyin/v1/video/upload_video/?open_id={open_id}");

    let resp: Value = client
        .post(&url)
        .header("access-token", access_token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: format!("upload_video request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: format!("upload_video parse: {e}"),
        })?;

    check_douyin_error(&resp, "douyin_publish_video")?;

    let video_id = resp
        .get("data")
        .and_then(|d| d.get("video"))
        .and_then(|v| v.get("video_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: "missing video_id in upload response".to_string(),
        })?
        .to_string();

    info!(video_id, "Douyin video uploaded successfully");
    Ok(video_id)
}

/// Create/publish a video (step 2 of 2).
///
/// # Endpoint
/// `POST https://open.douyin.com/api/douyin/v1/video/create_video/?open_id={open_id}`
pub async fn create_video(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    video_id: &str,
    text: &str,
) -> Result<Value> {
    debug!(video_id, "creating Douyin video post");

    let url = format!("{api_base}/api/douyin/v1/video/create_video/?open_id={open_id}");
    let body = json!({
        "video_id": video_id,
        "text": text
    });

    let resp: Value = client
        .post(&url)
        .header("access-token", access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: format!("create_video request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_publish_video".to_string(),
            reason: format!("create_video parse: {e}"),
        })?;

    check_douyin_error(&resp, "douyin_publish_video")?;
    info!(video_id, "Douyin video created successfully");
    Ok(resp)
}

/// Get the authenticated user's video list.
///
/// # Endpoint
/// `GET https://open.douyin.com/api/douyin/v1/video/list/?open_id={open_id}&count={count}`
pub async fn get_video_list(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    count: u32,
) -> Result<Value> {
    debug!(open_id, count, "fetching Douyin video list");

    let url = format!(
        "{api_base}/api/douyin/v1/video/list/?open_id={open_id}&count={count}"
    );

    let resp: Value = client
        .get(&url)
        .header("access-token", access_token)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_get_video_list".to_string(),
            reason: format!("get_video_list request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_get_video_list".to_string(),
            reason: format!("get_video_list parse: {e}"),
        })?;

    check_douyin_error(&resp, "douyin_get_video_list")?;
    Ok(resp)
}

/// Get video stats (plays, likes, comments, shares) for one or more item IDs.
///
/// # Endpoint
/// `POST https://open.douyin.com/api/douyin/v1/data/video/base/?open_id={open_id}`
pub async fn get_video_stats(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    item_ids: &[&str],
) -> Result<Value> {
    debug!(open_id, count = item_ids.len(), "fetching Douyin video stats");

    let url = format!("{api_base}/api/douyin/v1/data/video/base/?open_id={open_id}");
    let body = json!({ "item_ids": item_ids });

    let resp: Value = client
        .post(&url)
        .header("access-token", access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_get_video_stats".to_string(),
            reason: format!("get_video_stats request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "douyin_get_video_stats".to_string(),
            reason: format!("get_video_stats parse: {e}"),
        })?;

    check_douyin_error(&resp, "douyin_get_video_stats")?;
    Ok(resp)
}

/// Check a Douyin API response for error codes.
fn check_douyin_error(resp: &Value, tool: &str) -> Result<()> {
    if let Some(data) = resp.get("data") {
        if let Some(code) = data.get("error_code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let description = data
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(AdapterError::ExecutionFailed {
                    tool_name: tool.to_string(),
                    reason: format!("Douyin API error {code}: {description}"),
                });
            }
        }
    }

    let message = resp.get("message").and_then(|v| v.as_str()).unwrap_or("");
    if !message.is_empty() && message != "success" {
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool.to_string(),
            reason: format!("Douyin API error: {message}"),
        });
    }

    Ok(())
}
