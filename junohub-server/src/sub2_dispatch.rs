//! Upstream request construction. This module never dispatches requests or
//! changes model IDs; scheduling, retries and response delivery belong to the gateway.
use crate::{
    error::{AppError, AppResult},
    model_api::responses_suffix,
    sub2_protocol::{
        anthropic_messages_url, chat_to_anthropic, chat_to_gemini, chat_to_responses,
        gemini_generate_url, EngineProtocol,
    },
    sub2_repository::EngineAccount,
};
use axum::http::{HeaderMap, Method};
use serde_json::Value;

pub(crate) fn engine_protocol(account: &EngineAccount) -> AppResult<EngineProtocol> {
    match (account.kind.as_str(), account.platform.as_str()) {
        ("oauth", "openai") => Ok(EngineProtocol::OpenAiCodex),
        ("oauth", "anthropic") => Ok(EngineProtocol::ClaudeOAuth),
        ("oauth", "gemini") => Ok(EngineProtocol::GeminiOAuth),
        ("oauth", "grok") => Ok(EngineProtocol::GrokOAuth),
        ("oauth", "antigravity") => Ok(EngineProtocol::Antigravity),
        ("apikey", "anthropic") => Ok(EngineProtocol::Anthropic),
        ("apikey", "gemini") => Ok(EngineProtocol::Gemini),
        ("apikey", _) => Ok(EngineProtocol::OpenAiCompatible),
        (kind, platform) => Err(AppError::BadRequest(format!(
            "不支持的账户类型: {platform}/{kind}"
        ))),
    }
}

enum Auth {
    Bearer(String),
    ApiKey(String),
    Google(String),
    ClaudeBearer(String),
}

pub(crate) fn build_request(
    client: &reqwest::Client,
    account: &EngineAccount,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    payload: &Value,
) -> AppResult<reqwest::RequestBuilder> {
    let protocol = engine_protocol(account)?;
    let client_chat = matches!(path, "/v1/chat/completions" | "/chat/completions");
    let (url, mut outbound_payload, auth) = match protocol {
        EngineProtocol::OpenAiCompatible => {
            let base_url = credential(account, "base_url")?;
            let upstream_path = path.strip_prefix("/v1").unwrap_or(path);
            (
                format!("{}{}", base_url.trim_end_matches('/'), upstream_path),
                payload.clone(),
                Auth::Bearer(credential(account, "api_key")?),
            )
        }
        EngineProtocol::Anthropic => {
            if !client_chat {
                return Err(AppError::BadRequest(
                    "Claude API Key 账户仅支持 Chat Completions 协议".into(),
                ));
            }
            (
                anthropic_messages_url(&credential(account, "base_url")?)?,
                chat_to_anthropic(payload)?,
                Auth::ApiKey(credential(account, "api_key")?),
            )
        }
        EngineProtocol::ClaudeOAuth => {
            if !client_chat {
                return Err(AppError::BadRequest(
                    "Claude OAuth 账户仅支持 Chat Completions 协议".into(),
                ));
            }
            (
                "https://api.anthropic.com/v1/messages".into(),
                chat_to_anthropic(payload)?,
                Auth::ClaudeBearer(credential(account, "access_token")?),
            )
        }
        EngineProtocol::Gemini => {
            if !client_chat {
                return Err(AppError::BadRequest(
                    "Gemini API Key 账户仅支持 Chat Completions 协议".into(),
                ));
            }
            let (body, stream) = chat_to_gemini(payload)?;
            let model = body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (
                gemini_generate_url(&credential(account, "base_url")?, &model, stream)?,
                body,
                Auth::Google(credential(account, "api_key")?),
            )
        }
        EngineProtocol::GeminiOAuth | EngineProtocol::Antigravity => {
            if !client_chat {
                return Err(AppError::BadRequest(
                    "Google OAuth 账户仅支持 Chat Completions 协议".into(),
                ));
            }
            let (body, stream) = chat_to_gemini(payload)?;
            let model = body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let project = account
                .credentials
                .get("project_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if protocol == EngineProtocol::Antigravity && project.is_empty() {
                return Err(AppError::BadRequest(
                    "Antigravity 账户缺少 project_id，请重新绑定并填写 Google Cloud Project ID"
                        .into(),
                ));
            }
            if project.is_empty() {
                let url = gemini_generate_url(
                    "https://generativelanguage.googleapis.com",
                    &model,
                    stream,
                )?;
                (
                    url,
                    body,
                    Auth::Bearer(credential(account, "access_token")?),
                )
            } else {
                let action = if stream {
                    "streamGenerateContent?alt=sse"
                } else {
                    "generateContent"
                };
                let base = if protocol == EngineProtocol::Antigravity {
                    "https://daily-cloudcode-pa.googleapis.com"
                } else {
                    "https://cloudcode-pa.googleapis.com"
                };
                let outbound = serde_json::json!({
                    "project":project,
                    "requestId":format!("agent-{}", uuid::Uuid::new_v4()),
                    "userAgent":if protocol == EngineProtocol::Antigravity { "antigravity" } else { "gemini-cli" },
                    "requestType":"agent",
                    "model":model,
                    "request":body
                });
                (
                    format!("{base}/v1internal:{action}"),
                    outbound,
                    Auth::Bearer(credential(account, "access_token")?),
                )
            }
        }
        EngineProtocol::GrokOAuth => {
            let base_url = account
                .credentials
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("https://cli-chat-proxy.grok.com/v1");
            let response_path = responses_suffix(path).map(|suffix| format!("/responses{suffix}"));
            let upstream_path = if client_chat {
                "/chat/completions".to_owned()
            } else if let Some(response_path) = response_path {
                response_path
            } else {
                path.strip_prefix("/v1").unwrap_or(path).to_owned()
            };
            (
                format!("{}{}", base_url.trim_end_matches('/'), upstream_path),
                payload.clone(),
                Auth::Bearer(credential(account, "access_token")?),
            )
        }
        EngineProtocol::OpenAiCodex => {
            let suffix = responses_suffix(path);
            if !client_chat && suffix.is_none() {
                return Err(AppError::BadRequest(
                    "OpenAI OAuth 账户当前仅支持 Responses 和 Chat Completions 协议".into(),
                ));
            }
            let endpoint = format!(
                "https://chatgpt.com/backend-api/codex/responses{}",
                suffix.unwrap_or_default()
            );
            let outbound = if client_chat {
                chat_to_responses(payload)?
            } else {
                payload.clone()
            };
            (
                endpoint,
                outbound,
                Auth::Bearer(credential(account, "access_token")?),
            )
        }
    };
    let codex_oauth = protocol == EngineProtocol::OpenAiCodex;
    if codex_oauth && responses_suffix(path).is_none_or(|suffix| suffix.is_empty()) {
        outbound_payload["stream"] = Value::Bool(true);
        outbound_payload["store"] = Value::Bool(false);
    }
    let mut request = client.request(method, url).json(&outbound_payload);
    request = match auth {
        Auth::Bearer(token) => request.bearer_auth(token),
        Auth::ApiKey(key) => request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        Auth::Google(key) => request.header("x-goog-api-key", key),
        Auth::ClaudeBearer(token) => request
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header(
                "anthropic-beta",
                "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,effort-2025-11-24,context-management-2025-06-27,extended-cache-ttl-2025-04-11",
            )
            .header("user-agent", "claude-cli/2.1.220 (external, cli)")
            .header("x-stainless-lang", "js")
            .header("x-stainless-package-version", "0.94.0")
            .header("x-stainless-os", "Linux")
            .header("x-stainless-arch", "arm64")
            .header("x-stainless-runtime", "node")
            .header("x-stainless-runtime-version", "v24.3.0")
            .header("x-stainless-retry-count", "0")
            .header("x-stainless-timeout", "600")
            .header("x-app", "cli")
            .header("anthropic-dangerous-direct-browser-access", "true")
            .header("accept", "application/json")
            .header("x-client-request-id", uuid::Uuid::new_v4().to_string()),
    };
    for (name, value) in headers {
        // 账户引擎自行构造上游请求头，仅透传少量无副作用的客户端标识头，
        // 避免 user-agent / accept / cookie 等头与协议头冲突。
        if !matches!(name.as_str(), "x-request-id" | "x-correlation-id") {
            continue;
        }
        request = request.header(name, value);
    }
    if codex_oauth {
        if let Some(account_id) = account
            .credentials
            .get("chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header("chatgpt-account-id", account_id);
        }
        request = request
            .header(
                "accept",
                if responses_suffix(path).is_some_and(|suffix| !suffix.is_empty()) {
                    "application/json"
                } else {
                    "text/event-stream"
                },
            )
            .header("originator", "codex_cli_rs")
            .header("user-agent", "codex_cli_rs/0.200.1")
            .header("version", "0.200.1");
    }
    if protocol == EngineProtocol::GeminiOAuth {
        request = request.header("user-agent", "GeminiCLI/0.1.5 (Windows; AMD64)");
    }
    if protocol == EngineProtocol::Antigravity {
        request = request.header("user-agent", "antigravity/1.23.2 windows/amd64");
    }
    if protocol == EngineProtocol::GrokOAuth {
        request = request
            .header("user-agent", "xai-grok-workspace/0.2.120")
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("x-grok-client-version", "0.2.120")
            .header("x-grok-client-identifier", "grok-shell")
            .header("x-grok-client-mode", "cli");
    }
    Ok(request)
}

pub(crate) fn credential(account: &EngineAccount, key: &str) -> AppResult<String> {
    account
        .credentials
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::Internal(format!("账户缺少凭证字段 {key}")))
}
