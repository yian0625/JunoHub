use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::Response,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    proxy,
    state::AppState,
};

const BODY_LIMIT: usize = crate::gateway_upload::MAX_BODY;

fn body_read_error(error: axum::Error) -> AppError {
    if std::error::Error::source(&error)
        .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
    {
        AppError::PayloadTooLarge("请求体不能超过 32MiB".into())
    } else {
        AppError::BadRequest("无法读取请求体".into())
    }
}

pub async fn anthropic_messages(
    State(state): State<AppState>,
    request: Request,
) -> AppResult<Response> {
    let headers = gateway_headers(request.headers(), request.uri().query())?;
    // Reserve bounded ingress and user/key concurrency before buffering up to 32 MiB.
    let ingress = proxy::begin_ingress(&state, &headers).await?;
    let body = to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(body_read_error)?;
    let source: Value = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("Claude 请求必须是 JSON".into()))?;
    let key = &ingress.key;
    let alias = source
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let model = crate::console::resolve_model(&state, key.user_id, alias).await?;
    if model.api_config.api_type == crate::model_api::ApiType::Messages {
        let mut request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/v1/messages")
            .body(Body::from(source.to_string()))
            .map_err(|_| AppError::BadRequest("Claude 请求无效".into()))?;
        *request.headers_mut() = headers;
        return proxy::passthrough_admitted(State(state), request, ingress).await;
    }
    let wants_stream = source.get("stream").and_then(Value::as_bool) == Some(true);
    let chat = anthropic_request_to_chat(&source)?;
    let response =
        proxy::passthrough_admitted(State(state), chat_request(headers, chat)?, ingress).await?;
    adapt_anthropic_response(response, &source, wants_stream).await
}

pub async fn gemini_generate(
    State(state): State<AppState>,
    Path(path): Path<String>,
    request: Request,
) -> AppResult<Response> {
    // This compatibility endpoint implements generation only; countTokens and
    // arbitrary methods must not accidentally submit a billed generation.
    let (model, method) = path
        .rsplit_once(':')
        .ok_or_else(|| AppError::BadRequest("Gemini 请求路径无效".into()))?;
    if request.method() != Method::POST
        || !matches!(method, "generateContent" | "streamGenerateContent")
    {
        return Err(AppError::BadRequest(
            "当前 Gemini 接口仅支持 POST generateContent 或 streamGenerateContent".into(),
        ));
    }
    let query = request.uri().query().map(str::to_string);
    let headers = gateway_headers(request.headers(), query.as_deref())?;
    let ingress = proxy::begin_ingress(&state, &headers).await?;
    let body = to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(body_read_error)?;
    let source: Value = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("Gemini 请求必须是 JSON".into()))?;
    if model.trim().is_empty() {
        return Err(AppError::BadRequest("Gemini 模型不能为空".into()));
    }
    let wants_stream = method == "streamGenerateContent"
        || query
            .as_deref()
            .is_some_and(|query| query.contains("alt=sse"));
    let chat = gemini_request_to_chat(&source, model)?;
    let response =
        proxy::passthrough_admitted(State(state), chat_request(headers, chat)?, ingress).await?;
    adapt_gemini_response(response, model, wants_stream).await
}

fn gateway_headers(source: &HeaderMap, query: Option<&str>) -> AppResult<HeaderMap> {
    let mut headers = source.clone();
    headers.remove(header::CONTENT_LENGTH);
    if headers.get(header::AUTHORIZATION).is_none() {
        let key = source
            .get("x-api-key")
            .or_else(|| source.get("x-goog-api-key"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                query.and_then(|query| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .find(|(name, _)| name == "key")
                        .map(|(_, value)| value.into_owned())
                })
            })
            .ok_or(AppError::Unauthorized)?;
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| AppError::Unauthorized)?,
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(headers)
}

fn chat_request(headers: HeaderMap, payload: Value) -> AppResult<Request> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/v1/chat/completions");
    *builder
        .headers_mut()
        .ok_or_else(|| AppError::Internal("无法构建客户端请求".into()))? = headers;
    builder
        .body(Body::from(serde_json::to_vec(&payload)?))
        .map_err(|error| AppError::Internal(error.to_string()))
}

async fn response_json(response: Response) -> AppResult<(StatusCode, HeaderMap, Value)> {
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok((
                StatusCode::BAD_GATEWAY,
                parts.headers,
                json!({"error":{"message":"上游响应读取失败，无法转换结果"}}),
            ));
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok((parts.status, parts.headers, value)),
        Err(_) => Ok((
            if parts.status.is_success() {
                StatusCode::BAD_GATEWAY
            } else {
                parts.status
            },
            parts.headers,
            json!({"error":{"message":"上游返回非 JSON 响应，请检查服务状态和请求路径"}}),
        )),
    }
}

pub(crate) async fn adapt_anthropic_response(
    response: Response,
    request: &Value,
    stream: bool,
) -> AppResult<Response> {
    let (status, headers, chat) = response_json(response).await?;
    if !status.is_success() {
        return json_response(status, anthropic_error(&chat));
    }
    if let Err(message) = validate_chat_result(&chat) {
        return json_response(
            StatusCode::BAD_GATEWAY,
            anthropic_error(&json!({"error":{"message":message}})),
        );
    }
    if chat
        .pointer("/choices/0/message/reasoning_content")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
    {
        return json_response(
            StatusCode::BAD_GATEWAY,
            anthropic_error(
                &json!({"error":{"message":"上游返回 reasoning_content，当前 Anthropic 适配器无法保留该内容"}}),
            ),
        );
    }
    if chat
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| calls.iter().any(|call| call.get("extra_content").is_some()))
    {
        return json_response(
            StatusCode::BAD_GATEWAY,
            anthropic_error(
                &json!({"error":{"message":"上游工具调用包含 Gemini thought_signature，Anthropic 协议无法保留该字段"}}),
            ),
        );
    }
    let message = chat_response_to_anthropic(&chat, request)?;
    if !stream {
        return json_response(status, message);
    }
    let body = anthropic_sse(&message)?;
    stream_response(status, &headers, body)
}

async fn adapt_gemini_response(
    response: Response,
    model: &str,
    stream: bool,
) -> AppResult<Response> {
    let (status, headers, chat) = response_json(response).await?;
    if !status.is_success() {
        return json_response(status, gemini_error(&chat, status));
    }
    if let Err(message) = validate_chat_result(&chat) {
        return json_response(
            StatusCode::BAD_GATEWAY,
            gemini_error(
                &json!({"error":{"message":message}}),
                StatusCode::BAD_GATEWAY,
            ),
        );
    }
    if chat
        .pointer("/usage/prompt_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 0)
    {
        return json_response(
            StatusCode::BAD_GATEWAY,
            gemini_error(
                &json!({"error":{"message":"上游报告了 Gemini 协议无法表达的 cache_write_tokens 计费，已拒绝返回不完整 usage"}}),
                StatusCode::BAD_GATEWAY,
            ),
        );
    }
    let body = chat_response_to_gemini(&chat, model);
    if !stream {
        return json_response(status, body);
    }
    stream_response(
        status,
        &headers,
        format!("data: {}\n\n", serde_json::to_string(&body)?),
    )
}

fn validate_chat_result(chat: &Value) -> Result<(), &'static str> {
    if chat.get("error").is_some_and(|error| !error.is_null()) {
        return Err("上游返回错误结果，无法转换为成功响应");
    }
    if chat
        .get("id")
        .is_some_and(|id| !id.as_str().is_some_and(|id| !id.trim().is_empty()))
    {
        return Err("上游响应 id 格式无效");
    }
    let choices = chat
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| choices.len() == 1)
        .ok_or("上游响应必须且只能包含一个 choices 项")?;
    let choice = &choices[0];
    if choice.as_object().is_none_or(|object| {
        object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "index" | "message" | "finish_reason" | "logprobs"
            )
        })
    }) || choice
        .get("index")
        .is_some_and(|index| index.as_u64() != Some(0))
        || choice
            .get("logprobs")
            .is_some_and(|logprobs| !logprobs.is_null())
    {
        return Err("上游 choice 含有当前转换不支持的字段或索引");
    }
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .ok_or("上游响应缺少有效 finish_reason")?;
    if !matches!(
        finish_reason,
        "stop" | "length" | "tool_calls" | "content_filter"
    ) {
        return Err("上游返回了当前转换不支持的 finish_reason");
    }
    let message = choice
        .get("message")
        .filter(|message| message.is_object())
        .ok_or("上游响应缺少有效的 choices.message，无法转换结果")?;
    if message.as_object().is_some_and(|object| {
        object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "role" | "content" | "refusal" | "reasoning_content" | "tool_calls"
            )
        })
    }) || message
        .get("role")
        .is_some_and(|role| role.as_str() != Some("assistant"))
    {
        return Err("上游 assistant message 含有当前转换不支持的字段或角色");
    }
    let content = message.get("content");
    let text = match content {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => {
            for item in items {
                if !item.is_object()
                    || item["type"] != "text"
                    || !item["text"].is_string()
                    || item.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "text"))
                    })
                {
                    return Err("上游返回了当前转换不支持的内容块或扩展字段");
                }
            }
            items
                .iter()
                .any(|item| item["text"].as_str().is_some_and(|text| !text.is_empty()))
        }
        Some(Value::Null) | None => false,
        _ => return Err("上游返回了当前转换不支持的内容格式"),
    };
    if message
        .get("reasoning_content")
        .is_some_and(|reasoning| !reasoning.is_string() && !reasoning.is_null())
    {
        return Err("上游 reasoning_content 格式无效");
    }
    if let Some(refusal) = message.get("refusal") {
        if !refusal.is_string() && !refusal.is_null() {
            return Err("上游 refusal 格式无效");
        }
    }
    let mut has_tools = false;
    if let Some(calls) = message.get("tool_calls").filter(|calls| !calls.is_null()) {
        let calls = calls.as_array().ok_or("上游工具调用格式无效")?;
        has_tools = !calls.is_empty();
        for call in calls {
            let valid_string =
                |value: &Value| value.as_str().is_some_and(|value| !value.trim().is_empty());
            if !call.is_object()
                || call["type"] != "function"
                || !valid_string(&call["id"])
                || !valid_string(&call["function"]["name"])
                || !call["function"]["arguments"]
                    .as_str()
                    .and_then(|args| serde_json::from_str::<Value>(args).ok())
                    .is_some_and(|args| args.is_object())
                || call.as_object().is_some_and(|object| {
                    object.keys().any(|key| {
                        !matches!(key.as_str(), "id" | "type" | "function" | "extra_content")
                    })
                })
                || call["function"].as_object().is_some_and(|function| {
                    function
                        .keys()
                        .any(|key| !matches!(key.as_str(), "name" | "arguments"))
                })
            {
                return Err("上游工具调用缺少标识、名称或有效 JSON 参数");
            }
            if let Some(extra) = call.get("extra_content") {
                let signature = extra
                    .pointer("/google/thought_signature")
                    .and_then(Value::as_str)
                    .filter(|signature| !signature.is_empty());
                if signature.is_none()
                    || extra
                        .as_object()
                        .is_none_or(|object| object.keys().any(|key| key != "google"))
                    || extra["google"]
                        .as_object()
                        .is_none_or(|object| object.keys().any(|key| key != "thought_signature"))
                {
                    return Err("上游 tool_call 含有无法转换的扩展字段");
                }
            }
        }
    }
    if !text
        && !has_tools
        && !message
            .get("refusal")
            .and_then(Value::as_str)
            .is_some_and(|refusal| !refusal.is_empty())
    {
        return Err("上游响应缺少文本、工具调用或拒绝信息");
    }
    if finish_reason == "tool_calls" && !has_tools {
        return Err("上游 finish_reason 表示工具调用，但响应中没有 tool_calls");
    }
    if message
        .get("refusal")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        && message.get("content").is_some_and(|content| match content {
            Value::String(value) => !value.is_empty(),
            Value::Array(items) => items
                .iter()
                .any(|item| item["text"].as_str().is_some_and(|value| !value.is_empty())),
            _ => false,
        })
    {
        return Err("上游同时返回正文和 refusal，无法无损转换");
    }
    if let Some(usage) = chat.get("usage").filter(|usage| !usage.is_null()) {
        if !usage.is_object() {
            return Err("上游 usage 格式无效");
        }
        for field in ["prompt_tokens", "completion_tokens", "total_tokens"] {
            if usage
                .get(field)
                .is_some_and(|value| value.as_u64().is_none())
            {
                return Err("上游 usage token 数量无效");
            }
        }
        if usage.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "prompt_tokens"
                        | "completion_tokens"
                        | "total_tokens"
                        | "prompt_tokens_details"
                        | "completion_tokens_details"
                )
            })
        }) {
            return Err("上游 usage 含有当前协议无法保留的计费字段");
        }
        for (details, fields) in [
            (
                "prompt_tokens_details",
                &[
                    "cached_tokens",
                    "cache_write_tokens",
                    "tool_use_prompt_tokens",
                ][..],
            ),
            ("completion_tokens_details", &["reasoning_tokens"][..]),
        ] {
            if let Some(details) = usage.get(details) {
                if !details.is_object() {
                    return Err("上游 usage token 详情格式无效");
                }
                if details
                    .as_object()
                    .is_some_and(|object| object.keys().any(|key| !fields.contains(&key.as_str())))
                {
                    return Err("上游 usage token 详情含有当前协议无法保留的计费字段");
                }
                for field in fields {
                    if details
                        .get(*field)
                        .is_some_and(|value| value.as_u64().is_none())
                    {
                        return Err("上游 usage token 详情数量无效");
                    }
                }
            }
        }
        if let Some(prompt) = usage.get("prompt_tokens").and_then(Value::as_u64) {
            let cached = usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let written = usage
                .pointer("/prompt_tokens_details/cache_write_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if cached
                .checked_add(written)
                .is_none_or(|details| details > prompt)
            {
                return Err("上游 prompt token 详情超出 prompt_tokens");
            }
            if usage
                .pointer("/prompt_tokens_details/tool_use_prompt_tokens")
                .and_then(Value::as_u64)
                .is_some_and(|tool_prompt| tool_prompt > prompt)
            {
                return Err("上游 tool_use_prompt_tokens 超出 prompt_tokens");
            }
        }
        if let Some(completion) = usage.get("completion_tokens").and_then(Value::as_u64) {
            if usage
                .pointer("/completion_tokens_details/reasoning_tokens")
                .and_then(Value::as_u64)
                .is_some_and(|reasoning| reasoning > completion)
            {
                return Err("上游 reasoning_tokens 超出 completion_tokens");
            }
        }
    }
    Ok(())
}

fn response_text(message: &Value) -> String {
    let text = message["content"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        });
    if text.is_empty() {
        message["refusal"].as_str().unwrap_or("").to_owned()
    } else {
        text
    }
}

fn json_response(status: StatusCode, value: Value) -> AppResult<Response> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&value)?))
        .map_err(|error| AppError::Internal(error.to_string()))
}

fn stream_response(status: StatusCode, source: &HeaderMap, body: String) -> AppResult<Response> {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .map_err(|error| AppError::Internal(error.to_string()))?;
    if let Some(account) = source.get("x-junohub-engine-account-id") {
        response
            .headers_mut()
            .insert("x-junohub-engine-account-id", account.clone());
    }
    Ok(response)
}

pub(crate) fn anthropic_request_to_chat(source: &Value) -> AppResult<Value> {
    // Apply the same contract checks to legacy automatic routes and explicit
    // adapters; otherwise unsupported thinking/cache/tool fields are discarded.
    validate_anthropic_conversion(source)?;
    let model = required_string(source, "model")?;
    let mut messages = Vec::new();
    if let Some(system) = source.get("system") {
        let text = text_blocks(system);
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    let source_messages = source
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("Claude messages 必须是数组".into()))?;
    for message in source_messages {
        let role = required_string(message, "role")?;
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if let Some(text) = content.as_str() {
            messages.push(json!({"role":role,"content":text}));
            continue;
        }
        let mut regular = Vec::new();
        let mut tool_calls = Vec::new();
        for block in content.as_array().into_iter().flatten() {
            match block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "text" => {
                    if !tool_calls.is_empty() {
                        messages.push(json!({"role":role,"content":Value::Null,"tool_calls":std::mem::take(&mut tool_calls)}));
                    }
                    regular.push(json!({"type":"text","text":required_string(block,"text")?}));
                }
                "image" => {
                    if !tool_calls.is_empty() {
                        messages.push(json!({"role":role,"content":Value::Null,"tool_calls":std::mem::take(&mut tool_calls)}));
                    }
                    let image_source = &block["source"];
                    let url = if image_source["type"] == "base64" {
                        format!(
                            "data:{};base64,{}",
                            required_string(image_source, "media_type")?,
                            required_string(image_source, "data")?
                        )
                    } else {
                        required_string(image_source, "url")?.to_owned()
                    };
                    regular.push(json!({"type":"image_url","image_url":{"url":url}}));
                }
                "tool_use" => {
                    if !regular.is_empty() {
                        messages.push(json!({"role":role,"content":Value::Array(std::mem::take(&mut regular))}));
                    }
                    tool_calls.push(json!({"id":required_string(block,"id")?,"type":"function","function":{"name":required_string(block,"name")?,"arguments":serde_json::to_string(&block["input"])?}}));
                }
                "tool_result" => {
                    if !regular.is_empty() || !tool_calls.is_empty() {
                        let mut preceding = json!({"role":role,"content":Value::Array(std::mem::take(&mut regular))});
                        if !tool_calls.is_empty() {
                            preceding["tool_calls"] = Value::Array(std::mem::take(&mut tool_calls));
                        }
                        messages.push(preceding);
                    }
                    messages.push(json!({"role":"tool","tool_call_id":required_string(block,"tool_use_id")?,"content":text_blocks(&block["content"])}));
                }
                _ => {
                    return Err(AppError::BadRequest(
                        "Messages 内容块无法转换到 Chat Completions".into(),
                    ));
                }
            }
        }
        if !regular.is_empty() || !tool_calls.is_empty() {
            let mut converted = json!({"role":role,"content":if regular.is_empty(){Value::Null}else{Value::Array(regular)}});
            if !tool_calls.is_empty() {
                converted["tool_calls"] = Value::Array(tool_calls);
            }
            messages.push(converted);
        }
    }
    let mut chat = json!({"model":model,"messages":messages,"stream":false});
    copy_number(source, &mut chat, "max_tokens", "max_tokens");
    copy_number(source, &mut chat, "temperature", "temperature");
    copy_number(source, &mut chat, "top_p", "top_p");
    if let Some(stop) = source.get("stop_sequences") {
        chat["stop"] = stop.clone();
    }
    if let Some(choice) = source.get("tool_choice") {
        chat["tool_choice"] = match choice["type"].as_str().unwrap_or_default() {
            "auto" => json!("auto"),
            "any" => json!("required"),
            "none" => json!("none"),
            "tool" => json!({"type":"function","function":{"name":choice["name"]}}),
            _ => {
                return Err(AppError::BadRequest(
                    "此 tool_choice 无法转换到 Chat Completions".into(),
                ));
            }
        };
        if choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            chat["parallel_tool_calls"] = json!(false);
        }
    }
    if let Some(tools) = source.get("tools").and_then(Value::as_array) {
        chat["tools"] = Value::Array(tools.iter().map(|tool| -> AppResult<Value> {
            let mut function = json!({"name":required_string(tool,"name")?,"parameters":tool.get("input_schema").cloned().ok_or_else(|| AppError::BadRequest("Messages 工具缺少 input_schema".into()))?});
            if let Some(description) = tool.get("description") { function["description"] = description.clone(); }
            Ok(json!({"type":"function","function":function}))
        }).collect::<AppResult<Vec<_>>>()?);
    }
    Ok(chat)
}

// Explicit adapters reject fields they cannot preserve, before sending a billed request.
pub(crate) fn validate_anthropic_conversion(source: &Value) -> AppResult<()> {
    let object = source
        .as_object()
        .ok_or_else(|| AppError::BadRequest("Messages 请求须为 JSON 对象".into()))?;
    for name in object.keys() {
        if !matches!(
            name.as_str(),
            "model"
                | "messages"
                | "system"
                | "max_tokens"
                | "temperature"
                | "top_p"
                | "stream"
                | "tools"
                | "tool_choice"
                | "stop_sequences"
        ) {
            return Err(AppError::BadRequest(format!(
                "Messages → Chat Completions 暂不支持 {name}，请使用 Anthropic 原生上游"
            )));
        }
    }
    required_string(source, "model")?;
    let messages = source
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("Claude messages 必须是数组".into()))?;
    fn text_content(value: &Value) -> AppResult<()> {
        if value.is_string() {
            return Ok(());
        }
        let parts = value.as_array().ok_or_else(|| {
            AppError::BadRequest("Messages 文本内容必须是字符串或文本块数组".into())
        })?;
        for part in parts {
            if part.get("type").and_then(Value::as_str) != Some("text")
                || part.get("cache_control").is_some()
                || part.as_object().is_none_or(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "text"))
                })
            {
                return Err(AppError::BadRequest(
                    "当前协议转换只支持文本块，请使用 Anthropic 原生上游".into(),
                ));
            }
            required_string(part, "text")?;
        }
        Ok(())
    }
    fn blocks(value: &Value, allowed: &[&str]) -> AppResult<()> {
        let items = value.as_array().ok_or_else(|| {
            AppError::BadRequest("Messages content 必须是字符串或内容块数组".into())
        })?;
        for block in items {
            if !block.is_object() {
                return Err(AppError::BadRequest(
                    "Messages content block 必须是对象".into(),
                ));
            }
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !allowed.contains(&kind) || block.get("cache_control").is_some() {
                return Err(AppError::BadRequest(
                    "当前协议转换不支持该内容块或缓存控制，请使用 Anthropic 原生上游".into(),
                ));
            }
            match kind {
                "text" => {
                    if block.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "text"))
                    }) {
                        return Err(AppError::BadRequest(
                            "Messages text block 包含当前转换不支持的字段".into(),
                        ));
                    }
                    required_string(block, "text")?;
                }
                "image" => {
                    if block.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "source"))
                    }) {
                        return Err(AppError::BadRequest(
                            "Messages image block 包含当前转换不支持的字段".into(),
                        ));
                    }
                    let source = block
                        .get("source")
                        .filter(|value| value.is_object())
                        .ok_or_else(|| AppError::BadRequest("Messages 图片缺少 source".into()))?;
                    match source.get("type").and_then(Value::as_str) {
                        Some("base64") => {
                            if source.as_object().is_some_and(|object| {
                                object.keys().any(|key| {
                                    !matches!(key.as_str(), "type" | "media_type" | "data")
                                })
                            }) {
                                return Err(AppError::BadRequest(
                                    "Messages base64 image source 包含当前转换不支持的字段".into(),
                                ));
                            }
                            required_string(source, "media_type")?;
                            required_string(source, "data")?;
                        }
                        Some("url") => {
                            if source.as_object().is_some_and(|object| {
                                object
                                    .keys()
                                    .any(|key| !matches!(key.as_str(), "type" | "url"))
                            }) {
                                return Err(AppError::BadRequest(
                                    "Messages URL image source 包含当前转换不支持的字段".into(),
                                ));
                            }
                            required_string(source, "url")?;
                        }
                        _ => {
                            return Err(AppError::BadRequest("仅支持 base64 或 URL 图片源".into()));
                        }
                    }
                }
                "tool_use" => {
                    if block.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "id" | "name" | "input"))
                    }) {
                        return Err(AppError::BadRequest(
                            "Messages tool_use 包含当前转换不支持的字段".into(),
                        ));
                    }
                    required_string(block, "id")?;
                    required_string(block, "name")?;
                    if !block.get("input").is_some_and(Value::is_object) {
                        return Err(AppError::BadRequest(
                            "Messages 工具 input 必须是 JSON 对象".into(),
                        ));
                    }
                }
                "tool_result" => {
                    if block.as_object().is_some_and(|object| {
                        object.keys().any(|key| {
                            !matches!(
                                key.as_str(),
                                "type" | "tool_use_id" | "content" | "is_error"
                            )
                        })
                    }) {
                        return Err(AppError::BadRequest(
                            "Messages tool_result 包含当前转换不支持的字段".into(),
                        ));
                    }
                    if !block.get("is_error").is_none_or(|value| value == false) {
                        return Err(AppError::BadRequest(
                            "当前协议无法保留工具错误标记，请使用 Anthropic 原生上游".into(),
                        ));
                    }
                    required_string(block, "tool_use_id")?;
                    text_content(&block["content"])?;
                }
                _ => unreachable!("block kind checked above"),
            }
        }
        Ok(())
    }
    if let Some(system) = source.get("system") {
        text_content(system)?;
    }
    if let Some(max_tokens) = source.get("max_tokens") {
        if !max_tokens.as_u64().is_some_and(|count| count > 0) {
            return Err(AppError::BadRequest("max_tokens 必须是正整数".into()));
        }
    }
    for field in ["temperature", "top_p"] {
        if source.get(field).is_some_and(|value| !value.is_number()) {
            return Err(AppError::BadRequest(format!("{field} 必须是数字")));
        }
    }
    if source
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::BadRequest("stream 必须是布尔值".into()));
    }
    if let Some(stop) = source.get("stop_sequences") {
        if !stop
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err(AppError::BadRequest(
                "stop_sequences 必须是字符串数组".into(),
            ));
        }
    }
    for message in messages {
        let role = required_string(message, "role")?;
        if !matches!(role, "user" | "assistant") {
            return Err(AppError::BadRequest(
                "Messages role 仅支持 user 或 assistant".into(),
            ));
        }
        if message.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "role" | "content"))
        }) {
            return Err(AppError::BadRequest(
                "Messages message 包含当前转换不支持的字段".into(),
            ));
        }
        let content = message
            .get("content")
            .ok_or_else(|| AppError::BadRequest("Messages message 缺少 content".into()))?;
        if content.is_string() {
            continue;
        }
        blocks(content, &["text", "image", "tool_use", "tool_result"])?;
        if let Some(items) = content.as_array() {
            for block in items {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use") if role != "assistant" => {
                        return Err(AppError::BadRequest(
                            "Messages tool_use 必须位于 assistant 消息中".into(),
                        ));
                    }
                    Some("tool_result") if role != "user" => {
                        return Err(AppError::BadRequest(
                            "Messages tool_result 必须位于 user 消息中".into(),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(max_tokens) = source.get("max_tokens") {
        if !max_tokens.as_u64().is_some_and(|value| value > 0) {
            return Err(AppError::BadRequest("max_tokens 必须是正整数".into()));
        }
    }
    if let Some(stream) = source.get("stream") {
        if !stream.is_boolean() {
            return Err(AppError::BadRequest("stream 必须是布尔值".into()));
        }
    }
    for field in ["temperature", "top_p"] {
        if source.get(field).is_some_and(|value| !value.is_number()) {
            return Err(AppError::BadRequest(format!("{field} 必须是数字")));
        }
    }
    if let Some(stop) = source.get("stop_sequences") {
        if !stop
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err(AppError::BadRequest(
                "stop_sequences 必须是字符串数组".into(),
            ));
        }
    }
    if let Some(tools) = source.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| AppError::BadRequest("Messages tools 必须是数组".into()))?;
        for tool in tools {
            if tool.get("type").is_some() || tool.get("cache_control").is_some() {
                return Err(AppError::BadRequest(
                    "当前转换仅支持自定义函数工具，请使用 Anthropic 原生上游调用内置工具或缓存"
                        .into(),
                ));
            }
            required_string(tool, "name")?;
            if !tool.get("input_schema").is_some_and(Value::is_object) {
                return Err(AppError::BadRequest(
                    "Messages 工具缺少 JSON Schema input_schema".into(),
                ));
            }
            if tool.as_object().is_some_and(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "name" | "description" | "input_schema"))
            }) {
                return Err(AppError::BadRequest(
                    "Messages 工具定义包含当前转换不支持的字段".into(),
                ));
            }
            if tool
                .get("description")
                .is_some_and(|description| !description.is_string())
            {
                return Err(AppError::BadRequest(
                    "Messages 工具 description 必须是字符串".into(),
                ));
            }
        }
    }
    if let Some(choice) = source.get("tool_choice") {
        let kind = choice
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::BadRequest("Messages tool_choice.type 无效".into()))?;
        if !matches!(kind, "auto" | "any" | "none" | "tool") {
            return Err(AppError::BadRequest(
                "此 tool_choice 无法转换到 Chat Completions".into(),
            ));
        }
        if kind == "tool" {
            required_string(choice, "name")?;
        }
        if choice.as_object().is_some_and(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "name" | "disable_parallel_tool_use"))
        }) {
            return Err(AppError::BadRequest(
                "Messages tool_choice 包含当前转换不支持的字段".into(),
            ));
        }
        if kind == "tool" {
            let name = required_string(choice, "name")?;
            if !source
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                })
            {
                return Err(AppError::BadRequest(
                    "tool_choice 指定的函数不在 tools 中".into(),
                ));
            }
        }
        if matches!(kind, "any" | "tool")
            && !source
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty())
        {
            return Err(AppError::BadRequest(
                "该 tool_choice 必须同时提供 tools".into(),
            ));
        }
        if choice
            .get("disable_parallel_tool_use")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(AppError::BadRequest(
                "disable_parallel_tool_use 必须是布尔值".into(),
            ));
        }
    }
    Ok(())
}

fn gemini_request_to_chat(source: &Value, model: &str) -> AppResult<Value> {
    let object = source
        .as_object()
        .ok_or_else(|| AppError::BadRequest("Gemini 请求须为 JSON 对象".into()))?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "contents" | "systemInstruction" | "generationConfig" | "tools"
        ) {
            return Err(AppError::BadRequest(format!(
                "Gemini → Chat Completions 暂不支持 {key}，请使用 Gemini 原生上游"
            )));
        }
    }
    let contents = source
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("Gemini contents 必须是数组".into()))?;
    if contents.is_empty() {
        return Err(AppError::BadRequest("Gemini contents 不能为空".into()));
    }
    let mut messages = Vec::new();
    let mut pending_calls: Vec<(String, String)> = Vec::new();
    if let Some(system) = source.get("systemInstruction") {
        let parts = system.get("parts").unwrap_or(system);
        if !parts.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|part| part.get("text").is_some_and(Value::is_string))
        }) {
            return Err(AppError::BadRequest(
                "Gemini systemInstruction 仅支持文本 parts".into(),
            ));
        }
        if system.as_object().is_some_and(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "parts" | "role"))
        }) {
            return Err(AppError::BadRequest(
                "Gemini systemInstruction 包含当前转换不支持的字段".into(),
            ));
        }
        if parts.as_array().is_some_and(|items| {
            items.iter().any(|part| {
                part.as_object()
                    .is_none_or(|object| object.keys().any(|key| key != "text"))
            })
        }) {
            return Err(AppError::BadRequest(
                "Gemini systemInstruction 只支持纯文本 parts".into(),
            ));
        }
        let text = text_blocks(parts);
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    for content in contents {
        if content.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "role" | "parts"))
        }) {
            return Err(AppError::BadRequest(
                "Gemini content 包含当前转换不支持的字段".into(),
            ));
        }
        let role = match content
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
        {
            "model" => "assistant",
            "user" => "user",
            _ => {
                return Err(AppError::BadRequest(
                    "Gemini content role 仅支持 user 或 model".into(),
                ));
            }
        };
        let parts = content
            .get("parts")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::BadRequest("Gemini content.parts 必须是数组".into()))?;
        if parts.is_empty() {
            return Err(AppError::BadRequest("Gemini content.parts 不能为空".into()));
        }
        let mut blocks = Vec::new();
        let mut tool_calls = Vec::new();
        for part in parts {
            let supported = [
                "text",
                "inlineData",
                "inline_data",
                "functionCall",
                "functionResponse",
                "thoughtSignature",
            ];
            if part
                .as_object()
                .is_none_or(|object| object.keys().any(|key| !supported.contains(&key.as_str())))
            {
                return Err(AppError::BadRequest(
                    "Gemini part 包含当前转换不支持的字段".into(),
                ));
            }
            let variants = [
                "text",
                "inlineData",
                "inline_data",
                "functionCall",
                "functionResponse",
            ]
            .iter()
            .filter(|key| part.get(**key).is_some())
            .count();
            if variants != 1 {
                return Err(AppError::BadRequest(
                    "Gemini part 必须且只能包含一种内容".into(),
                ));
            }
            if part.get("thoughtSignature").is_some() && part.get("functionCall").is_none() {
                return Err(AppError::BadRequest(
                    "Gemini thoughtSignature 只能随 functionCall 转换".into(),
                ));
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if role == "assistant" && !tool_calls.is_empty() {
                    messages.push(json!({"role":"assistant","content":Value::Null,"tool_calls":std::mem::take(&mut tool_calls)}));
                }
                blocks.push(json!({"type":"text","text":text}));
            } else if part.get("text").is_some() {
                return Err(AppError::BadRequest("Gemini part.text 必须是字符串".into()));
            }
            if let Some(data) = part.get("inlineData").or_else(|| part.get("inline_data")) {
                if data.as_object().is_none_or(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "mimeType" | "mime_type" | "data"))
                }) || (data.get("mimeType").is_some() && data.get("mime_type").is_some())
                {
                    return Err(AppError::BadRequest("Gemini inlineData 格式无效".into()));
                }
                blocks.push(json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",required_string(data,"mimeType").or_else(|_| required_string(data,"mime_type"))?,required_string(data,"data")?)}}));
            }
            if let Some(call) = part.get("functionCall") {
                if role != "assistant" {
                    return Err(AppError::BadRequest(
                        "Gemini functionCall 须属于 model 消息".into(),
                    ));
                }
                if call.as_object().is_none_or(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "id" | "name" | "args"))
                }) {
                    return Err(AppError::BadRequest(
                        "Gemini functionCall 包含当前转换不支持的字段".into(),
                    ));
                }
                let name = required_string(call, "name")?;
                let id = match call.get("id") {
                    None => format!("call_{}", Uuid::new_v4().simple()),
                    Some(_) => required_string(call, "id")?.to_owned(),
                };
                if pending_calls.iter().any(|(_, pending)| pending == &id) {
                    return Err(AppError::BadRequest(
                        "Gemini functionCall 的 id 重复".into(),
                    ));
                }
                let args = call
                    .get("args")
                    .cloned()
                    .ok_or_else(|| AppError::BadRequest("Gemini functionCall 缺少 args".into()))?;
                if !args.is_object() {
                    return Err(AppError::BadRequest("Gemini 函数参数须为 JSON 对象".into()));
                }
                if !blocks.is_empty() {
                    messages.push(json!({"role":"assistant","content":Value::Array(std::mem::take(&mut blocks))}));
                }
                pending_calls.push((name.to_owned(), id.clone()));
                let mut converted = json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}});
                if let Some(signature) = part.get("thoughtSignature") {
                    let signature = signature
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            AppError::BadRequest("Gemini thoughtSignature 必须是非空字符串".into())
                        })?;
                    converted["extra_content"] = json!({"google":{"thought_signature":signature}});
                }
                tool_calls.push(converted);
            }
            if let Some(result) = part.get("functionResponse") {
                if role != "user" {
                    return Err(AppError::BadRequest(
                        "Gemini functionResponse 须属于 user 消息".into(),
                    ));
                }
                if part.get("thoughtSignature").is_some()
                    || result.as_object().is_none_or(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "id" | "name" | "response"))
                    })
                {
                    return Err(AppError::BadRequest(
                        "Gemini functionResponse 包含当前转换不支持的字段".into(),
                    ));
                }
                let name = required_string(result, "name")?;
                let explicit_id = if result.get("id").is_some() {
                    Some(required_string(result, "id")?)
                } else {
                    None
                };
                if explicit_id.is_none()
                    && pending_calls
                        .iter()
                        .filter(|(pending_name, _)| pending_name == name)
                        .count()
                        > 1
                {
                    return Err(AppError::BadRequest(
                        "存在多个同名函数调用，请在 Gemini 函数结果中提供对应 id".into(),
                    ));
                }
                let index = pending_calls
                    .iter()
                    .position(|(pending_name, id)| {
                        pending_name == name && explicit_id.is_none_or(|expected| expected == id)
                    })
                    .ok_or_else(|| {
                        AppError::BadRequest(
                            "Gemini 函数结果找不到对应调用，请检查 name 和 id".into(),
                        )
                    })?;
                let (_, id) = pending_calls.remove(index);
                let response = result
                    .get("response")
                    .filter(|response| response.is_object())
                    .ok_or_else(|| {
                        AppError::BadRequest("Gemini 函数结果须包含 JSON 对象 response".into())
                    })?;
                if !blocks.is_empty() {
                    messages.push(
                        json!({"role":"user","content":Value::Array(std::mem::take(&mut blocks))}),
                    );
                }
                messages
                    .push(json!({"role":"tool","tool_call_id":id,"content":response.to_string()}));
            }
        }
        if !blocks.is_empty() || !tool_calls.is_empty() {
            let mut message = json!({"role":role,"content":if blocks.is_empty(){Value::Null}else{Value::Array(blocks)}});
            if !tool_calls.is_empty() {
                message["tool_calls"] = Value::Array(tool_calls);
            }
            messages.push(message);
        }
    }
    let mut chat = json!({"model":model,"messages":messages,"stream":false});
    if let Some(config) = source.get("generationConfig") {
        let config = config
            .as_object()
            .ok_or_else(|| AppError::BadRequest("Gemini generationConfig 必须是对象".into()))?;
        for key in config.keys() {
            if !matches!(
                key.as_str(),
                "temperature" | "maxOutputTokens" | "topP" | "stopSequences"
            ) {
                return Err(AppError::BadRequest(format!(
                    "Gemini generationConfig.{key} 暂不支持协议转换，请使用 Gemini 原生上游"
                )));
            }
        }
        for field in ["temperature", "topP"] {
            if config.get(field).is_some_and(|value| !value.is_number()) {
                return Err(AppError::BadRequest(format!(
                    "Gemini generationConfig.{field} 必须是数字"
                )));
            }
        }
        if config
            .get("maxOutputTokens")
            .is_some_and(|value| !value.as_u64().is_some_and(|count| count > 0))
        {
            return Err(AppError::BadRequest(
                "Gemini generationConfig.maxOutputTokens 必须是正整数".into(),
            ));
        }
        let config = Value::Object(config.clone());
        copy_number(&config, &mut chat, "temperature", "temperature");
        copy_number(&config, &mut chat, "maxOutputTokens", "max_tokens");
        copy_number(&config, &mut chat, "topP", "top_p");
        if let Some(stop) = config.get("stopSequences") {
            if !stop
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
            {
                return Err(AppError::BadRequest(
                    "Gemini stopSequences 必须是字符串数组".into(),
                ));
            }
            chat["stop"] = stop.clone();
        }
    }
    if let Some(groups) = source.get("tools") {
        let groups = groups
            .as_array()
            .ok_or_else(|| AppError::BadRequest("Gemini tools 必须是数组".into()))?;
        let mut declarations = Vec::new();
        for group in groups {
            if group
                .as_object()
                .is_none_or(|object| object.keys().any(|key| key != "functionDeclarations"))
            {
                return Err(AppError::BadRequest(
                    "Gemini 工具组包含当前转换不支持的类型".into(),
                ));
            }
            let functions = group
                .get("functionDeclarations")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AppError::BadRequest("Gemini 仅支持 functionDeclarations 工具".into())
                })?;
            for function in functions {
                if function.as_object().is_none_or(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "name" | "description" | "parameters"))
                }) {
                    return Err(AppError::BadRequest(
                        "Gemini functionDeclaration 包含当前转换不支持的字段".into(),
                    ));
                }
                let name = required_string(function, "name")?;
                if !function.get("parameters").is_some_and(Value::is_object) {
                    return Err(AppError::BadRequest(
                        "Gemini 函数缺少 JSON Schema parameters".into(),
                    ));
                }
                let mut spec = json!({"name":name,"parameters":function["parameters"]});
                if let Some(description) = function.get("description") {
                    spec["description"] = description.clone();
                }
                declarations.push(json!({"type":"function","function":spec}));
            }
        }
        chat["tools"] = Value::Array(declarations);
    }
    Ok(chat)
}

fn chat_response_to_anthropic(chat: &Value, request: &Value) -> AppResult<Value> {
    let model = required_string(request, "model")
        .map_err(|_| AppError::BadGateway("无法从原始请求恢复 Anthropic model".into()))?;
    let choice = chat
        .pointer("/choices/0")
        .expect("validated chat response has a first choice");
    let message = &choice["message"];
    let mut content = Vec::new();
    let text = response_text(message);
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let arguments = serde_json::from_str::<Value>(
                call["function"]["arguments"]
                    .as_str()
                    .expect("validated tool arguments are strings"),
            )
            .expect("validated tool arguments are JSON objects");
            content.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":arguments}));
        }
    }
    let stop_reason = if content.iter().any(|block| block["type"] == "tool_use") {
        "tool_use"
    } else if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        "max_tokens"
    } else if message
        .get("refusal")
        .and_then(Value::as_str)
        .is_some_and(|refusal| !refusal.is_empty())
        || choice.get("finish_reason").and_then(Value::as_str) == Some("content_filter")
    {
        "refusal"
    } else {
        "end_turn"
    };
    let mut result = json!({
        "id":chat.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned).unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().simple())),
        "type":"message","role":"assistant","model":model,
        "content":content,"stop_reason":stop_reason,"stop_sequence":null
    });
    let mut usage = serde_json::Map::new();
    if let Some(prompt) = chat.pointer("/usage/prompt_tokens").and_then(Value::as_u64) {
        let cached = chat
            .pointer("/usage/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let written = chat
            .pointer("/usage/prompt_tokens_details/cache_write_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        usage.insert(
            "input_tokens".into(),
            Value::from(prompt - cached - written),
        );
    }
    if let Some(cached) = chat
        .pointer("/usage/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
    {
        usage.insert("cache_read_input_tokens".into(), Value::from(cached));
    }
    if let Some(written) = chat
        .pointer("/usage/prompt_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64)
    {
        usage.insert("cache_creation_input_tokens".into(), Value::from(written));
    }
    if let Some(count) = chat
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64)
    {
        usage.insert("output_tokens".into(), Value::from(count));
    }
    if !usage.is_empty() {
        result["usage"] = Value::Object(usage);
    }
    Ok(result)
}

fn chat_response_to_gemini(chat: &Value, model: &str) -> Value {
    let message = &chat["choices"][0]["message"];
    let mut parts = Vec::new();
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        parts.push(json!({"thought":true,"text":reasoning}));
    }
    let text = response_text(message);
    if !text.is_empty() {
        parts.push(json!({"text":text}));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let args = serde_json::from_str::<Value>(
                call["function"]["arguments"]
                    .as_str()
                    .expect("validated tool arguments are strings"),
            )
            .expect("validated tool arguments are JSON objects");
            let mut part = json!({"functionCall":{"id":call["id"],"name":call["function"]["name"],"args":args}});
            if let Some(signature) = call
                .pointer("/extra_content/google/thought_signature")
                .and_then(Value::as_str)
            {
                part["thoughtSignature"] = Value::String(signature.to_owned());
            }
            parts.push(part);
        }
    }
    let finish = match chat
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
    {
        Some("length") => "MAX_TOKENS",
        Some("content_filter") => "SAFETY",
        _ => "STOP",
    };
    let response_id = chat
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("response_{}", Uuid::new_v4().simple()));
    let mut result = json!({"responseId":response_id,"candidates":[{"content":{"role":"model","parts":parts},"finishReason":finish,"index":0}],"modelVersion":model});
    let mut usage = serde_json::Map::new();
    let prompt = chat.pointer("/usage/prompt_tokens").and_then(Value::as_u64);
    let tool_prompt = chat
        .pointer("/usage/prompt_tokens_details/tool_use_prompt_tokens")
        .and_then(Value::as_u64);
    if let Some(prompt) = prompt {
        usage.insert(
            "promptTokenCount".into(),
            Value::from(prompt.saturating_sub(tool_prompt.unwrap_or(0))),
        );
    }
    if let Some(tool_prompt) = tool_prompt {
        usage.insert("toolUsePromptTokenCount".into(), Value::from(tool_prompt));
    }
    if let Some(cached) = chat
        .pointer("/usage/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
    {
        usage.insert("cachedContentTokenCount".into(), Value::from(cached));
    }
    let completion = chat
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64);
    let reasoning = chat
        .pointer("/usage/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64);
    if let Some(completion) = completion {
        usage.insert(
            "candidatesTokenCount".into(),
            Value::from(completion.saturating_sub(reasoning.unwrap_or(0))),
        );
    }
    if let Some(reasoning) = reasoning {
        usage.insert("thoughtsTokenCount".into(), Value::from(reasoning));
    }
    if let Some(total) = chat.pointer("/usage/total_tokens").and_then(Value::as_u64) {
        usage.insert("totalTokenCount".into(), Value::from(total));
    }
    if !usage.is_empty() {
        result["usageMetadata"] = Value::Object(usage);
    }
    result
}

fn anthropic_sse(message: &Value) -> AppResult<String> {
    let mut frames = Vec::new();
    let mut start = message.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    let output_tokens = message
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64);
    frames.push((
        "message_start",
        json!({"type":"message_start","message":start}),
    ));
    for (index, block) in message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let initial = if block["type"] == "text" {
            json!({"type":"text","text":""})
        } else {
            json!({"type":"tool_use","id":block["id"],"name":block["name"],"input":{}})
        };
        frames.push((
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":initial}),
        ));
        let delta = if block["type"] == "text" {
            json!({"type":"text_delta","text":block["text"]})
        } else {
            json!({"type":"input_json_delta","partial_json":serde_json::to_string(&block["input"])?})
        };
        frames.push((
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":delta}),
        ));
        frames.push((
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
    }
    let mut delta = json!({"type":"message_delta","delta":{"stop_reason":message["stop_reason"],"stop_sequence":null}});
    if let Some(output_tokens) = output_tokens {
        delta["usage"] = json!({"output_tokens":output_tokens});
    }
    frames.push(("message_delta", delta));
    frames.push(("message_stop", json!({"type":"message_stop"})));
    Ok(frames
        .into_iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect())
}

fn anthropic_error(chat: &Value) -> Value {
    json!({"type":"error","error":{"type":"api_error","message":error_message(chat)}})
}

fn gemini_error(chat: &Value, status: StatusCode) -> Value {
    json!({"error":{"code":status.as_u16(),"message":error_message(chat),"status":"UNKNOWN"}})
}

fn error_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .unwrap_or("请求失败")
        .to_string()
}

fn required_string<'a>(source: &'a Value, key: &str) -> AppResult<&'a str> {
    source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest(format!("{key} 不能为空")))
}

fn text_blocks(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy_number(source: &Value, target: &mut Value, from: &str, to: &str) {
    if source.get(from).is_some_and(Value::is_number) {
        target[to] = source[from].clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functional_recheck_gemini_tool_results_match_their_calls() {
        let source = json!({"contents":[
            {"role":"model","parts":[{"functionCall":{"id":"first","name":"f","args":{"x":1}}},{"functionCall":{"id":"second","name":"f","args":{"x":2}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"second","name":"f","response":{"x":2}}},{"functionResponse":{"name":"f","response":{"x":1}}}]}
        ]});
        let chat = gemini_request_to_chat(&source, "alias").unwrap();
        assert_eq!(chat["messages"][0]["tool_calls"][0]["id"], "first");
        assert_eq!(chat["messages"][1]["tool_call_id"], "second");
        assert_eq!(chat["messages"][2]["tool_call_id"], "first");
        let without_ids = json!({"contents":[
            {"role":"model","parts":[{"functionCall":{"name":"f","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"name":"f","response":{"ok":true}}}]}
        ]});
        let chat = gemini_request_to_chat(&without_ids, "alias").unwrap();
        assert_eq!(
            chat["messages"][0]["tool_calls"][0]["id"],
            chat["messages"][1]["tool_call_id"]
        );
        for result in [
            json!({"name":"other","response":{}}),
            json!({"name":"f","id":"missing","response":{}}),
        ] {
            let mut broken = source.clone();
            broken["contents"][1]["parts"] = json!([{"functionResponse":result}]);
            assert!(gemini_request_to_chat(&broken, "alias").is_err());
        }
        let orphan = json!({"contents":[{"role":"user","parts":[{"functionResponse":{"name":"f","response":{}}}]}]});
        assert!(gemini_request_to_chat(&orphan, "alias").is_err());
        let mut ambiguous = source.clone();
        ambiguous["contents"][1]["parts"] =
            json!([{"functionResponse":{"name":"f","response":{}}}]);
        assert!(gemini_request_to_chat(&ambiguous, "alias").is_err());
        let reply = json!({"choices":[{"message":{"tool_calls":[{"id":"reply-id","function":{"name":"f","arguments":"{}"}}]}}]});
        assert_eq!(
            chat_response_to_gemini(&reply, "alias")["candidates"][0]["content"]["parts"][0]
                ["functionCall"]["id"],
            "reply-id"
        );
    }

    #[tokio::test]
    async fn functional_recheck_conversion_preserves_errors_and_rejects_invalid_results() {
        for (status, body, expected) in [
            (429, "upstream private non-json error", 429),
            (401, "<html>upstream private error</html>", 401),
            (200, "<html>not an API</html>", 502),
            (200, "{}", 502),
            (200, "{\"error\":{\"message\":\"failed\"}}", 502),
            (
                200,
                "{\"choices\":[{\"message\":{\"tool_calls\":[{\"id\":\"call-1\",\"function\":{\"name\":\"f\",\"arguments\":\"broken\"}}]}}]}",
                502,
            ),
        ] {
            for anthropic in [true, false] {
                let response = Response::builder()
                    .status(status)
                    .body(Body::from(body))
                    .unwrap();
                let response = if anthropic {
                    adapt_anthropic_response(response, &json!({"model":"alias"}), false)
                        .await
                        .unwrap()
                } else {
                    adapt_gemini_response(response, "alias", false)
                        .await
                        .unwrap()
                };
                assert_eq!(response.status().as_u16(), expected);
                let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                assert!(value.get("error").is_some());
                assert!(!String::from_utf8_lossy(&bytes).contains("upstream private"));
            }
        }
    }

    #[tokio::test]
    async fn functional_recheck_conversion_keeps_text_refusals_and_stop_reason() {
        for (message, expected) in [
            (
                json!({"content":[{"type":"text","text":"first"},{"type":"text","text":" second"}]}),
                "first second",
            ),
            (
                json!({"content":null,"refusal":"Cannot comply"}),
                "Cannot comply",
            ),
        ] {
            let chat = json!({"choices":[{"message":message,"finish_reason":"length"}],"usage":{"prompt_tokens":2,"completion_tokens":3}});
            let response = adapt_anthropic_response(
                Response::new(Body::from(chat.to_string())),
                &json!({"model":"alias"}),
                false,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), 200);
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap())
                    .unwrap();
            assert_eq!(value["content"][0]["text"], expected);
            assert_eq!(value["stop_reason"], "max_tokens");
            let response =
                adapt_gemini_response(Response::new(Body::from(chat.to_string())), "alias", false)
                    .await
                    .unwrap();
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap())
                    .unwrap();
            assert_eq!(
                value["candidates"][0]["content"]["parts"][0]["text"],
                expected
            );
            assert_eq!(value["candidates"][0]["finishReason"], "MAX_TOKENS");
        }
        let tool = json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"f","arguments":"{\"x\":1}"}}]},"finish_reason":"tool_calls"}]});
        let response = adapt_anthropic_response(
            Response::new(Body::from(tool.to_string())),
            &json!({"model":"alias"}),
            true,
        )
        .await
        .unwrap();
        let body = String::from_utf8(
            to_bytes(response.into_body(), 10000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("tool_use"));
        assert!(body.contains("call-1"));
        assert!(body.contains("input_json_delta"));
    }

    #[test]
    fn gateway_audit_legacy_conversion_rejects_unsupported_fields_before_dispatch() {
        let request =
            json!({"model":"legacy","messages":[{"role":"user","content":"hi"}],"max_tokens":16});
        assert!(anthropic_request_to_chat(&request).is_ok());
        let mut thinking = request.clone();
        thinking["thinking"] = json!({"type":"enabled","budget_tokens":1000});
        assert!(anthropic_request_to_chat(&thinking).is_err());
        let mut cached = request.clone();
        cached["system"] =
            json!([{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}]);
        assert!(anthropic_request_to_chat(&cached).is_err());
        let mut native_tool = request;
        native_tool["tools"] = json!([{"type":"web_search_20250305","name":"web_search"}]);
        assert!(anthropic_request_to_chat(&native_tool).is_err());
    }

    #[test]
    fn converts_anthropic_messages_and_tools_to_chat() {
        let chat = anthropic_request_to_chat(&json!({"model":"route","system":"Be brief","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]},{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"search","input":{"q":"x"}}]}],"tools":[{"name":"search","input_schema":{"type":"object"}}]})).unwrap();
        assert_eq!(chat["model"], "route");
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(
            chat["messages"][2]["tool_calls"][0]["function"]["name"],
            "search"
        );
        assert_eq!(chat["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn converts_chat_response_to_native_shapes() {
        let chat = json!({"id":"chat-1","choices":[{"message":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}});
        let anthropic = chat_response_to_anthropic(&chat, &json!({"model":"route"})).unwrap();
        assert_eq!(anthropic["content"][0]["text"], "done");
        assert_eq!(anthropic["usage"]["output_tokens"], 2);
        let gemini = chat_response_to_gemini(&chat, "route");
        assert_eq!(
            gemini["candidates"][0]["content"]["parts"][0]["text"],
            "done"
        );
        assert_eq!(gemini["usageMetadata"]["totalTokenCount"], 5);
    }
}
