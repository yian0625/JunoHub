//! TokenHub's image task API exposed as a synchronous image generation response.
//! Submit exactly once: ambiguous submission/poll failures must never retry on another key.
use crate::error::{AppError, AppResult};
use axum::{body::Body, http::{HeaderMap, StatusCode}, response::Response};
use serde_json::{json, Value};
use std::time::Duration;

pub fn is_endpoint(base: &str, path: &str) -> bool {
    reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_owned)).as_deref() == Some("tokenhub.piegateway.me")
        && path.split('?').next() == Some("/v1/images/generations")
}
fn output(status: StatusCode, body: Value) -> Response {
    Response::builder().status(status).header("content-type","application/json")
        .body(Body::from(body.to_string())).expect("static response headers")
}
fn error(status: StatusCode, message: &str, task: Option<&str>) -> Response {
    output(status,json!({"error":{"message":message,"type":"image_generation_error","task_id":task}}))
}
fn body(payload: &Value) -> AppResult<Value> {
    let model = payload["model"].as_str().unwrap_or("");
    if !matches!(model,"gpt-image-2"|"gpt-image-2.5-sunburst"|"gpt-image-2.5-flare") {
        return Err(AppError::BadRequest("此图片接口目前支持 TokenHub GPT Image 2 / 2.5，请配置对应的上游模型。".into()));
    }
    if payload["prompt"].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Err(AppError::BadRequest("图片生成需要非空 prompt".into()));
    }
    if payload.get("response_format").is_some_and(|v| v != "url") {
        return Err(AppError::BadRequest("TokenHub 图片适配当前返回 URL，请将 response_format 设为 url 或省略。".into()));
    }
    if payload.get("stream").is_some_and(|v| v != false) {
        return Err(AppError::BadRequest("图片生成当前为非流式接口，请省略 stream 或设为 false。".into()));
    }
    let mut body = payload.clone();
    let object = body.as_object_mut().ok_or_else(||AppError::BadRequest("请求体必须为 JSON 对象".into()))?;
    for key in ["response_format","stream","user"] {object.remove(key);}
    Ok(body)
}

pub async fn forward(client: &reqwest::Client, base: &str, key: &str, headers: &HeaderMap, payload: &Value) -> AppResult<Response> {
    let payload = body(payload)?;
    let mut url = reqwest::Url::parse(base).map_err(|_|AppError::BadRequest("图片服务地址无效".into()))?;
    url.set_path("/v2/extend/image/async/gpt_image/generations");url.set_query(None);url.set_fragment(None);
    let mut request = client.post(url.clone()).header("X-API-Key",key).timeout(Duration::from_secs(20)).json(&payload);
    // TokenHub documents this optional metadata header; never forward the caller's credentials.
    if let Some(value) = headers.get("payload") {request=request.header("payload",value);}
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => return Ok(error(StatusCode::BAD_GATEWAY,"图片提交结果未知；请先确认上游任务，避免重复计费。",None)),
    };
    let status = response.status();
    if !status.is_success() {return Ok(error(status,"TokenHub 拒绝图片生成请求，请检查模型权限、参数和余额。",None));}
    let submitted: Value = response.json().await?;
    let Some(task) = submitted["task_id"].as_str().filter(|s|!s.is_empty() && s.len()<256) else {
        return Ok(error(StatusCode::BAD_GATEWAY,"TokenHub 未返回图片任务 ID，无法确认提交结果。",None));
    };
    url.set_path("/v2/extend/image/gpt_image/tasks/");
    url.path_segments_mut().map_err(|_|AppError::BadRequest("图片服务地址无效".into()))?.pop_if_empty().push(task);
    let deadline = tokio::time::Instant::now()+Duration::from_secs(90);
    loop {
        let response = match tokio::time::timeout_at(deadline,client.get(url.clone()).header("X-API-Key",key).timeout(Duration::from_secs(15)).send()).await {
            Ok(Ok(response))=>response,_=>break,
        };
        if !response.status().is_success() {return Ok(error(StatusCode::BAD_GATEWAY,"图片任务已提交，但查询失败；请勿直接重复生成。",Some(task)));}
        let result: Value = match tokio::time::timeout_at(deadline,response.json()).await {Ok(Ok(v))=>v,_=>break};
        match result["status"].as_str() {
            Some("succeeded") => {
                let Some(images) = result.pointer("/result/images").and_then(Value::as_array).filter(|a|!a.is_empty()) else {return Ok(error(StatusCode::BAD_GATEWAY,"图片任务完成但没有图片。",Some(task)));};
                if images.iter().any(|i|i["url"].as_str().is_none_or(str::is_empty)) {return Ok(error(StatusCode::BAD_GATEWAY,"图片结果缺少 URL。",Some(task)));}
                let mut response = json!({"created":crate::store::now(),"data":images,"task_id":task});
                if let Some(usage) = result.pointer("/result/usage") {response["usage"]=usage.clone();}
                return Ok(output(StatusCode::OK,response));
            }
            Some("failed")=>return Ok(error(StatusCode::BAD_GATEWAY,"TokenHub 图片生成失败，请检查上游任务。",Some(task))),
            Some("queued"|"processing")=>{},
            _=>return Ok(error(StatusCode::BAD_GATEWAY,"图片任务返回未知状态。",Some(task))),
        }
        if tokio::time::timeout_at(deadline,tokio::time::sleep(Duration::from_secs(2))).await.is_err() {break;}
    }
    Ok(error(StatusCode::GATEWAY_TIMEOUT,"图片任务已提交但未在等待时间内完成；请先查询上游任务，避免重复计费。",Some(task)))
}
