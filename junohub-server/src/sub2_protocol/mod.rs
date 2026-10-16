//! Pure request/response conversion for the account-pool gateway.
//! Network dispatch and lease ownership live in sub2_gateway; incremental
//! Responses reconciliation lives in sub2_chat_bridge and SSE framing in sub2_sse.
use serde_json::Value;

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EngineProtocol {
    OpenAiCodex,
    OpenAiCompatible,
    Anthropic,
    Gemini,
    ClaudeOAuth,
    GeminiOAuth,
    GrokOAuth,
    Antigravity,
}

pub(crate) fn is_anthropic(protocol: EngineProtocol) -> bool {
    matches!(
        protocol,
        EngineProtocol::Anthropic | EngineProtocol::ClaudeOAuth
    )
}
pub(crate) fn is_gemini(protocol: EngineProtocol) -> bool {
    matches!(
        protocol,
        EngineProtocol::Gemini | EngineProtocol::GeminiOAuth | EngineProtocol::Antigravity
    )
}

pub(crate) fn protocol_error_to_openai(body: Value) -> Value {
    let message = response_error_message(&body);
    let kind = body
        .pointer("/error/type")
        .or_else(|| body.pointer("/error/status"))
        .and_then(Value::as_str)
        .unwrap_or("api_error");
    serde_json::json!({"error":{"message":message,"type":kind}})
}

pub(crate) fn has_upstream_error(body: &Value) -> bool {
    body.get("error").is_some_and(|error| !error.is_null()) || body["type"] == "error"
}

pub(crate) fn response_error_message(body: &Value) -> String {
    [
        body.pointer("/response/error/message"),
        body.pointer("/error/message"),
        body.get("message"),
        body.get("error"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|message| !message.trim().is_empty())
    .unwrap_or("上游请求失败")
    .chars()
    .take(400)
    .collect()
}

fn token_count(usage: &Value, field: &str) -> Option<i64> {
    usage
        .get(field)
        .and_then(Value::as_i64)
        .filter(|count| *count >= 0)
}

fn chat_usage_counts(input: Option<i64>, output: Option<i64>) -> Value {
    if input.is_none() && output.is_none() {
        return Value::Null;
    }
    let mut usage = serde_json::Map::new();
    if let Some(input) = input {
        usage.insert("prompt_tokens".into(), input.into());
    }
    if let Some(output) = output {
        usage.insert("completion_tokens".into(), output.into());
    }
    if let (Some(input), Some(output)) = (input, output) {
        usage.insert("total_tokens".into(), input.saturating_add(output).into());
    }
    Value::Object(usage)
}

pub(crate) fn responses_chat_usage(response: &Value) -> Value {
    let source = response.get("usage").unwrap_or(&Value::Null);
    let mut usage = chat_usage_counts(
        token_count(source, "input_tokens"),
        token_count(source, "output_tokens"),
    );
    if usage.is_null() && token_count(source, "total_tokens").is_some() {
        usage = serde_json::json!({});
    }
    if let Some(total) = token_count(source, "total_tokens") {
        usage["total_tokens"] = Value::from(total);
    }
    if !usage.is_null() {
        for (source_key, target_key) in [
            ("input_tokens_details", "prompt_tokens_details"),
            ("output_tokens_details", "completion_tokens_details"),
        ] {
            if let Some(details) = source.get(source_key).filter(|value| value.is_object()) {
                usage[target_key] = details.clone();
            }
        }
    }
    usage
}

/// Anthropic reports fresh input, cache reads, and cache writes separately.
/// Chat Completions prompt_tokens includes all three buckets.
pub(crate) fn anthropic_chat_usage(source: &Value) -> Value {
    let cached = token_count(source, "cache_read_input_tokens");
    let written = token_count(source, "cache_creation_input_tokens");
    let input = token_count(source, "input_tokens").map(|input| {
        input
            .saturating_add(cached.unwrap_or(0))
            .saturating_add(written.unwrap_or(0))
    });
    let mut usage = chat_usage_counts(input, token_count(source, "output_tokens"));
    if !usage.is_null() && (cached.is_some() || written.is_some()) {
        let mut details = serde_json::Map::new();
        if let Some(cached) = cached {
            details.insert("cached_tokens".into(), cached.into());
        }
        if let Some(written) = written {
            details.insert("cache_write_tokens".into(), written.into());
        }
        usage["prompt_tokens_details"] = Value::Object(details);
    }
    usage
}

pub(crate) fn responses_finish_reason(
    response: &Value,
    terminal_kind: Option<&str>,
    has_tools: bool,
    has_refusal: bool,
) -> &'static str {
    if terminal_kind == Some("response.incomplete") || response["status"] == "incomplete" {
        if response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            == Some("content_filter")
        {
            "content_filter"
        } else {
            "length"
        }
    } else if has_tools {
        "tool_calls"
    } else if has_refusal {
        "content_filter"
    } else {
        "stop"
    }
}

pub(crate) fn native_response_to_chat_completion(
    body: &Value,
    request: &Value,
    protocol: EngineProtocol,
) -> AppResult<Value> {
    if has_upstream_error(body) {
        return Err(AppError::BadGateway(response_error_message(body)));
    }
    if request
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(|model| model.trim().is_empty())
    {
        return Err(AppError::BadRequest("model 必须是非空字符串".into()));
    }
    if is_anthropic(protocol) && body.get("content").is_some_and(Value::is_array) {
        validate_anthropic_response(body)?;
        return Ok(anthropic_to_chat_completion(body, request));
    }
    if is_gemini(protocol)
        && (body.pointer("/candidates/0").is_some_and(Value::is_object)
            || gemini_prompt_blocked(body))
    {
        if !gemini_prompt_blocked(body) {
            validate_gemini_response(body)?;
        }
        return Ok(gemini_to_chat_completion(body, request));
    }
    Err(AppError::BadGateway("上游响应缺少有效的模型结果".into()))
}

fn validate_anthropic_response(body: &Value) -> AppResult<()> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadGateway("Anthropic 响应缺少 content 数组".into()))?;
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") if block.get("text").is_some_and(Value::is_string) => {
                if block.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "text"))
                }) {
                    return Err(AppError::BadGateway(
                        "Anthropic text block 含有无法转换的扩展字段".into(),
                    ));
                }
            }
            Some("tool_use") => {
                for field in ["id", "name"] {
                    chat_required_string(block, field).map_err(|_| {
                        AppError::BadGateway(format!("Anthropic tool_use 缺少有效 {field}"))
                    })?;
                }
                if !block.get("input").is_some_and(Value::is_object) {
                    return Err(AppError::BadGateway(
                        "Anthropic tool_use.input 必须是 JSON 对象".into(),
                    ));
                }
                if block.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "id" | "name" | "input"))
                }) {
                    return Err(AppError::BadGateway(
                        "Anthropic tool_use 含有无法转换的扩展字段".into(),
                    ));
                }
            }
            _ => {
                return Err(AppError::BadGateway(
                    "Anthropic 返回了当前网关无法转换的内容块".into(),
                ));
            }
        }
    }
    if content.is_empty()
        && !matches!(
            body.get("stop_reason").and_then(Value::as_str),
            Some("refusal")
        )
    {
        return Err(AppError::BadGateway(
            "Anthropic 响应没有可转换的内容".into(),
        ));
    }
    let stop_reason = body
        .get("stop_reason")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadGateway("Anthropic 响应缺少有效 stop_reason".into()))?;
    if !matches!(
        stop_reason,
        "end_turn" | "max_tokens" | "stop_sequence" | "tool_use" | "refusal"
    ) {
        return Err(AppError::BadGateway(format!(
            "Anthropic stop_reason {stop_reason} 暂不支持转换"
        )));
    }
    if let Some(server_tool_use) = body.pointer("/usage/server_tool_use") {
        let counts = server_tool_use.as_object().ok_or_else(|| {
            AppError::BadGateway("Anthropic usage.server_tool_use 格式无效".into())
        })?;
        if counts
            .values()
            .any(|value| value.as_u64().is_none_or(|count| count > 0))
        {
            return Err(AppError::BadGateway(
                "Anthropic 响应包含额外的 server-side 工具计费；当前网关不能准确结算该费用".into(),
            ));
        }
    }
    validate_reported_counts(
        body.get("usage"),
        &[
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ],
        "Anthropic",
    )
}

fn validate_gemini_response(body: &Value) -> AppResult<()> {
    let candidates = body
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadGateway("Gemini 响应缺少 candidates 数组".into()))?;
    if candidates.len() != 1 {
        return Err(AppError::BadGateway(
            "当前 Chat 兼容响应只支持一个 Gemini candidate".into(),
        ));
    }
    let candidate = &candidates[0];
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadGateway("Gemini candidate 缺少 content.parts".into()))?;
    let finish_reason = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadGateway("Gemini candidate 缺少有效 finishReason".into()))?;
    if parts.is_empty()
        && !matches!(
            finish_reason,
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY"
        )
    {
        return Err(AppError::BadGateway(
            "Gemini candidate 没有可转换的内容".into(),
        ));
    }
    for part in parts {
        if !part.is_object() {
            return Err(AppError::BadGateway("Gemini response part 格式无效".into()));
        }
        if let Some(text) = part.get("text") {
            if !text.is_string() || part.get("functionCall").is_some() {
                return Err(AppError::BadGateway("Gemini response part 无法转换".into()));
            }
            if part.get("thought").is_some_and(|value| !value.is_boolean()) {
                return Err(AppError::BadGateway("Gemini thought 标记格式无效".into()));
            }
            if part.get("thoughtSignature").is_some() {
                return Err(AppError::BadGateway(
                    "Gemini text thoughtSignature 无法用 Chat 格式保留".into(),
                ));
            }
            if part.as_object().is_some_and(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "text" | "thought"))
            }) {
                return Err(AppError::BadGateway(
                    "Gemini text part 含有无法转换的扩展字段".into(),
                ));
            }
        } else if let Some(call) = part.get("functionCall") {
            chat_required_string(call, "name")
                .map_err(|_| AppError::BadGateway("Gemini functionCall 缺少 name".into()))?;
            if !call.get("args").is_some_and(Value::is_object) {
                return Err(AppError::BadGateway(
                    "Gemini functionCall 缺少 JSON 对象 args".into(),
                ));
            }
            if call.as_object().is_none_or(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "id" | "name" | "args"))
            }) {
                return Err(AppError::BadGateway(
                    "Gemini functionCall 含有无法转换的扩展字段".into(),
                ));
            }
            if part.get("thoughtSignature").is_some_and(|value| {
                !value
                    .as_str()
                    .is_some_and(|signature| !signature.is_empty())
            }) {
                return Err(AppError::BadGateway(
                    "Gemini thoughtSignature 格式无效".into(),
                ));
            }
            if part.as_object().is_none_or(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "functionCall" | "thoughtSignature"))
            }) {
                return Err(AppError::BadGateway(
                    "Gemini functionCall part 含有无法转换的扩展字段".into(),
                ));
            }
        } else {
            return Err(AppError::BadGateway(
                "Gemini 返回了当前 Chat 兼容响应不支持的内容".into(),
            ));
        }
    }
    if !matches!(
        finish_reason,
        "STOP"
            | "MAX_TOKENS"
            | "SAFETY"
            | "RECITATION"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "OTHER"
    ) {
        return Err(AppError::BadGateway(format!(
            "Gemini finishReason {finish_reason} 暂不支持转换"
        )));
    }
    validate_reported_counts(
        body.get("usageMetadata"),
        &[
            "promptTokenCount",
            "cachedContentTokenCount",
            "candidatesTokenCount",
            "toolUsePromptTokenCount",
            "thoughtsTokenCount",
            "totalTokenCount",
        ],
        "Gemini",
    )
}

fn validate_reported_counts(
    source: Option<&Value>,
    fields: &[&str],
    provider: &str,
) -> AppResult<()> {
    let Some(source) = source.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    if !source.is_object() {
        return Err(AppError::BadGateway(format!("{provider} usage 格式无效")));
    }
    for field in fields {
        if source
            .get(*field)
            .is_some_and(|value| value.as_u64().is_none_or(|count| count > i64::MAX as u64))
        {
            return Err(AppError::BadGateway(format!(
                "{provider} usage.{field} 必须是非负整数"
            )));
        }
    }
    Ok(())
}

pub(crate) fn gemini_prompt_blocked(body: &Value) -> bool {
    body.pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED")
}

pub(crate) fn chat_to_responses(payload: &Value) -> AppResult<Value> {
    reject_chat_fields(
        payload,
        &[
            "model",
            "messages",
            "stream",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "store",
            "reasoning",
            "reasoning_effort",
        ],
        "Responses",
    )?;
    let model = chat_required_string(payload, "model")?;
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        if !message.is_object() {
            return Err(AppError::BadRequest("messages 中的项目必须是对象".into()));
        }
        let role = chat_required_string(message, "role")?;
        if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
            return Err(AppError::BadRequest(format!("不支持的 Chat role: {role}")));
        }
        validate_chat_message_fields(message, role)?;
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if matches!(role, "system" | "developer") {
            instructions.push(chat_text_content(&content, "system/developer")?);
            continue;
        }
        if role == "tool" {
            input.push(serde_json::json!({
                "type":"function_call_output",
                "call_id":chat_required_string(message, "tool_call_id")?,
                "output":chat_text_content(&content, "tool")?
            }));
            continue;
        }
        if message.get("reasoning_content").is_some() {
            return Err(AppError::BadRequest(
                "Responses API 无法保留 Chat reasoning_content".into(),
            ));
        }
        let content = chat_content_to_responses(content, role)?;
        let has_content =
            !content.is_null() && !content.as_array().is_some_and(|parts| parts.is_empty());
        if has_content {
            input.push(serde_json::json!({"type":"message","role":role,"content":content}));
        }
        if let Some(tool_calls) = chat_tool_calls(message)? {
            if role != "assistant" && !tool_calls.is_empty() {
                return Err(AppError::BadRequest(
                    "tool_calls 只能出现在 assistant 消息中".into(),
                ));
            }
            for call in tool_calls {
                if call.get("extra_content").is_some() {
                    return Err(AppError::BadRequest(
                        "Responses API 无法保留 Gemini thought_signature".into(),
                    ));
                }
                input.push(serde_json::json!({
                    "type":"function_call",
                    "call_id":chat_required_string(call,"id")?,
                    "name":chat_required_string(&call["function"],"name")?,
                    "arguments":chat_function_input(call)?.to_string()
                }));
            }
        }
    }
    let tools = chat_tools(payload, "Responses")?
        .into_iter()
        .map(|tool| {
            let function = &tool["function"];
            let mut converted = serde_json::json!({
                "type":"function","name":function["name"],"parameters":function["parameters"]
            });
            if let Some(description) = function.get("description") {
                converted["description"] = description.clone();
            }
            if let Some(strict) = function.get("strict") {
                converted["strict"] = strict.clone();
            }
            converted
        })
        .collect::<Vec<_>>();
    let store = payload
        .get("store")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if payload
        .get("store")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::BadRequest("store 必须是布尔值".into()));
    }
    let mut response = serde_json::json!({
        "model":model,
        "instructions":instructions.join("\n\n"),
        "input":input,
        "stream":true,
        "store":store
    });
    if !tools.is_empty() {
        response["tools"] = Value::Array(tools.clone());
    }
    if let Some(choice) = payload.get("tool_choice") {
        let normalized = normalize_responses_tool_choice(choice)?;
        let needs_tool = normalized == "required"
            || normalized.get("type").and_then(Value::as_str) == Some("function");
        if needs_tool && tools.is_empty() {
            return Err(AppError::BadRequest(
                "tool_choice 要求调用工具，但没有提供 tools".into(),
            ));
        }
        if let Some(name) = normalized.get("name").and_then(Value::as_str) {
            if !tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            {
                return Err(AppError::BadRequest(
                    "tool_choice 指定的函数不在 tools 中".into(),
                ));
            }
        }
        response["tool_choice"] = normalized;
    }
    if let Some(parallel) = payload.get("parallel_tool_calls") {
        response["parallel_tool_calls"] = Value::Bool(
            parallel
                .as_bool()
                .ok_or_else(|| AppError::BadRequest("parallel_tool_calls 必须是布尔值".into()))?,
        );
    }
    let mut reasoning = payload
        .get("reasoning")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !reasoning.is_object() {
        return Err(AppError::BadRequest("reasoning 必须是对象".into()));
    }
    if let Some(effort) = payload.get("reasoning_effort") {
        let effort = effort
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::BadRequest("reasoning_effort 必须是非空字符串".into()))?;
        reasoning["effort"] = serde_json::json!(effort);
    }
    if reasoning.get("summary").is_none() {
        reasoning["summary"] = serde_json::json!("auto");
    }
    response["reasoning"] = reasoning;
    Ok(response)
}

fn chat_content_to_responses(mut content: Value, role: &str) -> AppResult<Value> {
    let replacement = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    if let Value::String(text) = content {
        return Ok(serde_json::json!([{"type":replacement,"text":text}]));
    }
    if let Some(parts) = content.as_array_mut() {
        for part in parts {
            if !part.is_object() {
                return Err(AppError::BadRequest("Chat content block 必须是对象".into()));
            }
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = chat_required_string(part, "text")?.to_owned();
                    if part.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "text"))
                    }) {
                        return Err(AppError::BadRequest(
                            "Responses 转换不支持 Chat 文本块扩展字段".into(),
                        ));
                    }
                    part["type"] = Value::String(replacement.into());
                    part["text"] = Value::String(text);
                }
                Some("image_url") if role == "user" => {
                    let image = part.get("image_url").cloned().unwrap_or(Value::Null);
                    let url = image
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| image.get("url").and_then(Value::as_str).map(str::to_owned))
                        .filter(|url| !url.trim().is_empty())
                        .ok_or_else(|| AppError::BadRequest("image_url 缺少有效 URL".into()))?;
                    let detail = image.get("detail").or_else(|| part.get("detail"));
                    if let Some(detail) = detail {
                        if !detail
                            .as_str()
                            .is_some_and(|value| matches!(value, "auto" | "low" | "high"))
                        {
                            return Err(AppError::BadRequest(
                                "image_url detail 仅支持 auto、low 或 high".into(),
                            ));
                        }
                        part["detail"] = detail.clone();
                    }
                    if part.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "type" | "image_url" | "detail"))
                    }) {
                        return Err(AppError::BadRequest(
                            "Responses 转换不支持 Chat 图片块扩展字段".into(),
                        ));
                    }
                    part["type"] = Value::String("input_image".into());
                    part["image_url"] = Value::String(url);
                }
                Some("image_url") => {
                    return Err(AppError::BadRequest(
                        "Responses assistant history 不支持 image_url 内容".into(),
                    ));
                }
                Some(kind) => {
                    return Err(AppError::BadRequest(format!(
                        "Responses 转换不支持 Chat 内容块 {kind}"
                    )));
                }
                None => return Err(AppError::BadRequest("Chat 内容块缺少 type".into())),
            }
        }
        return Ok(content);
    }
    if content.is_null() {
        Ok(content)
    } else {
        Err(AppError::BadRequest(
            "Chat content 必须是字符串、内容块数组或 null".into(),
        ))
    }
}

/// Normalize legacy Chat-shaped text blocks in a Responses request while leaving
/// already-native Responses content items untouched.
pub(crate) fn normalize_chat_content_for_responses(mut content: Value, role: &str) -> Value {
    let replacement = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    if let Value::String(text) = content {
        return serde_json::json!([{"type":replacement,"text":text}]);
    }
    if let Some(parts) = content.as_array_mut() {
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => part["type"] = Value::String(replacement.into()),
                Some("image_url") => {
                    let image = part.get("image_url").cloned().unwrap_or(Value::Null);
                    let url = image
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| image.get("url").and_then(Value::as_str).map(str::to_owned));
                    if let Some(url) = url {
                        part["type"] = Value::String("input_image".into());
                        part["image_url"] = Value::String(url);
                        if part.get("detail").is_none() {
                            if let Some(detail) = image.get("detail") {
                                part["detail"] = detail.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    content
}

fn chat_required_string<'a>(source: &'a Value, field: &str) -> AppResult<&'a str> {
    source
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest(format!("{field} 必须是非空字符串")))
}

fn reject_chat_fields(payload: &Value, supported: &[&str], target: &str) -> AppResult<()> {
    let object = payload
        .as_object()
        .ok_or_else(|| AppError::BadRequest("请求必须是 JSON 对象".into()))?;
    if let Some(field) = object
        .keys()
        .find(|field| !supported.contains(&field.as_str()))
    {
        return Err(AppError::BadRequest(format!(
            "Chat Completions → {target} 暂不支持参数 {field}；已拒绝转换以避免静默丢弃"
        )));
    }
    Ok(())
}

fn chat_text_content(content: &Value, role: &str) -> AppResult<String> {
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    let parts = content
        .as_array()
        .ok_or_else(|| AppError::BadRequest(format!("{role} 消息 content 必须是文本")))?;
    let mut text = Vec::with_capacity(parts.len());
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return Err(AppError::BadRequest(format!(
                "{role} 消息只支持文本 content"
            )));
        }
        text.push(chat_required_string(part, "text")?.to_owned());
    }
    Ok(text.join("\n"))
}

fn chat_tool_calls(message: &Value) -> AppResult<Option<&[Value]>> {
    match message.get("tool_calls") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(calls)) => {
            for call in calls {
                if !call.is_object() || call.get("type").and_then(Value::as_str) != Some("function")
                {
                    return Err(AppError::BadRequest(
                        "tool_calls 仅支持 function 调用".into(),
                    ));
                }
                if call.as_object().is_some_and(|object| {
                    object.keys().any(|key| {
                        !matches!(key.as_str(), "id" | "type" | "function" | "extra_content")
                    })
                }) {
                    return Err(AppError::BadRequest(
                        "tool_call 包含当前转换不支持的字段".into(),
                    ));
                }
                chat_required_string(call, "id")?;
                let function = call
                    .get("function")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| AppError::BadRequest("tool_call 缺少 function 对象".into()))?;
                if function.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "name" | "arguments"))
                }) {
                    return Err(AppError::BadRequest(
                        "tool_call function 包含当前转换不支持的字段".into(),
                    ));
                }
                chat_required_string(function, "name")?;
                chat_function_input(call)?;
                if let Some(extra) = call.get("extra_content") {
                    let signature = extra
                        .pointer("/google/thought_signature")
                        .and_then(Value::as_str)
                        .filter(|signature| !signature.is_empty());
                    if signature.is_none()
                        || extra
                            .as_object()
                            .is_none_or(|object| object.keys().any(|key| key != "google"))
                        || extra["google"].as_object().is_none_or(|object| {
                            object.keys().any(|key| key != "thought_signature")
                        })
                    {
                        return Err(AppError::BadRequest(
                            "tool_call 包含当前转换不支持的 extra_content".into(),
                        ));
                    }
                }
            }
            Ok(Some(calls))
        }
        Some(_) => Err(AppError::BadRequest("tool_calls 必须是数组".into())),
    }
}

fn chat_tools(payload: &Value, target: &str) -> AppResult<Vec<Value>> {
    let Some(tools) = payload.get("tools").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let tools = tools
        .as_array()
        .ok_or_else(|| AppError::BadRequest("tools 必须是数组".into()))?;
    tools.iter().map(|tool| {
        if !tool.is_object() || tool.get("type").and_then(Value::as_str) != Some("function") {
            return Err(AppError::BadRequest(format!("{target} 转换仅支持 function tools")));
        }
        if tool.as_object().is_some_and(|object| object.keys().any(|key| !matches!(key.as_str(), "type" | "function"))) {
            return Err(AppError::BadRequest(format!("{target} 转换不支持 tool 扩展字段")));
        }
        let function = tool.get("function").filter(|value| value.is_object())
            .ok_or_else(|| AppError::BadRequest("function tool 缺少 function 对象".into()))?;
        if function.as_object().is_some_and(|object| object.keys().any(|key| !matches!(key.as_str(), "name" | "description" | "parameters" | "strict"))) {
            return Err(AppError::BadRequest(format!("{target} 转换不支持 function 扩展字段")));
        }
        let name = chat_required_string(function, "name")?;
        let parameters = function.get("parameters").filter(|value| value.is_object())
            .ok_or_else(|| AppError::BadRequest("function tool 缺少 JSON Schema parameters 对象".into()))?;
        let mut converted = serde_json::json!({"type":"function","function":{"name":name,"parameters":parameters}});
        if let Some(description) = function.get("description") {
            if !description.is_string() { return Err(AppError::BadRequest("function description 必须是字符串".into())); }
            converted["function"]["description"] = description.clone();
        }
        if let Some(strict) = function.get("strict") {
            if !strict.is_boolean() { return Err(AppError::BadRequest("function strict 必须是布尔值".into())); }
            converted["function"]["strict"] = strict.clone();
        }
        Ok(converted)
    }).collect()
}

fn validate_chat_message_fields(message: &Value, role: &str) -> AppResult<()> {
    let allowed: &[&str] = match role {
        "assistant" => &["role", "content", "reasoning_content", "tool_calls"],
        "tool" => &["role", "content", "tool_call_id"],
        _ => &["role", "content"],
    };
    if message
        .as_object()
        .is_some_and(|object| object.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(AppError::BadRequest(format!(
            "Chat {role} 消息包含当前目标协议无法保留的字段"
        )));
    }
    if role != "assistant" && message.get("content").is_none() {
        return Err(AppError::BadRequest(format!(
            "Chat {role} 消息缺少 content"
        )));
    }
    if role != "assistant" && message.get("content").is_some_and(Value::is_null) {
        return Err(AppError::BadRequest(format!(
            "Chat {role} 消息 content 不能为空"
        )));
    }
    if role == "tool" {
        chat_required_string(message, "tool_call_id")?;
    }
    if role == "assistant" {
        if let Some(reasoning) = message.get("reasoning_content") {
            if reasoning.as_str().is_none_or(|text| text.is_empty()) {
                return Err(AppError::BadRequest(
                    "assistant reasoning_content 必须是非空字符串".into(),
                ));
            }
        }
        let has_calls = chat_tool_calls(message)?.is_some_and(|calls| !calls.is_empty());
        if message.get("content").is_none_or(Value::is_null) && !has_calls {
            return Err(AppError::BadRequest(
                "Chat assistant 消息必须包含 content 或 tool_calls".into(),
            ));
        }
    }
    Ok(())
}

fn normalize_responses_tool_choice(choice: &Value) -> AppResult<Value> {
    match choice {
        Value::String(mode) if matches!(mode.as_str(), "none" | "auto" | "required") => {
            Ok(choice.clone())
        }
        Value::Object(_) if choice.get("type").and_then(Value::as_str) == Some("function") => Ok(
            serde_json::json!({"type":"function","name":chat_required_string(&choice["function"], "name")?}),
        ),
        _ => Err(AppError::BadRequest(
            "tool_choice 仅支持 none、auto、required 或指定 function".into(),
        )),
    }
}

pub(crate) fn response_to_chat_completion(response: &Value, request: &Value) -> Value {
    let mut text = String::new();
    let mut refusal = String::new();
    let mut tool_calls = Vec::new();
    let mut public_summaries = Vec::new();
    for item in response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for content in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(value) = content.get("text").and_then(Value::as_str) {
                        text.push_str(value);
                    }
                    if content.get("type").and_then(Value::as_str) == Some("refusal") {
                        if let Some(value) = content.get("refusal").and_then(Value::as_str) {
                            refusal.push_str(value);
                        }
                    }
                }
            }
            Some("reasoning") => {
                for summary in item.get("summary").and_then(Value::as_array).into_iter().flatten() {
                    if summary.get("type").and_then(Value::as_str)==Some("summary_text") {
                        if let Some(text)=summary.get("text").and_then(Value::as_str).filter(|s|!s.is_empty()){public_summaries.push(text.to_owned());}
                    }
                }
            },
            Some("function_call") => tool_calls.push(serde_json::json!({
                "id":item.get("call_id").or_else(|| item.get("id")).expect("validated Responses call id"),
                "type":"function",
                "function":{
                    "name":item.get("name").expect("validated Responses function name"),
                    "arguments":item.get("arguments").expect("validated Responses function arguments")
                }
            })),
            _ => unreachable!("Responses output item is validated before conversion"),
        }
    }
    let has_tool_calls = !tool_calls.is_empty();
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !public_summaries.is_empty() {
        message["reasoning_content"] = Value::String(public_summaries.join("\n\n"));
    }
    if !refusal.is_empty() {
        message["refusal"] = Value::String(refusal.clone());
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish_reason =
        responses_finish_reason(response, None, has_tool_calls, !refusal.is_empty());
    serde_json::json!({
        "id":response.get("id").cloned().unwrap_or_else(|| Value::String(format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()))),
        "object":"chat.completion",
        "created":response.get("created_at").cloned().unwrap_or_else(|| Value::from(chrono::Utc::now().timestamp())),
        "model":request["model"],
        "choices":[{"index":0,"message":message,"finish_reason":finish_reason}],
        "usage":responses_chat_usage(response)
    })
}

pub(crate) fn validate_responses_result(response: &Value, request: &Value) -> AppResult<()> {
    if has_upstream_error(response) {
        return Err(AppError::BadGateway(response_error_message(response)));
    }
    chat_required_string(request, "model")
        .map_err(|_| AppError::BadRequest("model 必须是非空字符串".into()))?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadGateway("Responses 上游缺少 output 数组".into()))?;
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                let content = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        AppError::BadGateway("Responses message 缺少 content 数组".into())
                    })?;
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") if part.get("text").is_some_and(Value::is_string) => {
                            if part.get("annotations").is_some_and(|annotations| {
                                !annotations.as_array().is_some_and(Vec::is_empty)
                            }) {
                                return Err(AppError::BadGateway(
                                    "Responses 输出含有无法转成 Chat 的文本注释".into(),
                                ));
                            }
                        }
                        Some("refusal") if part.get("refusal").is_some_and(Value::is_string) => {}
                        _ => {
                            return Err(AppError::BadGateway(
                                "Responses message 含有当前 Chat 转换不支持的内容块".into(),
                            ));
                        }
                    }
                }
            }
            Some("reasoning") => {
                if let Some(summary) = item.get("summary") {
                    let summary = summary.as_array().ok_or_else(|| {
                        AppError::BadGateway("Responses reasoning.summary 格式无效".into())
                    })?;
                    if summary.iter().any(|entry| {
                        entry.get("type").and_then(Value::as_str) != Some("summary_text")
                            || !entry.get("text").is_some_and(Value::is_string)
                    }) {
                        return Err(AppError::BadGateway(
                            "Responses reasoning summary 含有无法转换的项目".into(),
                        ));
                    }
                }
            }
            Some("function_call") => {
                chat_required_string(item, "call_id")
                    .or_else(|_| chat_required_string(item, "id"))
                    .map_err(|_| {
                        AppError::BadGateway("Responses function_call 缺少 call_id".into())
                    })?;
                chat_required_string(item, "name").map_err(|_| {
                    AppError::BadGateway("Responses function_call 缺少 name".into())
                })?;
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AppError::BadGateway("Responses function_call.arguments 格式无效".into())
                    })?;
                if !serde_json::from_str::<Value>(arguments).is_ok_and(|value| value.is_object()) {
                    return Err(AppError::BadGateway(
                        "Responses function_call.arguments 不是 JSON 对象".into(),
                    ));
                }
            }
            _ => {
                return Err(AppError::BadGateway(
                    "Responses 上游返回了当前 Chat 转换不支持的 output 项目".into(),
                ));
            }
        }
    }
    validate_reported_counts(
        response.get("usage"),
        &["input_tokens", "output_tokens", "total_tokens"],
        "Responses",
    )
}

pub(crate) fn anthropic_messages_url(base_url: &str) -> AppResult<String> {
    let trimmed = base_url.trim_end_matches('/');
    let base = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    Ok(format!("{base}/v1/messages"))
}

pub(crate) fn gemini_generate_url(base_url: &str, model: &str, stream: bool) -> AppResult<String> {
    let trimmed = base_url.trim_end_matches('/');
    let base = trimmed
        .strip_suffix("/v1beta")
        .or_else(|| trimmed.strip_suffix("/v1"))
        .unwrap_or(trimmed);
    let model = model.trim();
    if model.is_empty()
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(AppError::BadRequest("Gemini 模型名无效".into()));
    }
    let method = if stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };
    Ok(format!("{base}/v1beta/models/{model}:{method}"))
}

fn anthropic_content_blocks(content: &Value) -> AppResult<Vec<Value>> {
    match content {
        Value::String(text) => Ok(if text.is_empty() { Vec::new() } else { vec![serde_json::json!({"type":"text","text":text})] }),
        Value::Array(parts) => parts.iter().map(|part| {
            if !part.is_object() { return Err(AppError::BadRequest("Chat content block 必须是对象".into())); }
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if part.as_object().is_some_and(|object| object.keys().any(|key| !matches!(key.as_str(),"type"|"text"))) {
                        return Err(AppError::BadRequest("Anthropic 转换不支持 Chat 文本块扩展字段".into()));
                    }
                    Ok(serde_json::json!({"type":"text","text":chat_required_string(part,"text")?}))
                }
                Some("image_url") => {
                    if part.get("detail").is_some() || part.pointer("/image_url/detail").is_some() {
                        return Err(AppError::BadRequest("Anthropic 转换无法保留 image_url detail，请移除此字段或使用 Anthropic 原生请求".into()));
                    }
                    if part.as_object().is_some_and(|object| object.keys().any(|key| !matches!(key.as_str(),"type"|"image_url"))) {
                        return Err(AppError::BadRequest("Anthropic 转换不支持 Chat 图片块扩展字段".into()));
                    }
                    let image = &part["image_url"];
                    let url = image.as_str().or_else(|| image.get("url").and_then(Value::as_str))
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| AppError::BadRequest("image_url 缺少有效 URL".into()))?;
                    if let Some(data_url) = url.strip_prefix("data:") {
                        let (metadata, data) = data_url.split_once(',').ok_or_else(|| AppError::BadRequest("图片 data URL 格式无效".into()))?;
                        let (media_type, encoding) = metadata.split_once(';').ok_or_else(|| AppError::BadRequest("图片 data URL 缺少 MIME 或 base64 标记".into()))?;
                        if media_type.trim().is_empty() || encoding != "base64" || data.trim().is_empty() {
                            return Err(AppError::BadRequest("图片 data URL 必须包含 MIME 类型及非空 Base64 数据".into()));
                        }
                        Ok(serde_json::json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}))
                    } else if url.starts_with("https://") || url.starts_with("http://") {
                        Ok(serde_json::json!({"type":"image","source":{"type":"url","url":url}}))
                    } else {
                        Err(AppError::BadRequest("图片仅支持 HTTP(S) URL 或 Base64 data URL".into()))
                    }
                }
                Some(kind) => Err(AppError::BadRequest(format!("Anthropic 转换不支持 Chat 内容块 {kind}"))),
                None => Err(AppError::BadRequest("Chat content block 缺少 type".into())),
            }
        }).collect(),
        Value::Null => Ok(Vec::new()),
        _ => Err(AppError::BadRequest("Chat content 必须是字符串、内容块数组或 null".into())),
    }
}

fn append_protocol_message(messages: &mut Vec<Value>, role: &str, block: Value) {
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.push(block);
                return;
            }
        }
    }
    messages.push(serde_json::json!({"role":role,"content":[block]}));
}

pub(crate) fn chat_to_anthropic(payload: &Value) -> AppResult<Value> {
    reject_chat_fields(
        payload,
        &[
            "model",
            "messages",
            "stream",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stop",
            "stop_sequences",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
        ],
        "Anthropic Messages",
    )?;
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let model = chat_required_string(payload, "model")?;
    let max_tokens = match (payload.get("max_completion_tokens"), payload.get("max_tokens")) {
        (Some(primary), Some(legacy)) if primary != legacy => return Err(AppError::BadRequest("max_tokens 与 max_completion_tokens 冲突，请只发送一个".into())),
        (Some(value), _) | (_, Some(value)) => value.as_u64().filter(|value| *value > 0)
            .ok_or_else(|| AppError::BadRequest("max_tokens 必须是正整数".into()))?,
        (None, None) => return Err(AppError::BadRequest("Anthropic Messages 需要 max_tokens；请在 Chat 请求中提供 max_tokens 或 max_completion_tokens".into())),
    };
    for field in ["temperature", "top_p"] {
        if payload.get(field).is_some_and(|value| !value.is_number()) {
            return Err(AppError::BadRequest(format!("{field} 必须是数字")));
        }
    }
    if payload
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::BadRequest("stream 必须是布尔值".into()));
    }
    let stop = match (payload.get("stop"), payload.get("stop_sequences")) {
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest(
                "stop 与 stop_sequences 不能同时指定".into(),
            ));
        }
        (Some(Value::String(text)), None) => Some(serde_json::json!([text])),
        (Some(Value::Array(items)), None) | (None, Some(Value::Array(items)))
            if items.iter().all(Value::is_string) =>
        {
            Some(Value::Array(items.clone()))
        }
        (None, None) => None,
        _ => return Err(AppError::BadRequest("stop 必须是字符串或字符串数组".into())),
    };
    let tools = chat_tools(payload, "Anthropic Messages")?;
    let tool_choice = payload.get("tool_choice");
    let choice_mode = match tool_choice {
        None => "auto",
        Some(Value::String(mode)) if matches!(mode.as_str(), "auto" | "required" | "none") => {
            mode.as_str()
        }
        Some(Value::Object(choice))
            if choice.get("type").and_then(Value::as_str) == Some("function") =>
        {
            "function"
        }
        Some(_) => {
            return Err(AppError::BadRequest(
                "Anthropic 转换的 tool_choice 仅支持 auto、required、none 或指定 function".into(),
            ));
        }
    };
    if choice_mode == "function" && tools.is_empty() {
        return Err(AppError::BadRequest(
            "指定 tool_choice.function 时必须提供 tools".into(),
        ));
    }
    if choice_mode == "required" && tools.is_empty() {
        return Err(AppError::BadRequest(
            "tool_choice=required 时必须提供 tools".into(),
        ));
    }
    if choice_mode == "function" {
        let name = chat_required_string(
            &tool_choice.expect("function choice is present")["function"],
            "name",
        )?;
        if !tools
            .iter()
            .any(|tool| tool.pointer("/function/name").and_then(Value::as_str) == Some(name))
        {
            return Err(AppError::BadRequest(
                "tool_choice 指定的函数不在 tools 中".into(),
            ));
        }
    }
    if tools
        .iter()
        .any(|tool| tool.pointer("/function/strict").and_then(Value::as_bool) == Some(true))
    {
        return Err(AppError::BadRequest(
            "Anthropic 转换不能保证 Chat strict 工具约束，请移除 strict 或使用 Anthropic 原生请求"
                .into(),
        ));
    }
    if let Some(parallel) = payload.get("parallel_tool_calls") {
        if !parallel.is_boolean() {
            return Err(AppError::BadRequest(
                "parallel_tool_calls 必须是布尔值".into(),
            ));
        }
    }
    let disable_parallel =
        payload.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false);
    let mut system = Vec::new();
    let mut converted: Vec<Value> = Vec::new();
    for message in messages {
        if !message.is_object() {
            return Err(AppError::BadRequest("messages 中的项目必须是对象".into()));
        }
        let role = chat_required_string(message, "role")?;
        if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
            return Err(AppError::BadRequest(format!("不支持的 Chat role: {role}")));
        }
        validate_chat_message_fields(message, role)?;
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "system" | "developer" => {
                system.push(chat_text_content(&content, "system/developer")?);
            }
            "tool" => {
                append_protocol_message(
                    &mut converted,
                    "user",
                    serde_json::json!({
                        "type":"tool_result",
                        "tool_use_id":chat_required_string(message,"tool_call_id")?,
                        "content":chat_text_content(&content,"tool")?
                    }),
                );
            }
            "assistant" => {
                if message.get("reasoning_content").is_some() {
                    return Err(AppError::BadRequest(
                        "Anthropic Messages 无法保留 Chat reasoning_content".into(),
                    ));
                }
                for block in anthropic_content_blocks(&content)? {
                    append_protocol_message(&mut converted, "assistant", block);
                }
                if let Some(calls) = chat_tool_calls(message)? {
                    for call in calls {
                        if call.get("extra_content").is_some() {
                            return Err(AppError::BadRequest(
                                "Anthropic Messages 无法保留 Gemini thought_signature".into(),
                            ));
                        }
                        append_protocol_message(
                            &mut converted,
                            "assistant",
                            serde_json::json!({
                                "type":"tool_use",
                                "id":chat_required_string(call,"id")?,
                                "name":chat_required_string(&call["function"],"name")?,
                                "input":chat_function_input(call)?
                            }),
                        );
                    }
                }
            }
            "user" => {
                for block in anthropic_content_blocks(&content)? {
                    append_protocol_message(&mut converted, "user", block);
                }
                if chat_tool_calls(message)?.is_some_and(|calls| !calls.is_empty()) {
                    return Err(AppError::BadRequest(
                        "tool_calls 只能出现在 assistant 消息中".into(),
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    if converted.is_empty() {
        return Err(AppError::BadRequest("messages 不能为空".into()));
    }
    let mut body = serde_json::json!({"model":model,"max_tokens":max_tokens,"messages":converted});
    if !system.is_empty() {
        body["system"] = Value::String(system.join("\n\n"));
    }
    if let Some(stream) = payload.get("stream") {
        body["stream"] = stream.clone();
    }
    for field in ["temperature", "top_p"] {
        if let Some(value) = payload.get(field) {
            body[field] = value.clone();
        }
    }
    if let Some(stop) = stop {
        body["stop_sequences"] = stop;
    }
    if choice_mode != "none" && !tools.is_empty() {
        body["tools"] = Value::Array(tools.into_iter().map(|tool| {
            let function = &tool["function"];
            let mut converted = serde_json::json!({"name":function["name"],"input_schema":function["parameters"]});
            if let Some(description) = function.get("description") { converted["description"] = description.clone(); }
            converted
        }).collect());
        body["tool_choice"] = match tool_choice {
            Some(Value::Object(choice)) => {
                serde_json::json!({"type":"tool","name":chat_required_string(&choice["function"],"name")?})
            }
            Some(Value::String(mode)) if mode == "required" => serde_json::json!({"type":"any"}),
            _ => serde_json::json!({"type":"auto"}),
        };
        if disable_parallel {
            body["tool_choice"]["disable_parallel_tool_use"] = Value::Bool(true);
        }
    }
    Ok(body)
}

pub(crate) fn anthropic_finish_reason(stop_reason: Option<&str>) -> &'static str {
    match stop_reason {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

pub(crate) fn anthropic_to_chat_completion(body: &Value, request: &Value) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            Some("tool_use") => tool_calls.push(serde_json::json!({
                "id":block["id"],
                "type":"function",
                "function":{
                    "name":block["name"],
                    "arguments":block["input"].to_string()
                }
            })),
            _ => unreachable!("Anthropic response is validated before conversion"),
        }
    }
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish = anthropic_finish_reason(
        body.get("stop_reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    );
    serde_json::json!({
        "id":body.get("id").cloned().unwrap_or_else(|| Value::String(format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()))),
        "object":"chat.completion",
        "created":chrono::Utc::now().timestamp(),
        "model":request["model"],
        "choices":[{"index":0,"message":message,"finish_reason":if message.get("tool_calls").is_some() && finish == "stop" {"tool_calls"} else {finish}}],
        "usage":anthropic_chat_usage(body.get("usage").unwrap_or(&Value::Null))
    })
}

fn chat_function_input(call: &Value) -> AppResult<Value> {
    let source = call
        .pointer("/function/arguments")
        .ok_or_else(|| AppError::BadRequest("工具调用缺少 arguments".into()))?;
    let arguments = match source {
        Value::String(arguments) => serde_json::from_str::<Value>(arguments)
            .map_err(|_| AppError::BadRequest("工具调用参数不是有效的 JSON 对象".into()))?,
        value => value.clone(),
    };
    if !arguments.is_object() {
        return Err(AppError::BadRequest("工具调用参数必须是 JSON 对象".into()));
    }
    Ok(arguments)
}

pub(crate) fn chat_to_gemini(payload: &Value) -> AppResult<(Value, bool)> {
    reject_chat_fields(
        payload,
        &[
            "model",
            "messages",
            "stream",
            "temperature",
            "top_p",
            "max_tokens",
            "max_completion_tokens",
            "stop",
            "tools",
        ],
        "Gemini",
    )?;
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let model = chat_required_string(payload, "model")?;
    if payload
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::BadRequest("stream 必须是布尔值".into()));
    }
    for field in ["temperature", "top_p"] {
        if payload.get(field).is_some_and(|value| !value.is_number()) {
            return Err(AppError::BadRequest(format!("{field} 必须是数字")));
        }
    }
    let max_tokens = match (
        payload.get("max_completion_tokens"),
        payload.get("max_tokens"),
    ) {
        (Some(primary), Some(legacy)) if primary != legacy => {
            return Err(AppError::BadRequest(
                "max_tokens 与 max_completion_tokens 冲突，请只发送一个".into(),
            ));
        }
        (Some(value), _) | (_, Some(value)) => Some(
            value
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or_else(|| AppError::BadRequest("max_tokens 必须是正整数".into()))?,
        ),
        (None, None) => None,
    };
    let stop = match payload.get("stop") {
        Some(Value::String(text)) => Some(serde_json::json!([text])),
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            Some(Value::Array(items.clone()))
        }
        Some(_) => return Err(AppError::BadRequest("stop 必须是字符串或字符串数组".into())),
        None => None,
    };
    let tools = chat_tools(payload, "Gemini")?;
    if tools
        .iter()
        .any(|tool| tool.pointer("/function/strict").and_then(Value::as_bool) == Some(true))
    {
        return Err(AppError::BadRequest(
            "Gemini 转换不能保证 Chat strict 工具约束，请移除 strict 或使用 Gemini 原生请求".into(),
        ));
    }
    let mut system = Vec::new();
    let mut tool_names = std::collections::HashMap::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut append = |role: &str, part: Value| {
        if let Some(last) = contents.last_mut() {
            if last.get("role").and_then(Value::as_str) == Some(role) {
                if let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
                    parts.push(part);
                    return;
                }
            }
        }
        contents.push(serde_json::json!({"role":role,"parts":[part]}));
    };
    for message in messages {
        if !message.is_object() {
            return Err(AppError::BadRequest("messages 中的项目必须是对象".into()));
        }
        let role = chat_required_string(message, "role")?;
        if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
            return Err(AppError::BadRequest(format!("不支持的 Chat role: {role}")));
        }
        validate_chat_message_fields(message, role)?;
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "system" | "developer" => system.push(chat_text_content(&content, "system/developer")?),
            "tool" => {
                let output = chat_text_content(&content, "tool")?;
                let call_id = chat_required_string(message, "tool_call_id")?;
                let name = tool_names.get(call_id).ok_or_else(|| {
                    AppError::BadRequest(
                        "工具结果找不到对应的调用，请保留此前 assistant.tool_calls".into(),
                    )
                })?;
                append(
                    "user",
                    serde_json::json!({"functionResponse":{"id":call_id,"name":name,"response":{"result":output}}}),
                );
            }
            "assistant" => {
                if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
                    append(
                        "model",
                        serde_json::json!({"thought":true,"text":reasoning}),
                    );
                }
                if !content.is_null() {
                    for part in chat_text_parts(&content, "assistant")? {
                        append("model", part);
                    }
                }
                if let Some(calls) = chat_tool_calls(message)? {
                    for call in calls {
                        let call_id = chat_required_string(call, "id")?;
                        let name = chat_required_string(&call["function"], "name")?;
                        if tool_names
                            .insert(call_id.to_owned(), name.to_owned())
                            .is_some()
                        {
                            return Err(AppError::BadRequest("tool_calls 中的 id 重复".into()));
                        }
                        let mut part = serde_json::json!({"functionCall":{"id":call_id,"name":name,"args":chat_function_input(call)?}});
                        if let Some(signature) = call
                            .pointer("/extra_content/google/thought_signature")
                            .and_then(Value::as_str)
                        {
                            part["thoughtSignature"] = Value::String(signature.to_owned());
                        }
                        append("model", part);
                    }
                }
            }
            "user" => {
                for part in chat_gemini_user_parts(&content)? {
                    append("user", part);
                }
                if chat_tool_calls(message)?.is_some_and(|calls| !calls.is_empty()) {
                    return Err(AppError::BadRequest(
                        "tool_calls 只能出现在 assistant 消息中".into(),
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    if contents.is_empty() {
        return Err(AppError::BadRequest("messages 不能为空".into()));
    }
    let stream = payload.get("stream").and_then(Value::as_bool) == Some(true);
    let mut body = serde_json::json!({"model":model,"contents":contents});
    if !system.is_empty() {
        body["systemInstruction"] = serde_json::json!({"parts":[{"text":system.join("\n\n")}]});
    }
    let mut generation = serde_json::Map::new();
    if let Some(value) = payload.get("temperature") {
        generation.insert("temperature".into(), value.clone());
    }
    if let Some(value) = payload.get("top_p") {
        generation.insert("topP".into(), value.clone());
    }
    if let Some(value) = max_tokens {
        generation.insert("maxOutputTokens".into(), Value::from(value));
    }
    if let Some(stop) = stop {
        generation.insert("stopSequences".into(), stop);
    }
    if !generation.is_empty() {
        body["generationConfig"] = Value::Object(generation);
    }
    if !tools.is_empty() {
        let declarations = tools.into_iter().map(|tool| {
            let function = &tool["function"];
            let mut declaration = serde_json::json!({"name":function["name"],"parameters":function["parameters"]});
            if let Some(description) = function.get("description") { declaration["description"] = description.clone(); }
            declaration
        }).collect::<Vec<_>>();
        body["tools"] = serde_json::json!([{"functionDeclarations":declarations}]);
    }
    Ok((body, stream))
}

fn chat_text_parts(content: &Value, role: &str) -> AppResult<Vec<Value>> {
    if let Some(text) = content.as_str() {
        return Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![serde_json::json!({"text":text})]
        });
    }
    let parts = content
        .as_array()
        .ok_or_else(|| AppError::BadRequest(format!("{role} content 仅支持文本块")))?;
    parts
        .iter()
        .map(|part| {
            if part.get("type").and_then(Value::as_str) != Some("text") {
                return Err(AppError::BadRequest(format!("{role} content 仅支持文本块")));
            }
            if part.as_object().is_some_and(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "type" | "text"))
            }) {
                return Err(AppError::BadRequest(
                    "Gemini 转换不支持 Chat 文本块扩展字段".into(),
                ));
            }
            Ok(serde_json::json!({"text":chat_required_string(part,"text")?}))
        })
        .collect()
}

fn chat_gemini_user_parts(content: &Value) -> AppResult<Vec<Value>> {
    if let Some(text) = content.as_str() {
        return Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![serde_json::json!({"text":text})]
        });
    }
    let parts = content.as_array().ok_or_else(|| {
        AppError::BadRequest("Gemini user content 必须是字符串或文本/图片块数组".into())
    })?;
    parts
        .iter()
        .map(|part| match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if part.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "text"))
                }) {
                    return Err(AppError::BadRequest(
                        "Gemini 转换不支持 Chat 文本块扩展字段".into(),
                    ));
                }
                Ok(serde_json::json!({"text":chat_required_string(part,"text")?}))
            }
            Some("image_url") => {
                if part.get("detail").is_some() || part.pointer("/image_url/detail").is_some() {
                    return Err(AppError::BadRequest(
                        "Gemini 转换无法保留 image_url detail，请移除此字段或使用 Gemini 原生请求"
                            .into(),
                    ));
                }
                if part.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "image_url"))
                }) {
                    return Err(AppError::BadRequest(
                        "Gemini 转换不支持 Chat 图片块扩展字段".into(),
                    ));
                }
                let image = &part["image_url"];
                let url = image
                    .as_str()
                    .or_else(|| image.get("url").and_then(Value::as_str))
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| AppError::BadRequest("image_url 缺少有效 URL".into()))?;
                let data_url = url.strip_prefix("data:").ok_or_else(|| {
                    AppError::BadRequest("Gemini 仅支持 Base64 data URL 图片".into())
                })?;
                let (metadata, data) = data_url
                    .split_once(',')
                    .ok_or_else(|| AppError::BadRequest("图片 data URL 格式无效".into()))?;
                let (mime, encoding) = metadata.split_once(';').ok_or_else(|| {
                    AppError::BadRequest("图片 data URL 缺少 MIME 或 base64 标记".into())
                })?;
                if mime.trim().is_empty() || encoding != "base64" || data.trim().is_empty() {
                    return Err(AppError::BadRequest(
                        "图片 data URL 必须包含 MIME 类型及非空 Base64 数据".into(),
                    ));
                }
                Ok(serde_json::json!({"inlineData":{"mimeType":mime,"data":data}}))
            }
            Some(kind) => Err(AppError::BadRequest(format!(
                "Gemini 转换不支持 Chat 内容块 {kind}"
            ))),
            None => Err(AppError::BadRequest("Chat content block 缺少 type".into())),
        })
        .collect()
}

pub(crate) fn gemini_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("MAX_TOKENS") => "length",
        Some("SAFETY")
        | Some("RECITATION")
        | Some("BLOCKLIST")
        | Some("PROHIBITED_CONTENT")
        | Some("SPII")
        | Some("IMAGE_SAFETY") => "content_filter",
        _ => "stop",
    }
}

pub(crate) fn gemini_to_chat_completion(body: &Value, request: &Value) -> Value {
    let candidate = body
        .pointer("/candidates/0")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for part in candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                reasoning.push_str(value);
            } else {
                text.push_str(value);
            }
        }
        if part.get("functionCall").is_some() {
            tool_calls.push(gemini_tool_call(part));
        }
    }
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !reasoning.is_empty() {
        message["reasoning_content"] = Value::String(reasoning);
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish = if gemini_prompt_blocked(body) {
        "content_filter"
    } else {
        gemini_finish_reason(
            candidate
                .get("finishReason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        )
    };
    serde_json::json!({
        "id":format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),
        "object":"chat.completion",
        "created":chrono::Utc::now().timestamp(),
        "model":request["model"],
        "choices":[{"index":0,"message":message,"finish_reason":if message.get("tool_calls").is_some() && finish == "stop" {"tool_calls"} else {finish}}],
        "usage":gemini_chat_usage(&body["usageMetadata"])
    })
}

pub(crate) fn gemini_tool_call(part: &Value) -> Value {
    let call = &part["functionCall"];
    let mut converted = serde_json::json!({
        "id":call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())
            .map(str::to_owned).unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple())),
        "type":"function",
        "function":{"name":call["name"],"arguments":call.get("args").expect("validated Gemini functionCall args").to_string()}
    });
    if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
        converted["extra_content"] = serde_json::json!({"google":{"thought_signature":signature}});
    }
    converted
}

pub(crate) fn gemini_chat_usage(source: &Value) -> Value {
    let prompt = token_count(source, "promptTokenCount");
    let tool_prompt = token_count(source, "toolUsePromptTokenCount");
    let reasoning = token_count(source, "thoughtsTokenCount");
    let candidates = token_count(source, "candidatesTokenCount");
    let input = prompt.map(|count| count.saturating_add(tool_prompt.unwrap_or(0)));
    // Gemini's total includes prompt, candidate, and thought tokens. Some
    // compatible Gemini endpoints only return the total, so derive output
    // from that documented total when the explicit candidate count is absent.
    let total = token_count(source, "totalTokenCount");
    let output = candidates
        .map(|count| count.saturating_add(reasoning.unwrap_or(0)))
        .or_else(|| {
            total
                .zip(input)
                .map(|(total, input)| total.saturating_sub(input))
        });
    let mut usage = chat_usage_counts(input, output);
    if usage.is_null() && total.is_some() {
        usage = serde_json::json!({});
    }
    if let Some(total) = total {
        usage["total_tokens"] = Value::from(total);
    }
    if !usage.is_null() {
        if let Some(cached) = token_count(source, "cachedContentTokenCount") {
            usage["prompt_tokens_details"]["cached_tokens"] = Value::from(cached);
        }
        if let Some(tool_prompt) = tool_prompt {
            usage["prompt_tokens_details"]["tool_use_prompt_tokens"] = Value::from(tool_prompt);
        }
        if let Some(reasoning) = reasoning {
            usage["completion_tokens_details"]["reasoning_tokens"] = Value::from(reasoning);
        }
    }
    usage
}

pub(crate) fn chat_chunk(
    id: &str,
    model: &str,
    created: i64,
    delta: Value,
    finish_reason: Value,
    usage: Value,
) -> Value {
    serde_json::json!({
        "id":id,
        "object":"chat.completion.chunk",
        "created":created,
        "model":model,
        "choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}],
        "usage":usage
    })
}

#[cfg(test)]
mod tests;
