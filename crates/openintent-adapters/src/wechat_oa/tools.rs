//! WeChat OA tool implementations.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};

use super::api::check_wechat_error;
use super::types::{ImageContent, ImageMessage, TextContent, TextMessage};

macro_rules! required_str {
    ($params:expr, $field:expr, $tool:expr) => {
        $params
            .get($field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: $tool.to_string(),
                reason: format!("missing required field `{}`", $field),
            })?
    };
}

macro_rules! exec_err {
    ($tool:expr, $e:expr) => {
        AdapterError::ExecutionFailed {
            tool_name: $tool.to_string(),
            reason: $e.to_string(),
        }
    };
}

/// Send a text message to a follower (wechat_oa_send_text).
pub async fn tool_send_text(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let openid = required_str!(params, "openid", "wechat_oa_send_text");
    let content = required_str!(params, "content", "wechat_oa_send_text");

    let body = TextMessage {
        touser: openid,
        msgtype: "text",
        text: TextContent { content },
    };

    let url = format!("{api_base}/message/custom/send?access_token={token}");
    let resp: Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_text", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_text", e))?;

    check_wechat_error(&resp, "wechat_oa_send_text")?;
    info!(openid, "WeChat OA text message sent");
    Ok(json!({ "success": true, "openid": openid }))
}

/// Send an image message (wechat_oa_send_image).
pub async fn tool_send_image(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let openid = required_str!(params, "openid", "wechat_oa_send_image");
    let media_id = required_str!(params, "media_id", "wechat_oa_send_image");

    let body = ImageMessage {
        touser: openid,
        msgtype: "image",
        image: ImageContent { media_id },
    };

    let url = format!("{api_base}/message/custom/send?access_token={token}");
    let resp: Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_image", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_image", e))?;

    check_wechat_error(&resp, "wechat_oa_send_image")?;
    Ok(json!({ "success": true, "openid": openid, "media_id": media_id }))
}

/// Send a template message (wechat_oa_send_template).
pub async fn tool_send_template(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let openid = required_str!(params, "openid", "wechat_oa_send_template");
    let template_id = required_str!(params, "template_id", "wechat_oa_send_template");
    let data = params.get("data").cloned().unwrap_or(json!({}));
    let url_param = params.get("url").and_then(|v| v.as_str()).unwrap_or("");

    let mut body = json!({
        "touser": openid,
        "template_id": template_id,
        "data": data
    });
    if !url_param.is_empty() {
        body["url"] = json!(url_param);
    }

    let api_url = format!("{api_base}/message/template/send?access_token={token}");
    let resp: Value = client
        .post(&api_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_template", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_send_template", e))?;

    check_wechat_error(&resp, "wechat_oa_send_template")?;
    let msgid = resp.get("msgid").cloned().unwrap_or(json!(null));
    Ok(json!({ "success": true, "msgid": msgid }))
}

/// List follower OpenIDs (wechat_oa_get_followers).
pub async fn tool_get_followers(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let next = params
        .get("next_openid")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let url = format!("{api_base}/user/get?access_token={token}&next_openid={next}");
    let resp: Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_get_followers", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_get_followers", e))?;

    check_wechat_error(&resp, "wechat_oa_get_followers")?;

    let total = resp.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let count = resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    let next_openid = resp
        .get("next_openid")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let openids: Vec<String> = resp
        .get("data")
        .and_then(|d| d.get("openid"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    Ok(json!({
        "total": total,
        "count": count,
        "openids": openids,
        "next_openid": next_openid
    }))
}

/// Get follower user info (wechat_oa_get_user_info).
pub async fn tool_get_user_info(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let openid = required_str!(params, "openid", "wechat_oa_get_user_info");
    let lang = params
        .get("lang")
        .and_then(|v| v.as_str())
        .unwrap_or("zh_CN");

    let url = format!("{api_base}/user/info?access_token={token}&openid={openid}&lang={lang}");
    let resp: Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_get_user_info", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_get_user_info", e))?;

    check_wechat_error(&resp, "wechat_oa_get_user_info")?;
    Ok(resp)
}

/// Upload a temporary image (wechat_oa_upload_image).
pub async fn tool_upload_image(
    client: &reqwest::Client,
    _api_base: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let file_path = required_str!(params, "file_path", "wechat_oa_upload_image");

    let bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| exec_err!("wechat_oa_upload_image", e))?;

    let file_name = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image.jpg")
        .to_string();

    let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
    let form = reqwest::multipart::Form::new().part("media", part);

    let url = format!(
        "https://api.weixin.qq.com/cgi-bin/media/upload?access_token={token}&type=image"
    );
    let resp: Value = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| exec_err!("wechat_oa_upload_image", e))?
        .json()
        .await
        .map_err(|e| exec_err!("wechat_oa_upload_image", e))?;

    check_wechat_error(&resp, "wechat_oa_upload_image")?;

    let media_id = resp
        .get("media_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(json!({ "success": true, "media_id": media_id, "expires_in_days": 3 }))
}
