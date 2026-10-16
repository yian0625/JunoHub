//! Explicit, bounded connection diagnostics. No writes or scheduler/cooldown mutations.
use crate::{
    console::{validate_base_url, ProviderSecret},
    error::{AppError, AppResult},
    state::AppState,
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub ok: bool,
    pub outcome: String,
    pub status_code: u16,
    pub message: String,
    pub model_count: usize,
    pub models: Vec<String>,
    pub latency_ms: u128,
    pub suggested_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_path: Option<String>,
}
fn result(outcome: &str, status: u16, message: &str, started: Instant) -> Probe {
    Probe {
        ok: outcome == "success",
        outcome: outcome.into(),
        status_code: status,
        message: message.into(),
        model_count: 0,
        models: vec![],
        latency_ms: started.elapsed().as_millis(),
        suggested_base_url: None,
        request_path: None,
    }
}

pub fn route_test_payload(kind: &crate::model_api::ApiType, model: &str) -> AppResult<Value> {
    use crate::model_api::ApiType;
    Ok(match kind {
        ApiType::ChatCompletions | ApiType::Auto => json!({"model":model,"messages":[{"role":"user","content":"Reply only OK."}],"stream":false}),
        ApiType::Responses => json!({"model":model,"input":"Reply only OK.","max_output_tokens":16,"stream":false,"store":false}),
        ApiType::Messages => json!({"model":model,"messages":[{"role":"user","content":"Reply only OK."}],"max_tokens":16,"stream":false}),
        _ => return Err(AppError::BadRequest("当前测试仅支持 Chat Completions、Responses 和 Messages 格式".into())),
    })
}

pub fn route_test_failure(started: Instant, message: &str) -> Probe {
    result("error", 0, message, started)
}

pub fn route_test_result(status: u16, bytes: &[u8], kind: &crate::model_api::ApiType, started: Instant) -> Probe {
    use crate::model_api::ApiType;
    if !(200..300).contains(&status) {
        let message = match status {
            401 | 403 => "上游拒绝认证，请检查供应商凭证、权限和鉴权方式。",
            404 | 405 => "上游路径或模型不可用，请核对请求路径与模型名称。",
            429 => "上游拒绝了本次请求（HTTP 429）。请稍后重试，或在供应商控制台查看原因。",
            400 | 422 => "上游未接受测试参数，请核对请求格式与模型支持范围。",
            _ => "上游请求失败，请检查服务状态后重试。",
        };
        return result(if status == 429 { "warning" } else { "error" }, status, message, started);
    }
    let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
        return result("error", status, "响应不是有效 JSON，请检查是否填写了网站页面地址。", started);
    };
    if body.get("error").is_some_and(|value| !value.is_null()) {
        return result("error", status, "上游返回错误结果，请检查模型权限及请求格式。", started);
    }
    let valid = match kind {
        ApiType::ChatCompletions | ApiType::Auto => body.pointer("/choices/0/message/content")
            .is_some_and(|value| value.as_str().is_some_and(|text| !text.is_empty()) || value.as_array().is_some_and(|items| !items.is_empty())),
        ApiType::Responses => body["object"] == "response" && body["output"].is_array(),
        ApiType::Messages => body["type"] == "message" && body["content"].as_array().is_some_and(|items| !items.is_empty()),
        _ => false,
    };
    result(if valid { "success" } else { "error" }, status,
        if valid { "上游模型调用通过，当前路径和请求格式已收到有效响应。" }
        else { "服务已响应，但内容不符合所选请求格式，不能确认模型调用成功。" }, started)
}

pub async fn probe_route(client: &reqwest::Client, base: &str, key: &str, config: &crate::model_api::ApiConfig, model: &str, path: &str) -> AppResult<Probe> {
    use crate::model_api::ApiType;
    let started = Instant::now();
    let kind = if *config.upstream_type() == ApiType::Auto { ApiType::ChatCompletions } else { config.upstream_type().clone() };
    let body = route_test_payload(&kind, model)?;
    let url = config.upstream_target(base, kind.endpoint().unwrap(), path)?;
    let mut request = crate::model_api::authenticate(client.post(url.clone()), &config.auth_type, key)
        .timeout(Duration::from_secs(15)).json(&body);
    if kind == ApiType::Messages { request = request.header("anthropic-version", "2023-06-01"); }
    request = crate::model_api::apply_context_1m(request, config, &axum::http::HeaderMap::new());
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => return Ok(route_test_failure(started, if error.is_timeout() { "上游测试超时（15 秒），请核对路径和服务器网络。" } else { "无法连接上游，请检查地址、证书和服务器网络。" })),
    };
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk { Ok(chunk) => chunk, Err(_) => return Ok(route_test_failure(started, "上游响应未完整读取，请稍后重试。")) };
        if bytes.len() + chunk.len() > 1024 * 1024 { return Ok(route_test_failure(started, "上游测试响应超过 1MB，已停止读取。")); }
        bytes.extend_from_slice(&chunk);
    }
    let mut report = with_error_detail(route_test_result(status, &bytes, &kind, started), &bytes, key);
    report.request_path = Some(url.path().to_owned());
    Ok(report)
}

pub fn test_config(payload: &Value) -> AppResult<Option<crate::model_api::ApiConfig>> {
    use crate::model_api::{ApiConfig, ApiType, AuthType};
    let Some(kind) = payload.get("api_type") else { return Ok(None) };
    let kind = match kind.as_str() {
        Some("chat_completions") => ApiType::ChatCompletions,
        Some("responses") => ApiType::Responses,
        Some("messages") => ApiType::Messages,
        _ => return Err(AppError::BadRequest("测试格式仅支持 Chat Completions、Responses、Messages".into())),
    };
    let auth = match payload.get("auth_type") {
        None => if kind == ApiType::Messages { AuthType::XApiKey } else { AuthType::Bearer },
        Some(value) if value == "bearer" => AuthType::Bearer,
        Some(value) if value == "x_api_key" => AuthType::XApiKey,
        _ => return Err(AppError::BadRequest("请选择 Bearer 或 X-API-Key 鉴权".into())),
    };
    let context_1m = match payload.get("context_1m") {
        None => false,
        Some(Value::Bool(enabled)) => *enabled,
        _ => return Err(AppError::BadRequest("1M 上下文设置须为开启或关闭".into())),
    };
    let config = ApiConfig { api_type: kind, auth_type: auth, context_1m, ..Default::default() };
    config.validate_protocols()?;
    Ok(Some(config))
}

pub async fn probe_configured(state: &AppState, base: &str, key: &str, platform: &str, model: Option<&str>, config: Option<&crate::model_api::ApiConfig>) -> AppResult<Probe> {
    if tokenhub_image_model(base, model) { return probe(state, base, key, platform, model).await; }
    if let (Some(model), Some(config)) = (model.map(str::trim).filter(|model| !model.is_empty()), config) {
        if model.len() > 200 { return Err(AppError::BadRequest("模型名称不能超过 200 字符".into())); }
        let url = config.upstream_target(base, config.api_type.endpoint().unwrap(), "")?;
        let mut report = probe_route(&state.client, base, key, config, model, "").await?;
        report.request_path = Some(url.path().to_owned());
        return Ok(report);
    }
    probe(state, base, key, platform, model).await
}

// Only expose a bounded JSON error message; never forward raw HTML or credentials.
fn with_error_detail(mut report: Probe, bytes: &[u8], key: &str) -> Probe {
    if report.ok { return report; }
    if let Some(detail) = public_error_detail(bytes, key) {
        if detail.eq_ignore_ascii_case("Service Unavailable") {
            report.message = "上游报告服务不可用，未说明具体原因。请稍后重试，或在供应商控制台检查服务状态。".into();
        }
        if !detail.is_empty() { report.message.push_str(&format!(" 上游提示：{detail}")); }
    }
    report
}
pub(crate) fn public_error_detail(bytes: &[u8], key: &str) -> Option<String> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let detail = body.pointer("/error/message").and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .or_else(|| body.get("message").and_then(Value::as_str))?;
    let detail = if key.trim().is_empty() { detail.to_owned() } else { detail.replace(key.trim(), "[已隐藏密钥]") };
    let detail = detail.split_whitespace().map(|word| if word.contains("sk-") { "[已隐藏密钥]" } else { word }).collect::<Vec<_>>().join(" ");
    let detail: String = detail.chars().filter(|c| !c.is_control()).take(400).collect();
    (!detail.is_empty()).then_some(detail)
}
/// Accept a pasted OpenAI endpoint as well as an API base, preserving custom prefixes.
pub fn compatible_base(base: &str) -> AppResult<String> {
    let validated = validate_base_url(base.trim())?;
    let mut url =
        reqwest::Url::parse(&validated).map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
    let path = url.path().trim_end_matches('/').to_owned();
    let prefix = ["/chat/completions", "/responses", "/messages", "/models"]
        .iter()
        .find_map(|suffix| path.strip_suffix(suffix));
    if let Some(prefix) = prefix {
        url.set_path(prefix);
    }
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}
pub fn endpoint(base: &str, path: &str) -> AppResult<reqwest::Url> {
    let base = compatible_base(base)?;
    let mut url =
        reqwest::Url::parse(&base).map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
    url.set_path(&format!("{}{}", url.path().trim_end_matches('/'), path));
    Ok(url)
}
pub async fn models(state: &AppState, provider: &ProviderSecret) -> AppResult<Vec<String>> {
    let report = probe(state, &provider.base_url, &provider.api_key, &provider.provider_type, None).await?;
    if report.ok {
        Ok(report.models)
    } else {
        Err(AppError::BadRequest(report.message))
    }
}
pub fn tokenhub_image_model(base: &str, model: Option<&str>) -> bool {
    reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_owned)).as_deref() == Some("tokenhub.piegateway.me")
        && model.is_some_and(|m| matches!(m.trim(), "gpt-image-2" | "gpt-image-2.5-sunburst" | "gpt-image-2.5-flare"))
}

async fn tokenhub_image_probe(client: &reqwest::Client, base: &str, key: &str, model: &str) -> AppResult<Probe> {
    let started = Instant::now();
    let mut url = reqwest::Url::parse(&validate_base_url(base)?).map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
    url.set_query(None); url.set_fragment(None);
    url.set_path("/v2/extend/image/async/gpt_image/generations");
    let sent = client.post(url.clone()).header("X-API-Key",key.trim())
        .timeout(Duration::from_secs(15))
        .json(&json!({"model":model,"prompt":"A small blue circle on a white background.","size":"1024x1024","quality":"low","n":1,"output_format":"png"})).send().await;
    let response = match sent {
        Ok(response) => response,
        Err(_) => return Ok(result("warning",0,"图片任务提交结果未知，请先在 TokenHub 确认，避免重复生成。",started)),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Ok(result("error",status,if matches!(status,401|403) {"图片接口拒绝认证，请检查 Key 的图片服务权限。"} else {"TokenHub 图片接口未接受请求，请检查模型权限和图片服务配置。"},started));
    }
    let body: Value = response.json().await?;
    let Some(task) = body.get("task_id").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len()<256) else {
        return Ok(result("warning",status,"图片接口未返回任务 ID，无法确认生成结果，请先检查 TokenHub 任务记录。",started));
    };
    url.set_path("/v2/extend/image/gpt_image/tasks/");
    url.path_segments_mut().map_err(|_| AppError::BadRequest("连接地址无效".into()))?.pop_if_empty().push(task);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(105);
    loop {
        let response = match tokio::time::timeout_at(deadline,client.get(url.clone()).header("X-API-Key",key.trim()).timeout(Duration::from_secs(10)).send()).await {
            Ok(Ok(response)) => response,
            _ => break,
        };
        let status = response.status().as_u16();
        if !response.status().is_success() { break; }
        let body: Value = match tokio::time::timeout_at(deadline,response.json()).await {Ok(Ok(body)) => body,_ => break};
        match body.get("status").and_then(Value::as_str) {
            Some("succeeded") => {
                let valid = body.pointer("/result/images").and_then(Value::as_array).is_some_and(|images| images.iter().any(|image| image.get("url").and_then(Value::as_str).is_some_and(|url| !url.is_empty())));
                return Ok(result(if valid {"success"} else {"warning"},status,if valid {"图片生成成功，当前 Key 可调用此模型。"} else {"图片任务已完成，但未返回图片地址。"},started));
            }
            Some("failed") => return Ok(result("error",status,&format!("图片生成任务失败，请在 TokenHub 查看任务 {task}。"),started)),
            Some("queued" | "processing") => {},
            _ => break,
        }
        if tokio::time::timeout_at(deadline,tokio::time::sleep(Duration::from_secs(2))).await.is_err() {break;}
    }
    Ok(result("warning",status,&format!("图片任务已提交，尚未确认完成。任务 ID：{task}。请在 TokenHub 查询，避免重复生成。"),started))
}

pub async fn probe(
    state: &AppState,
    base: &str,
    key: &str,
    platform: &str,
    model: Option<&str>,
) -> AppResult<Probe> {
    if tokenhub_image_model(base, model) {
        return tokenhub_image_probe(&state.client,base,key,model.unwrap().trim()).await;
    }
    let started = Instant::now();
    let base = if matches!(platform, "anthropic" | "gemini") {
        validate_base_url(base.trim())?
    } else {
        compatible_base(base)?
    };
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    if model.is_some_and(|m| m.len() > 200) {
        return Err(AppError::BadRequest("模型名称不能超过 200 字符".into()));
    }
    let mut url =
        reqwest::Url::parse(&base).map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
    let prefix = url.path().trim_end_matches('/').to_owned();
    let suffix = match (platform, model) {
        ("anthropic", Some(_)) => "/messages",
        ("gemini", _) => "/models",
        (_, Some(_)) => "/chat/completions",
        _ => "/models",
    };
    let version = if platform == "anthropic" && !prefix.ends_with("/v1") {
        "/v1"
    } else if platform == "gemini" && !prefix.ends_with("/v1") && !prefix.ends_with("/v1beta") {
        "/v1beta"
    } else {
        ""
    };
    url.set_path(&format!("{prefix}{version}{suffix}"));
    if platform == "gemini" {
        if let Some(model) = model {
            url.path_segments_mut()
                .map_err(|_| AppError::BadRequest("连接地址无效".into()))?
                .push(&format!(
                    "{}:generateContent",
                    model.trim_start_matches("models/")
                ));
        }
    }
    let mut request = if model.is_some() {
        state.client.post(url.clone())
    } else {
        state.client.get(url.clone())
    };
    request = request.timeout(Duration::from_secs(15));
    if !key.trim().is_empty() {
        request = match platform {
            "anthropic" => request.header("x-api-key", key.trim()),
            "gemini" => request.header("x-goog-api-key", key.trim()),
            _ => request.bearer_auth(key.trim()),
        };
    }
    if platform == "anthropic" {
        request = request.header("anthropic-version", "2023-06-01");
    }
    if let Some(model) = model {
        request=request.json(&if platform=="gemini"{json!({"contents":[{"parts":[{"text":"Hi"}]}],"generationConfig":{"maxOutputTokens":1}})}else{json!({"model":model,"messages":[{"role":"user","content":"Hi"}],"max_tokens":1,"stream":false})});
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let message = if error.is_timeout() {
                "连接测试超时，请检查地址及服务器网络（最长等待 15 秒）。"
            } else if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
                "无法连接该地址。测试由容器发起；访问宿主机服务请使用 host.docker.internal，而不是 localhost。"
            } else {
                "无法连接上游，请检查地址、端口、证书和容器的网络连接。"
            };
            return Ok(result("error", 0, message, started));
        }
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let (outcome, message) = match status {
            401 | 403 => ("error", "上游拒绝了认证，请检查 Key 及其访问权限。"),
            404 | 405 if model.is_none() => (
                "warning",
                "服务已响应，但没有提供模型列表接口。请核对 API 地址，或填写模型名称验证实际调用。",
            ),
            404 => ("error", "调用接口或模型不存在，请核对 API 地址和模型名称。"),
            429 => ("warning", "上游拒绝了本次请求（HTTP 429）。请稍后重试，或在供应商控制台查看原因。"),
            500..=599 => ("error", "上游服务暂时异常，请稍后重试。"),
            400 | 422 => ("error", "上游不接受测试请求，请检查模型名称及接口兼容性。"),
            _ => ("error", "上游未接受测试请求，请检查服务配置。"),
        };
        let mut report = result(outcome, status, message, started);
        if model.is_none()
            && matches!(status, 404 | 405)
            && prefix.is_empty()
            && !matches!(platform, "anthropic" | "gemini")
        {
            let mut suggested = url.clone();
            suggested.set_path("/v1");
            suggested.set_query(None);
            report.suggested_base_url = Some(suggested.to_string());
            report
                .message
                .push_str(" 当前地址没有 API 路径，常见兼容服务使用 /v1。");
        }
        return Ok(report);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                return Ok(result(
                    "error",
                    status,
                    "上游响应未完整读取，请重试。",
                    started,
                ))
            }
        };
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Ok(result(
                "warning",
                status,
                "服务已响应，但测试结果过大，未继续读取。",
                started,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return Ok(result(
                "warning",
                status,
                "服务返回了非 JSON 内容，可能填写了网站地址而不是 API 地址。",
                started,
            ))
        }
    };
    if body.get("error").is_some_and(|e| !e.is_null()) {
        return Ok(result(
            "error",
            status,
            "上游返回了错误结果，请检查 Key 权限、模型名称及服务配置。",
            started,
        ));
    }
    if model.is_some() {
        let valid = if platform == "anthropic" {
            body.get("content").is_some_and(Value::is_array)
        } else if platform == "gemini" {
            body.get("candidates")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        } else {
            body.get("choices")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        };
        return Ok(result(
            if valid { "success" } else { "warning" },
            status,
            if valid {
                "模型调用成功，当前地址和凭证可用于这次测试。"
            } else {
                "服务已响应，但未返回预期的模型调用结果，请核对接口兼容性。"
            },
            started,
        ));
    }
    let items = body
        .get(if platform == "gemini" {
            "models"
        } else {
            "data"
        })
        .and_then(Value::as_array);
    let Some(items) = items else {
        return Ok(result(
            "warning",
            status,
            "服务已响应，但模型列表格式不兼容。可填写模型名称验证实际调用。",
            started,
        ));
    };
    let mut report = result(
        "success",
        status,
        if items.is_empty() {
            "模型列表接口可访问，但列表为空。可填写模型名称进一步验证调用。"
        } else {
            "连接成功，已读取模型列表。模型调用权限可另行验证。"
        },
        started,
    );
    report.models = items
        .iter()
        .filter_map(|item| {
            item.get(if platform == "gemini" { "name" } else { "id" })
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|m| !m.is_empty() && m.len() <= 200)
        .map(str::to_owned)
        .collect();
    report.models.sort();
    report.models.dedup();
    report.model_count = report.models.len();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_429_does_not_infer_balance_or_invalid_credentials() {
        use crate::model_api::ApiType;
        let report = with_error_detail(route_test_result(429, b"", &ApiType::Messages, Instant::now()), br#"{"error":"Service Unavailable"}"#, "fixture-secret");
        assert!(!report.ok);
        assert_eq!(report.status_code, 429);
        assert!(report.message.contains("上游报告服务不可用"));
        assert!(report.message.contains("Service Unavailable"));
        assert!(!report.message.contains("限流"));
        assert!(!report.message.contains("额度不足"));
        for bytes in [br#"{"error":{"message":"rate limit exceeded"}}"#.as_slice(), br#"{"error":{"message":"quota exhausted"}}"#.as_slice(), b"<html>private error</html>"] {
            let report = with_error_detail(route_test_result(429, b"", &ApiType::Messages, Instant::now()), bytes, "fixture-secret");
            assert!(!report.ok);
            assert!(!report.message.contains("额度不足"));
            assert!(!report.message.contains("private error"));
            assert!(report.message.contains("HTTP 429"));
        }
    }
    #[tokio::test]
    async fn context_marker_changes_probe_and_saved_route_requests() {
        use axum::{http::{HeaderMap, StatusCode}, routing::post, Json, Router};
        use crate::model_api::{ApiConfig, CONTEXT_1M_BETA};
        let app = Router::new().route("/v1/messages", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["x-api-key"], "fixture-test-key");
            assert_eq!(body["max_tokens"], 16);
            if headers.get("anthropic-beta").is_some_and(|value| value == CONTEXT_1M_BETA) {
                (StatusCode::OK, Json(json!({"type":"message","content":[{"type":"text","text":"OK"}]})))
            } else {
                (StatusCode::BAD_REQUEST, Json(json!({"error":"请启用 1m 上下文后重试"})))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        for enabled in [false, true] {
            let config = test_config(&json!({"api_type":"messages", "context_1m":enabled})).unwrap().unwrap();
            // Both supplier diagnostics and model route tests use this same request path.
            let stored = ApiConfig::from_stored(&serde_json::to_string(&config).unwrap()).unwrap();
            let report = probe_route(&client, &base, "fixture-test-key", &stored, "test-model", "").await.unwrap();
            assert_eq!(report.ok, enabled);
            assert_eq!(report.status_code, if enabled { 200 } else { 400 });
            if !enabled { assert!(report.message.contains("请启用 1m 上下文")); }
        }
        assert!(test_config(&json!({"api_type":"messages","context_1m":"true"})).is_err());
        assert!(test_config(&json!({"api_type":"responses","context_1m":true})).is_err());
        server.abort();
    }
    #[tokio::test]
    async fn supplier_model_test_uses_explicit_protocol_and_redacts_upstream_errors() {
        use axum::{http::{HeaderMap, StatusCode}, routing::post, Json, Router};
        let app = Router::new()
            .route("/v1/chat/completions", post(|| async {
                (StatusCode::NOT_FOUND, Json(json!({"error":{"message":"Unsupported chat route fixture-sensitive-key sk-other-secret"}})))
            }))
            .route("/v1/responses", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture-sensitive-key");
                assert_eq!(body["input"], "Reply only OK.");
                assert!(body.get("messages").is_none());
                Json(json!({"object":"response","output":[]}))
            }))
            .route("/v1/messages", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixture-sensitive-key");
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                assert_eq!(body["model"], "test-model");
                assert_eq!(body["max_tokens"], 16);
                Json(json!({"type":"message","content":[{"type":"text","text":"OK"}]}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = crate::console::tests::state().await;
        for kind in ["chat_completions", "responses", "messages"] {
            let config = test_config(&json!({"api_type":kind,"auth_type":"bearer"})).unwrap();
            let report = probe_configured(&state, &base, "fixture-sensitive-key", "custom", Some("test-model"), config.as_ref()).await.unwrap();
            if kind == "chat_completions" {
                assert_eq!(report.status_code, 404);
                assert!(!report.ok);
                assert_eq!(report.request_path.as_deref(), Some("/v1/chat/completions"));
                assert!(report.message.contains("Unsupported chat route"));
                assert!(!report.message.contains("fixture-sensitive-key"));
                assert!(!report.message.contains("sk-other-secret"));
            } else { assert!(report.ok, "{}", report.message); }
        }
        assert!(test_config(&json!({"api_type":"images_generations"})).is_err());
        assert!(test_config(&json!({"api_type":"responses","auth_type":false})).is_err());
        assert!(test_config(&json!({})).unwrap().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn route_test_uses_draft_path_protocol_and_auth() {
        use axum::{extract::Path, http::HeaderMap, routing::post, Json, Router};
        use crate::model_api::{ApiConfig, ApiType, AuthType};
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let app = Router::new().route("/draft/{kind}", post(move |Path(kind): Path<String>, headers: HeaderMap, Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["model"], "draft-model");
                assert_eq!(body["stream"], false);
                if kind == "messages" {
                    assert_eq!(headers["x-api-key"], "private-fixture-key");
                    assert_eq!(headers["anthropic-version"], "2023-06-01");
                    assert!(headers.get("authorization").is_none());
                } else { assert_eq!(headers["authorization"], "Bearer private-fixture-key"); }
                Json(match kind.as_str() {
                    "responses" => {
                        assert_eq!(body["input"], "Reply only OK.");
                        assert_eq!(body["store"], false);
                        assert!(body.get("messages").is_none());
                        json!({"object":"response","output":[]})
                    },
                    "messages" => json!({"type":"message","content":[{"type":"text","text":"OK"}]}),
                    _ => json!({"choices":[{"message":{"content":"OK"}}]}),
                })
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/prefix/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // The probe sends the actual draft request without a database or scheduler.
        for (kind, path) in [(ApiType::ChatCompletions, "chat"), (ApiType::Responses, "responses"), (ApiType::Messages, "messages")] {
            let config = ApiConfig { api_type: kind.clone(), upstream_api_type: Some(kind.clone()), auth_type: if kind == ApiType::Messages { AuthType::XApiKey } else { AuthType::Bearer }, ..Default::default() };
            let report = probe_route(&reqwest::Client::new(), &base, "private-fixture-key", &config, "draft-model", &format!("/draft/{path}")).await.unwrap();
            assert!(report.ok, "{}", report.message);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        server.abort();
    }
    #[test]
    fn route_test_rejects_http_errors_and_malformed_success() {
        use crate::model_api::ApiType;
        for (status, body) in [(401, "private-fixture-key"), (404, "missing"), (200, "<html>website</html>"), (200, "{}"), (200, "{\"choices\":[{}]}"), (200, "{\"error\":\"private-fixture-key\"}")] {
            let report = route_test_result(status, body.as_bytes(), &ApiType::ChatCompletions, Instant::now());
            assert!(!report.ok); assert_eq!(report.status_code, status);
            assert!(!report.message.contains("private-fixture-key"));
        }
        assert!(route_test_payload(&ApiType::ImagesGenerations, "image").is_err());
    }
    #[tokio::test]
    async fn tokenhub_image_uses_service_auth_and_task_result() {
        use axum::{Router,routing::{get,post},Json,http::HeaderMap};
        let app = Router::new()
            .route("/v2/extend/image/async/gpt_image/generations",post(|headers:HeaderMap,Json(body):Json<Value>|async move {
                assert_eq!(headers["x-api-key"],"fixture-key");
                assert!(headers.get("authorization").is_none());
                assert_eq!(body["model"],"gpt-image-2");
                assert_eq!(body["n"],1);
                assert!(body.get("messages").is_none());
                assert!(body.get("max_tokens").is_none());
                Json(json!({"task_id":"fixture-task","status":"queued"}))
            }))
            .route("/v2/extend/image/gpt_image/tasks/fixture-task",get(|headers:HeaderMap|async move {
                assert_eq!(headers["x-api-key"],"fixture-key");
                Json(json!({"status":"succeeded","result":{"images":[{"url":"https://example.test/image.png"}]}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1",listener.local_addr().unwrap());
        let server = tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
        let report = tokenhub_image_probe(&reqwest::Client::new(),&base,"fixture-key","gpt-image-2").await.unwrap();
        server.abort();
        assert!(report.ok,"{}",report.message);
    }
    #[test]
    fn image_adapter_is_scoped_to_documented_tokenhub_models() {
        assert!(tokenhub_image_model("https://tokenhub.piegateway.me/v1",Some("gpt-image-2")));
        assert!(!tokenhub_image_model("https://example.com/v1",Some("gpt-image-2")));
        assert!(!tokenhub_image_model("https://tokenhub.piegateway.me/v1",Some("gpt-5.6-sol")));
        assert!(!tokenhub_image_model("https://tokenhub.piegateway.me/v1",None));
    }
    #[test]
    fn endpoint_preserves_prefix_and_query_without_duplicate_routes() {
        for base in [
            "https://example.com/custom/v1",
            "https://example.com/custom/v1/",
            "https://example.com/custom/v1/chat/completions",
            "https://example.com/custom/v1/responses",
            "https://example.com/custom/v1/models",
        ] {
            assert_eq!(
                endpoint(base, "/models").unwrap().as_str(),
                "https://example.com/custom/v1/models"
            );
        }
        assert_eq!(
            endpoint(
                "https://example.com/v1/chat/completions?region=us",
                "/responses"
            )
            .unwrap()
            .as_str(),
            "https://example.com/v1/responses?region=us"
        );
        assert_eq!(
            endpoint("https://example.com", "/models").unwrap().as_str(),
            "https://example.com/models"
        );
        assert!(endpoint("file:///etc/passwd", "/models").is_err());
    }
}
