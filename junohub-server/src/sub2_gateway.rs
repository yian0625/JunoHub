use std::{pin::Pin, time::Duration};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, Response},
};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::Value;

use crate::model_api::responses_suffix;
use crate::sub2_dispatch::{build_request, credential, engine_protocol};

use crate::sub2_protocol::{
    anthropic_chat_usage, anthropic_finish_reason, chat_chunk, gemini_chat_usage,
    gemini_finish_reason, gemini_prompt_blocked, gemini_tool_call, has_upstream_error,
    is_anthropic, is_gemini, native_response_to_chat_completion, protocol_error_to_openai,
    response_error_message, response_to_chat_completion, EngineProtocol,
};

use crate::{
    config::Config,
    error::{AppError, AppResult},
    state::AppState,
    sub2_repository::{EngineAccount, EngineKeyRoute, Sub2Repository},
    sub2_scheduler::{SchedulerBucket, SchedulerCache, SelectionRequest},
    sub2_sticky::StickyStore,
};

#[derive(Debug, Serialize)]
pub struct GatewayProbe {
    pub status: u16,
    pub content_type: String,
    pub response_bytes: usize,
    pub has_completed_event: bool,
    pub has_failed_event: bool,
    pub error_summary: Option<String>,
    pub input_tokens: Option<i64>,
}

fn is_code_assist(account: &EngineAccount) -> AppResult<bool> {
    Ok(engine_protocol(account)? == EngineProtocol::GeminiOAuth
        && account
            .credentials
            .get("oauth_type")
            .and_then(Value::as_str)
            != Some("ai_studio"))
}

fn code_assist_connection_report(value: &Value) -> AppResult<Value> {
    crate::oauth::validate_gemini_eligibility(value)?;
    // A reachable eligibility endpoint does not establish generation permissions.
    Ok(
        serde_json::json!({"ok":false,"outcome":"warning","status_code":200,
        "message":"Google 授权接口可达；此检查不验证模型调用权限。请使用实际模型请求验证账户是否可用。"}),
    )
}

async fn check_code_assist_connection(
    client: &reqwest::Client,
    account: &EngineAccount,
) -> AppResult<Value> {
    let base = account
        .credentials
        .get("base_url")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://cloudcode-pa.googleapis.com");
    let response = client.post(format!("{}/v1internal:loadCodeAssist", base.trim_end_matches('/')))
        .bearer_auth(credential(account, "access_token")?)
        .header("User-Agent", "GeminiCLI/0.1.5 (Windows; AMD64)")
        .json(&serde_json::json!({"metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}))
        .timeout(Duration::from_secs(15)).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::BadRequest(format!(
            "Google 授权资格检查失败，HTTP {}",
            status.as_u16()
        )));
    }
    code_assist_connection_report(&response.json::<Value>().await?)
}

pub async fn test_connection(state: &AppState, engine_api_key: &str) -> AppResult<Value> {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let route = repository.resolve_engine_key(engine_api_key).await?;
    if route.platform != "gemini" {
        let models = discover_models(state, engine_api_key).await?;
        return Ok(
            serde_json::json!({"ok":true,"outcome":"success","status_code":200,
            "message":"已读取授权账户的模型列表","model_count":models.len()}),
        );
    }
    let ids = repository
        .schedulable_account_ids(route.group_id, &route.platform, false)
        .await?;
    let mut checks = Vec::new();
    for id in ids {
        let Some(mut account) = repository.account_by_id(id).await? else {
            continue;
        };
        let result: AppResult<Value> = async {
            account.credentials = crate::secrets::reveal_credentials(&state.config.encryption_key, &account.credentials)?;
            refresh_oauth_if_needed(state, &repository, &mut account).await?;
            if is_code_assist(&account)? {
                check_code_assist_connection(&state.client, &account).await
            } else {
                let models = discover_account_models(state, &account).await?;
                Ok(serde_json::json!({"ok":!models.is_empty(),"outcome":if models.is_empty(){"warning"}else{"success"},
                    "status_code":200,"message":"已读取 Google 模型列表；模型调用权限需另行验证","model_count":models.len()}))
            }
        }.await;
        let mut report = match result {
            Ok(report) => report,
            Err(error) => {
                let message = match error {
                    AppError::BadRequest(message) => message,
                    _ => "Google 连接检查失败，请检查网络与授权状态。".into(),
                };
                serde_json::json!({"ok":false,"outcome":"error","status_code":0,"message":message})
            }
        };
        report["name"] = Value::String(account.name.clone());
        checks.push(report);
    }
    if checks.is_empty() {
        return Err(AppError::BadRequest("账户池没有可探测账户".into()));
    }
    if checks.len() == 1 {
        return Ok(checks.remove(0));
    }
    let ok = checks.iter().all(|r| r["ok"] == true);
    let outcome = if ok {
        "success"
    } else if checks.iter().all(|r| r["outcome"] == "error") {
        "error"
    } else {
        "warning"
    };
    Ok(
        serde_json::json!({"ok":ok,"outcome":outcome,"status_code":0,"message":"已逐一检查 Google 账户，请查看各账户结果","checks":checks}),
    )
}

pub async fn discover_models(state: &AppState, engine_api_key: &str) -> AppResult<Vec<String>> {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let route = repository.resolve_engine_key(engine_api_key).await?;
    let account_ids = repository
        .schedulable_account_ids(route.group_id, &route.platform, false)
        .await?;
    let mut last_error = None;
    for account_id in account_ids {
        let Some(mut account) = repository.account_by_id(account_id).await? else {
            continue;
        };
        account.credentials =
            crate::secrets::reveal_credentials(&state.config.encryption_key, &account.credentials)?;
        if let Err(error) = refresh_oauth_if_needed(state, &repository, &mut account).await {
            last_error = Some(error);
            continue;
        }
        match discover_account_models(state, &account).await {
            Ok(mut models) if !models.is_empty() => {
                models.sort();
                models.dedup();
                return Ok(models);
            }
            Ok(_) => {
                last_error = Some(AppError::BadRequest("上游返回了空模型列表".into()));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::BadRequest("账户池没有可探测账户".into())))
}

async fn discover_account_models(
    state: &AppState,
    account: &EngineAccount,
) -> AppResult<Vec<String>> {
    let protocol = engine_protocol(account)?;
    if is_code_assist(account)? {
        check_code_assist_connection(&state.client, account).await?;
        return Err(AppError::BadRequest("Code Assist 不提供此模型列表接口，请手动填写模型 ID；连接检查与模型调用权限需分别验证。".into()));
    }
    if protocol == EngineProtocol::Antigravity {
        let token = credential(account, "access_token")?;
        let project = credential(account, "project_id")?;
        let mut last_status = 502;
        for base in [
            credential(account, "base_url")
                .unwrap_or_else(|_| "https://daily-cloudcode-pa.googleapis.com".into()),
            "https://daily-cloudcode-pa.googleapis.com".into(),
            "https://cloudcode-pa.googleapis.com".into(),
        ] {
            let response = state
                .client
                .post(format!(
                    "{}/v1internal:fetchAvailableModels",
                    base.trim_end_matches('/')
                ))
                .bearer_auth(&token)
                .header(
                    "User-Agent",
                    if protocol == EngineProtocol::Antigravity {
                        "antigravity/1.23.2 windows/amd64"
                    } else {
                        "GeminiCLI/0.1.5 (Windows; AMD64)"
                    },
                )
                .json(&serde_json::json!({"project":project}))
                .send()
                .await?;
            last_status = response.status().as_u16();
            if !response.status().is_success() {
                continue;
            }
            let value = response.json::<Value>().await?;
            return Ok(value
                .get("models")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|models| models.keys().cloned())
                .collect());
        }
        return Err(AppError::BadRequest(format!(
            "无法读取 Google 上游模型，HTTP {last_status}"
        )));
    }

    let base = account
        .credentials
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/');
    let (url, mut request) = match protocol {
        EngineProtocol::OpenAiCodex => {
            let url = "https://chatgpt.com/backend-api/codex/models?client_version=0.200.1";
            let mut request = state
                .client
                .get(url)
                .bearer_auth(credential(account, "access_token")?)
                .header("User-Agent", "codex_cli_rs/0.200.1")
                .header("originator", "codex_cli_rs");
            if let Some(account_id) = account
                .credentials
                .get("chatgpt_account_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                request = request.header("chatgpt-account-id", account_id);
            }
            (url.to_owned(), request)
        }
        EngineProtocol::ClaudeOAuth => {
            let url = "https://api.anthropic.com/v1/models";
            (
                url.to_owned(),
                state
                    .client
                    .get(url)
                    .bearer_auth(credential(account, "access_token")?)
                    .header("anthropic-version", "2023-06-01")
                    .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,effort-2025-11-24,context-management-2025-06-27,extended-cache-ttl-2025-04-11")
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
                    .header("x-client-request-id", uuid::Uuid::new_v4().to_string()),
            )
        }
        EngineProtocol::GeminiOAuth => {
            let url = "https://generativelanguage.googleapis.com/v1beta/models";
            (
                url.to_owned(),
                state
                    .client
                    .get(url)
                    .bearer_auth(credential(account, "access_token")?),
            )
        }
        EngineProtocol::GrokOAuth => {
            let url = format!("{}/models", ensure_v1_base(base));
            let mut request = state
                .client
                .get(&url)
                .bearer_auth(credential(account, "access_token")?)
                .header("x-xai-token-auth", "xai-grok-cli")
                .header("x-grok-client-version", "0.2.120")
                .header("x-grok-client-identifier", "grok-shell")
                .header("x-grok-client-mode", "interactive")
                .header("user-agent", "xai-grok-workspace/0.2.120");
            if let Some(subject) = account
                .credentials
                .get("sub")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                request = request.header("x-userid", subject);
            }
            if let Some(email) = account
                .credentials
                .get("email")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                request = request.header("x-email", email);
            }
            (url, request)
        }
        EngineProtocol::Anthropic => {
            let url = format!("{}/models", ensure_v1_base(base));
            (
                url.clone(),
                state
                    .client
                    .get(&url)
                    .header("x-api-key", credential(account, "api_key")?)
                    .header("anthropic-version", "2023-06-01"),
            )
        }
        EngineProtocol::Gemini => {
            let normalized = base
                .strip_suffix("/v1beta")
                .or_else(|| base.strip_suffix("/v1"))
                .unwrap_or(base);
            let url = format!("{normalized}/v1beta/models");
            (
                url.clone(),
                state
                    .client
                    .get(&url)
                    .header("x-goog-api-key", credential(account, "api_key")?),
            )
        }
        EngineProtocol::OpenAiCompatible => {
            let url = format!("{}/models", base.trim_end_matches('/'));
            (
                url.clone(),
                state
                    .client
                    .get(&url)
                    .bearer_auth(credential(account, "api_key")?),
            )
        }
        EngineProtocol::Antigravity => unreachable!("handled above"),
    };
    request = request.header("accept", "application/json");
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "无法读取上游模型 {}，HTTP {}",
            url,
            response.status().as_u16()
        )));
    }
    extract_model_ids(&response.json::<Value>().await?)
}

fn ensure_v1_base(base: &str) -> String {
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

fn extract_model_ids(value: &Value) -> AppResult<Vec<String>> {
    let arrays = [value.get("data"), value.get("models")];
    for array in arrays.into_iter().flatten().filter_map(Value::as_array) {
        let models = array
            .iter()
            .filter_map(|item| {
                item.get("id")
                    .or_else(|| item.get("slug"))
                    .or_else(|| item.get("model"))
                    .or_else(|| item.get("name"))
                    .and_then(Value::as_str)
            })
            .map(|model| model.trim().trim_start_matches("models/").to_owned())
            .filter(|model| !model.is_empty() && model.len() <= 200)
            .collect::<Vec<_>>();
        if !models.is_empty() {
            return Ok(models);
        }
    }
    Err(AppError::BadRequest("上游模型响应格式不受支持".into()))
}

pub async fn probe(
    state: &AppState,
    engine_api_key: &str,
    protocol: &str,
    stream: bool,
) -> AppResult<GatewayProbe> {
    let payload = if protocol == "chat" {
        serde_json::json!({
            "model":"gpt-5.4",
            "messages":[{"role":"user","content":"Reply with OK."}],
            "stream":stream
        })
    } else if protocol == "input_tokens" {
        serde_json::json!({
            "model":"gpt-5.4",
            "instructions":"Reply with OK.",
            "input":[{
                "type":"message",
                "role":"user",
                "content":[{"type":"input_text","text":"Reply with OK."}]
            }]
        })
    } else {
        serde_json::json!({
            "model":"gpt-5.4",
            "instructions":"Reply with OK.",
            "input":[{
                "type":"message",
                "role":"user",
                "content":[{"type":"input_text","text":"Reply with OK."}]
            }],
            "store":false,
            "stream":stream
        })
    };
    let path = match protocol {
        "chat" => "/v1/chat/completions",
        "input_tokens" => "/v1/responses/input_tokens",
        _ => "/v1/responses",
    };
    let response = forward(
        state,
        engine_api_key,
        Method::POST,
        path,
        &HeaderMap::new(),
        &payload,
    )
    .await?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let text = String::from_utf8_lossy(&body);
    let parsed_body = serde_json::from_slice::<Value>(&body).ok();
    let error_summary = if status >= 400 {
        parsed_body.as_ref().and_then(|value| {
            value
                .get("detail")
                .and_then(Value::as_str)
                .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
                .map(|message| message.chars().take(300).collect())
        })
    } else {
        None
    };
    Ok(GatewayProbe {
        status,
        content_type,
        response_bytes: body.len(),
        has_completed_event: text.contains("response.completed")
            || text.contains("data: [DONE]")
            || parsed_body
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                == Some("completed")
            || parsed_body
                .as_ref()
                .and_then(|value| value.get("object"))
                .and_then(Value::as_str)
                == Some("chat.completion")
            || parsed_body
                .as_ref()
                .and_then(|value| value.get("input_tokens"))
                .is_some(),
        has_failed_event: text.contains("response.failed") || text.contains("event: error"),
        error_summary,
        input_tokens: parsed_body
            .as_ref()
            .and_then(|value| value.get("input_tokens"))
            .and_then(Value::as_i64),
    })
}

pub fn supports_path(path: &str) -> bool {
    matches!(path, "/v1/chat/completions" | "/chat/completions") || responses_suffix(path).is_some()
}

pub async fn forward(
    state: &AppState,
    engine_api_key: &str,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    payload: &Value,
) -> AppResult<Response<Body>> {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let route = repository.resolve_engine_key(engine_api_key).await?;
    let policy = crate::sub2_policy::for_group(state, route.group_id).await?;
    tokio::time::timeout(
        Duration::from_secs(policy.request_timeout_seconds as u64),
        forward_with_exclusions(
            state,
            engine_api_key,
            method,
            path,
            headers,
            payload,
            Vec::new(),
            &policy,
            None,
        ),
    )
    .await
    .map_err(|_| AppError::GatewayTimeout("账户尝试链已超过响应期限".into()))?
}

#[allow(clippy::too_many_arguments)]
async fn forward_with_exclusions(
    state: &AppState,
    engine_api_key: &str,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    payload: &Value,
    excluded_ids: Vec<i64>,
    policy: &crate::sub2_policy::DispatchPolicy,
    candidate_order: Option<Vec<i64>>,
) -> AppResult<Response<Body>> {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let route = repository.resolve_engine_key(engine_api_key).await?;
    if excluded_ids.len() >= policy.max_attempts as usize {
        return Err(AppError::Internal("已达到策略配置的最多尝试账户数".into()));
    }

    let cache = SchedulerCache::new(
        state
            .sub2_redis
            .clone()
            .ok_or_else(|| AppError::Internal("Sub2API Redis 未配置".into()))?,
        state.config.sub2api_slot_ttl_seconds,
        state.config.sub2api_wait_ttl_seconds,
    );
    let sticky = state.sub2_redis.clone().map(|connection| {
        EngineSticky::new(
            connection,
            &state.config,
            route.group_id,
            route.user_id,
            route.api_key_id,
            payload,
        )
    });
    let mut previous_response_account_id = None;
    let mut sticky_account_id = None;
    if let Some(sticky) = sticky.as_ref() {
        previous_response_account_id = sticky.previous_response_account(payload).await?;
        if policy.sticky_sessions {
            sticky_account_id = sticky.session_account().await?;
        }
    }
    let request_id = format!("junohub-gateway-{}", uuid::Uuid::new_v4());
    let eligible_ids = repository
        .schedulable_account_ids(route.group_id, &route.platform, false)
        .await?;
    let mut order = if let Some(order) = candidate_order {
        order
    } else {
        let mut accounts = repository.dispatch_accounts(route.group_id).await?;
        accounts.retain(|a| {
            a.priority > 0
                && eligible_ids.contains(&a.id)
                && (a.allowed_models.is_empty()
                    || a.allowed_models.iter().any(|model| {
                        model == "*"
                            || Some(model.as_str()) == payload.get("model").and_then(Value::as_str)
                    }))
        });
        let snapshot = if policy.strategy == "balanced" {
            snapshot_candidate_ids(&cache, &route, None, None, &excluded_ids)
                .await?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let rotation = if policy.strategy == "round_robin" {
            use redis::AsyncCommands;
            let mut connection = state
                .sub2_redis
                .clone()
                .ok_or_else(|| AppError::Internal("Redis 未配置".into()))?;
            let cursor: i64 = connection
                .incr(
                    format!("junohub:dispatch:round-robin:{}", route.group_id),
                    1,
                )
                .await
                .map_err(|error| AppError::Internal(format!("无法更新轮询游标: {error}")))?;
            cursor.saturating_sub(1) as usize
        } else {
            0
        };
        let mut order =
            crate::sub2_policy::rank_accounts(&accounts, &policy.strategy, rotation, &snapshot);
        // Session affinity never jumps over a healthier higher-priority tier.
        let first_priority = accounts.iter().map(|a| a.priority).min();
        let session = sticky_account_id.filter(|id| {
            if policy.strategy == "priority" {
                return false;
            }
            accounts
                .iter()
                .any(|a| a.id == *id && Some(a.priority) == first_priority)
        });
        crate::sub2_policy::order_candidates(&mut order, 0, session);
        order
    };
    if payload
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
    {
        let bound = if previous_response_account_id.is_some() {
            previous_response_account_id
        } else {
            let configured = repository.dispatch_accounts(route.group_id).await?;
            if configured.len() == 1 {
                Some(configured[0].id)
            } else {
                None
            }
        };
        crate::sub2_policy::pin_response_candidates(&mut order, bound)?;
    }
    if order.is_empty() {
        return Err(AppError::BadRequest(
            "没有支持该模型的可用账户，请检查模型范围和账户状态".into(),
        ));
    }
    let mut selected = None;
    for &account_id in &order {
        if excluded_ids.contains(&account_id) || !eligible_ids.contains(&account_id) {
            continue;
        }
        let Some(account) = repository.account_by_id(account_id).await? else {
            continue;
        };
        if promotable(&account, &route.platform)
            && acquire_slots(&cache, &route, &account, &request_id).await?
        {
            let mut guard = LeaseGuard::new(
                cache.clone(),
                &route,
                &account,
                request_id.clone(),
                state.config.sub2api_slot_ttl_seconds,
            );
            let rpm = account
                .extra
                .get("junohub_rpm")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            match crate::gateway_limits::acquire(
                state,
                &[(
                    format!("managed-key:{}", account.id),
                    crate::gateway_limits::Limits {
                        max_concurrency: 0,
                        rpm,
                    },
                )],
            )
            .await
            {
                Ok(lease) => {
                    guard.rate_lease = lease;
                    selected = Some((account, guard));
                    break;
                }
                Err(AppError::TooManyRequests(_)) => continue,
                Err(error) => return Err(error),
            }
        }
    }
    let (mut account, guard) =
        selected.ok_or_else(|| AppError::TooManyRequests("账户池并发或 RPM 已满".into()))?;
    account.credentials =
        crate::secrets::reveal_credentials(&state.config.encryption_key, &account.credentials)?;
    mark_account_used(state, &cache, account.id);
    tracing::info!(group_id = route.group_id, account_id = account.id, attempt = excluded_ids.len()+1, strategy = %policy.strategy, "dispatch account selected");
    if let Err(error) = refresh_oauth_if_needed(state, &repository, &mut account).await {
        drop(guard);
        let reason = format!("token refresh failed: {error}");
        let _ = repository
            .record_account_failure(account.id, None, &reason, 600)
            .await;
        let mut retry_excluded = excluded_ids.clone();
        retry_excluded.push(account.id);
        if let Ok(response) = Box::pin(forward_with_exclusions(
            state,
            engine_api_key,
            method.clone(),
            path,
            headers,
            payload,
            retry_excluded,
            policy,
            Some(order.clone()),
        ))
        .await
        {
            return Ok(response);
        }
        return Err(error);
    }
    let protocol = engine_protocol(&account)?;
    if !matches!(path, "/v1/chat/completions" | "/chat/completions")
        && responses_suffix(path).is_none()
        && !matches!(
            protocol,
            EngineProtocol::OpenAiCompatible | EngineProtocol::OpenAiCodex
        )
    {
        drop(guard);
        return Err(AppError::BadRequest(format!(
            "该平台账户仅支持 Chat Completions 协议: {}/{}",
            account.platform, account.kind
        )));
    }
    let upstream_request = match build_request(
        &state.client,
        &account,
        method.clone(),
        path,
        headers,
        payload,
    ) {
        Ok(request) => request,
        Err(error) => {
            drop(guard);
            return Err(error);
        }
    };
    let client_stream = payload.get("stream").and_then(Value::as_bool) == Some(true);
    let client_chat = matches!(path, "/v1/chat/completions" | "/chat/completions");
    let responses_unary = responses_suffix(path).is_some_and(|suffix| !suffix.is_empty());
    let upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(error) => {
            drop(guard);
            let reason = format!("upstream transport failed: {error}");
            let _ = repository
                .record_account_failure(account.id, None, &reason, policy.failure_cooldown_seconds)
                .await;
            let mut retry_excluded = excluded_ids.clone();
            retry_excluded.push(account.id);
            if let Ok(response) = Box::pin(forward_with_exclusions(
                state,
                engine_api_key,
                method.clone(),
                path,
                headers,
                payload,
                retry_excluded,
                policy,
                Some(order.clone()),
            ))
            .await
            {
                return Ok(response);
            }
            return Err(AppError::Http(error));
        }
    };
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    if should_failover_status(status) {
        let retry_after = if response_headers.contains_key(reqwest::header::RETRY_AFTER)
            || matches!(status.as_u16(), 401 | 403 | 429)
        {
            retry_after_seconds(&response_headers, status)
        } else {
            policy.failure_cooldown_seconds
        };
        let body = read_gateway_body(upstream).await?;
        drop(guard);
        let reason = upstream_error_summary(status, &body);
        let _ = repository
            .record_account_failure(account.id, Some(status.as_u16()), &reason, retry_after)
            .await;
        let mut retry_excluded = excluded_ids.clone();
        retry_excluded.push(account.id);
        if let Ok(response) = Box::pin(forward_with_exclusions(
            state,
            engine_api_key,
            method,
            path,
            headers,
            payload,
            retry_excluded,
            policy,
            Some(order.clone()),
        ))
        .await
        {
            return Ok(response);
        }
        return response_from_error(status, response_headers, body, account.id);
    }
    if status.is_success() {
        if let Err(error) = repository.clear_account_failure(account.id).await {
            tracing::warn!(account_id=account.id,error=%error,"failed to clear account failure state");
        }
    }
    if !status.is_success() && (is_anthropic(protocol) || is_gemini(protocol)) {
        let body = read_gateway_body(upstream).await;
        drop(guard);
        let error_body = body
            .ok()
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .map(protocol_error_to_openai)
            .unwrap_or_else(
                || serde_json::json!({"error":{"message":"上游请求失败","type":"api_error"}}),
            );
        let mut response = Response::new(Body::from(serde_json::to_vec(&error_body)?));
        *response.status_mut() = status;
        response.headers_mut().insert(
            "content-type",
            "application/json".parse().expect("static header"),
        );
        response.headers_mut().insert(
            "x-junohub-engine-account-id",
            account.id.to_string().parse().unwrap(),
        );
        return Ok(response);
    }
    if account.kind == "oauth"
        && account.platform == "openai"
        && !client_stream
        && !responses_unary
        && status.is_success()
    {
        let body = read_gateway_body(upstream).await?;
        let response_value = terminal_response_from_sse(&body)?;
        bind_sticky_artifacts(sticky.as_ref(), &response_value, account.id).await;
        let response_value = if client_chat {
            crate::sub2_protocol::validate_responses_result(&response_value, payload)?;
            response_to_chat_completion(&response_value, payload)
        } else {
            let mut value = response_value;
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                value["model"] = Value::String(model.to_owned());
            }
            value
        };
        drop(guard);
        let mut response = Response::new(Body::from(serde_json::to_vec(&response_value)?));
        *response.status_mut() = status;
        response.headers_mut().insert(
            "content-type",
            "application/json".parse().expect("static header"),
        );
        response.headers_mut().insert(
            "x-junohub-engine-account-id",
            account.id.to_string().parse().unwrap(),
        );
        return Ok(response);
    }
    if (is_anthropic(protocol) || is_gemini(protocol)) && client_chat {
        if client_stream {
            let stream = protocol_sse_to_chat_stream(upstream, guard, payload, protocol);
            let mut response = Response::new(Body::from_stream(stream));
            *response.status_mut() = status;
            response.headers_mut().insert(
                "content-type",
                "text/event-stream".parse().expect("static header"),
            );
            response.headers_mut().insert(
                "x-junohub-engine-account-id",
                account.id.to_string().parse().unwrap(),
            );
            return Ok(response);
        }
        let body = read_gateway_body(upstream).await?;
        let mut parsed: Value = serde_json::from_slice(&body)
            .map_err(|_| AppError::BadGateway("上游返回无效 JSON".into()))?;
        if matches!(
            protocol,
            EngineProtocol::GeminiOAuth | EngineProtocol::Antigravity
        ) {
            if let Some(response) = parsed.get("response").cloned() {
                parsed = response;
            }
        }
        let chat = native_response_to_chat_completion(&parsed, payload, protocol)?;
        drop(guard);
        let mut response = Response::new(Body::from(serde_json::to_vec(&chat)?));
        *response.status_mut() = status;
        response.headers_mut().insert(
            "content-type",
            "application/json".parse().expect("static header"),
        );
        response.headers_mut().insert(
            "x-junohub-engine-account-id",
            account.id.to_string().parse().unwrap(),
        );
        return Ok(response);
    }
    let stream_guard = guard;
    let stream: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> =
        if should_project_responses_to_chat(protocol, client_chat, status) {
            chat_completion_sse_stream(upstream, stream_guard, payload, sticky, account.id)
        } else if !client_chat
            && client_stream
            && responses_suffix(path).is_some()
            && status.is_success()
        {
            sticky_binding_stream(upstream, stream_guard, sticky.as_ref(), account.id)
        } else {
            passthrough_stream_from_chunks(
                upstream
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(std::io::Error::other)),
                Some(stream_guard),
            )
        };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    for (name, value) in response_headers {
        if let Some(name) = name {
            if !matches!(
                name.as_str(),
                "content-length" | "connection" | "transfer-encoding"
            ) {
                response.headers_mut().insert(name, value);
            }
        }
    }
    // Codex can omit Content-Type even though this branch emits SSE frames.
    // Declare our response format so accounting and clients keep streaming.
    if protocol == EngineProtocol::OpenAiCodex && !responses_unary && status.is_success() {
        response.headers_mut().insert(
            "content-type",
            "text/event-stream".parse().expect("static header"),
        );
    }
    response.headers_mut().insert(
        "x-junohub-engine-account-id",
        account.id.to_string().parse().unwrap(),
    );
    Ok(response)
}

fn credentials_expire_at(account: &EngineAccount) -> Option<i64> {
    account.credentials.get("expires_at").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn should_failover_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status.as_u16(),
        401 | 403 | 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504
    )
}

pub(crate) fn retry_after_seconds(
    headers: &reqwest::header::HeaderMap,
    status: reqwest::StatusCode,
) -> i64 {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.trim().parse::<i64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|date| {
                        (date.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds()
                    })
            })
        })
        .unwrap_or_else(|| match status.as_u16() {
            401 | 403 => 600,
            429 => 60,
            _ => 30,
        })
        .clamp(5, 3600)
}

fn upstream_error_summary(status: reqwest::StatusCode, body: &[u8]) -> String {
    let parsed = serde_json::from_slice::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("detail"))
                .or_else(|| value.get("message"))
        })
        .and_then(Value::as_str)
        .unwrap_or("upstream request failed");
    format!(
        "HTTP {}: {}",
        status.as_u16(),
        message.chars().take(300).collect::<String>()
    )
}

fn response_from_error(
    status: reqwest::StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Bytes,
    account_id: i64,
) -> AppResult<Response<Body>> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    for (name, value) in headers {
        if let Some(name) = name {
            if !matches!(
                name.as_str(),
                "content-length" | "connection" | "transfer-encoding"
            ) {
                response.headers_mut().insert(name, value);
            }
        }
    }
    response.headers_mut().insert(
        "x-junohub-engine-account-id",
        account_id.to_string().parse().expect("account id header"),
    );
    Ok(response)
}

async fn refresh_oauth_if_needed(
    state: &AppState,
    repository: &Sub2Repository,
    account: &mut EngineAccount,
) -> AppResult<()> {
    if account.kind != "oauth" {
        return Ok(());
    }
    if credentials_expire_at(account)
        .is_some_and(|expires| expires > chrono::Utc::now().timestamp() + 60)
    {
        return Ok(());
    }
    let old_refresh_token = credential(account, "refresh_token")?;
    let client_id = account.credentials.get("client_id").and_then(Value::as_str);
    let (access_token, refresh_token, token_type, expires_in, id_token, refresh_client_id) =
        if account.platform == "openai" {
            let token =
                crate::oauth::refresh_openai_token(&state.client, &old_refresh_token, client_id)
                    .await?;
            (
                token.access_token,
                token.refresh_token,
                token.token_type,
                token.expires_in,
                token.id_token,
                token.client_id,
            )
        } else {
            let token = crate::oauth::refresh_provider_token(
                &state.client,
                &account.platform,
                &old_refresh_token,
                &account.credentials,
            )
            .await?;
            (
                token.access_token,
                token.refresh_token,
                token.token_type,
                token.expires_in,
                token.id_token,
                token.client_id,
            )
        };
    let expires_at = chrono::Utc::now()
        .timestamp()
        .saturating_add(expires_in.saturating_sub(300).max(60));
    let mut patch = serde_json::json!({
        "access_token":access_token,
        "refresh_token":refresh_token,
        "token_type":token_type,
        "expires_at":expires_at,
        "client_id":refresh_client_id
    });
    if !id_token.trim().is_empty() {
        patch["id_token"] = Value::String(id_token);
    }
    if repository
        .update_openai_tokens_cas(
            account.id,
            &old_refresh_token,
            &patch,
            &state.config.encryption_key,
        )
        .await?
    {
        merge_json_object(&mut account.credentials, &patch);
    } else if let Some(mut fresh) = repository.account_by_id(account.id).await? {
        fresh.credentials =
            crate::secrets::reveal_credentials(&state.config.encryption_key, &fresh.credentials)?;
        *account = fresh;
    }
    Ok(())
}

fn merge_json_object(target: &mut Value, patch: &Value) {
    if !target.is_object() {
        *target = patch.clone();
        return;
    }
    let target = target.as_object_mut().expect("checked object");
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn promotable(account: &EngineAccount, platform: &str) -> bool {
    let now = chrono::Utc::now();
    account.status == "active"
        && account.schedulable
        && account.platform == platform
        && account
            .temp_unschedulable_until
            .is_none_or(|until| now >= until)
        && account.overload_until.is_none_or(|until| now >= until)
        && account.rate_limit_reset_at.is_none_or(|until| now >= until)
        && account
            .expires_at
            .is_none_or(|until| !(account.auto_pause_on_expired && now >= until))
}

fn account_concurrency(account: &EngineAccount) -> i32 {
    account
        .load_factor
        .filter(|value| *value > 0)
        .unwrap_or_else(|| account.concurrency.max(1))
}

async fn acquire_slots(
    cache: &SchedulerCache,
    route: &EngineKeyRoute,
    account: &EngineAccount,
    request_id: &str,
) -> AppResult<bool> {
    if !cache
        .acquire_user_slot(route.user_id, i32::MAX, request_id)
        .await?
    {
        return Ok(false);
    }
    if let Err(error) = cache.track_api_key_slot(route.api_key_id, request_id).await {
        let _ = cache.release_user_slot(route.user_id, request_id).await;
        return Err(error);
    }
    if !cache
        .acquire_account_slot(account.id, account_concurrency(account), request_id)
        .await?
    {
        let _ = cache
            .release_api_key_slot(route.api_key_id, request_id)
            .await;
        let _ = cache.release_user_slot(route.user_id, request_id).await;
        return Ok(false);
    }
    Ok(true)
}

async fn snapshot_candidate_ids(
    cache: &SchedulerCache,
    route: &EngineKeyRoute,
    previous_response_account_id: Option<i64>,
    sticky_account_id: Option<i64>,
    excluded_ids: &[i64],
) -> AppResult<Option<Vec<i64>>> {
    let modes: &[&str] = if matches!(route.platform.as_str(), "anthropic" | "gemini") {
        &["forced", "single", "mixed"]
    } else {
        &["forced", "single"]
    };
    for mode in modes {
        let bucket = SchedulerBucket {
            group_id: route.group_id,
            platform: route.platform.clone(),
            mode: (*mode).to_owned(),
        };
        let Some(version) = cache.active_version(&bucket).await? else {
            continue;
        };
        let request = SelectionRequest {
            bucket_key: format!(
                "sched:{}:{}:{}:v{}",
                route.group_id, route.platform, mode, version
            ),
            platform: route.platform.clone(),
            excluded_ids: excluded_ids.to_vec(),
            sticky_account_id,
            previous_response_account_id,
        };
        let snapshot = cache.selection_snapshot(request).await?;
        let ids: Vec<i64> = snapshot
            .ranked
            .into_iter()
            .map(|candidate| candidate.account_id)
            .collect();
        if !ids.is_empty() {
            return Ok(Some(ids));
        }
    }
    Ok(None)
}

fn mark_account_used(state: &AppState, cache: &SchedulerCache, account_id: i64) {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let cache = cache.clone();
    let timestamp = chrono::Utc::now().timestamp();
    tokio::spawn(async move {
        if let Err(error) = repository.touch_account_last_used(account_id).await {
            tracing::warn!(account_id, error = %error, "failed to record account last_used");
        }
        if let Err(error) = cache.update_last_used(account_id, timestamp).await {
            tracing::warn!(account_id, error = %error, "failed to refresh scheduler last_used");
        }
    });
}

#[derive(Clone)]
struct EngineSticky {
    store: StickyStore,
    group_id: i64,
    user_id: i64,
    api_key_id: i64,
    session_hashes: Option<(String, String)>,
    previous_response_id: Option<String>,
}

impl EngineSticky {
    fn new(
        connection: redis::aio::ConnectionManager,
        config: &Config,
        group_id: i64,
        user_id: i64,
        api_key_id: i64,
        payload: &Value,
    ) -> Self {
        let store = StickyStore::new(
            connection,
            config.sub2api_sticky_ttl_seconds,
            config.sub2api_response_sticky_ttl_seconds,
            config.sub2api_session_hash_read_old_fallback,
            config.sub2api_session_hash_dual_write_old,
        );
        let session_hashes = session_seed(payload)
            .and_then(|seed| crate::sub2_sticky::derive_session_hashes(&seed).ok());
        let previous_response_id = payload
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Self {
            store,
            group_id,
            user_id,
            api_key_id,
            session_hashes,
            previous_response_id,
        }
    }

    async fn previous_response_account(&self, _payload: &Value) -> AppResult<Option<i64>> {
        let Some(response_id) = self.previous_response_id.as_deref() else {
            return Ok(None);
        };
        self.store
            .get_response_account(self.group_id, response_id)
            .await
    }

    async fn session_account(&self) -> AppResult<Option<i64>> {
        let Some((current, legacy)) = self.session_hashes.as_ref() else {
            return Ok(None);
        };
        self.store
            .get_session_account(self.group_id, current, Some(legacy))
            .await
    }

    async fn bind_response_and_session(&self, response_id: &str, account_id: i64) {
        let response_id = response_id.trim();
        if response_id.is_empty() {
            return;
        }
        if let Err(error) = self
            .store
            .bind_response_account(self.group_id, response_id, account_id)
            .await
        {
            tracing::warn!(account_id, error = %error, "failed to bind response account");
        }
        if let Err(error) = self
            .store
            .bind_response_owner(self.group_id, response_id, self.user_id, self.api_key_id)
            .await
        {
            tracing::warn!(account_id, error = %error, "failed to bind response owner");
        }
        if let Some((current, legacy)) = self.session_hashes.as_ref() {
            if let Err(error) = self
                .store
                .bind_session_account(self.group_id, current, Some(legacy), account_id)
                .await
            {
                tracing::warn!(account_id, error = %error, "failed to bind sticky session");
            }
        }
    }
}

fn session_seed(payload: &Value) -> Option<String> {
    for field in ["session_id", "prompt_cache_key"] {
        if let Some(value) = payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_owned());
        }
    }
    None
}

async fn bind_sticky_artifacts(sticky: Option<&EngineSticky>, response: &Value, account_id: i64) {
    let Some(sticky) = sticky else { return };
    if let Some(response_id) = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        sticky
            .bind_response_and_session(response_id, account_id)
            .await;
    }
}

async fn read_gateway_body(upstream: reqwest::Response) -> AppResult<Bytes> {
    collect_gateway_body(
        upstream
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other)),
        32 * 1024 * 1024,
    )
    .await
}

async fn collect_gateway_body<S>(upstream: S, limit: usize) -> AppResult<Bytes>
where
    S: Stream<Item = Result<Bytes, std::io::Error>>,
{
    let mut upstream = Box::pin(upstream);
    let mut body = Vec::new();
    while let Some(chunk) = upstream.next().await {
        let chunk = chunk.map_err(|_| AppError::BadGateway("上游响应读取失败".into()))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(AppError::BadGateway("上游响应超过大小限制".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn should_project_responses_to_chat(
    protocol: EngineProtocol,
    client_chat: bool,
    status: reqwest::StatusCode,
) -> bool {
    protocol == EngineProtocol::OpenAiCodex && client_chat && status.is_success()
}

fn passthrough_stream_from_chunks<S>(
    upstream: S,
    guard: Option<LeaseGuard>,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    Box::pin(async_stream::try_stream! {
        let _guard = guard;
        let mut upstream = Box::pin(upstream);
        while let Some(chunk) = upstream.next().await {
            yield chunk?;
        }
    })
}

fn chat_completion_sse_stream(
    upstream: reqwest::Response,
    guard: LeaseGuard,
    request: &Value,
    sticky: Option<EngineSticky>,
    account_id: i64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
    chat_completion_stream_from_chunks(
        upstream
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other)),
        Some(guard),
        request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        sticky,
        account_id,
    )
}

fn chat_completion_stream_from_chunks<S>(
    upstream: S,
    guard: Option<LeaseGuard>,
    model: String,
    sticky: Option<EngineSticky>,
    account_id: i64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    Box::pin(async_stream::stream! {
        let _guard = guard;
        let mut events = crate::sub2_sse::events(upstream);
        let mut response_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
        let created = chrono::Utc::now().timestamp();
        let mut bridge = crate::sub2_chat_bridge::ChatBridge::default();
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    yield Ok(chat_stream_error(&error.to_string()));
                    yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                    return;
                }
            };
            if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                response_id = id.to_owned();
            }
            let outputs = match bridge.convert(&event, &response_id, &model, created) {
                Ok(outputs) => outputs,
                Err(message) => {
                    yield Ok(chat_stream_error(&message));
                    yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                    return;
                }
            };
            if bridge.completed && event["type"] == "response.completed" {
                bind_sticky_artifacts(sticky.as_ref(), &event["response"], account_id).await;
            }
            for output in outputs {
                yield Ok(chat_data_chunk(output));
            }
            if bridge.completed {
                yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                return;
            }
        }
        yield Ok(chat_stream_error("上游响应提前结束，未收到终止事件"));
        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
    })
}

fn chat_stream_error(message: &str) -> Bytes {
    chat_data_chunk(serde_json::json!({"error":{
        "message":message.chars().take(400).collect::<String>(),
        "type":"server_error"
    }}))
}

fn sticky_binding_stream(
    upstream: reqwest::Response,
    guard: LeaseGuard,
    sticky: Option<&EngineSticky>,
    account_id: i64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
    let sticky = sticky.cloned();
    Box::pin(async_stream::try_stream! {
        let _guard = guard;
        let mut upstream = upstream.bytes_stream();
        let mut buffer = Vec::new();
        let mut bound = false;
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            let mut scan_from = buffer.len().saturating_sub(3);
            buffer.extend_from_slice(&chunk);
            while let Some((end, delimiter_len)) = crate::sub2_sse::frame_end(&buffer[scan_from..]) {
                let end = scan_from + end;
                if end > crate::sub2_sse::MAX_FRAME_BYTES {
                    Err(std::io::Error::other("上游 SSE 单个事件超过 32MB 限制"))?;
                }
                let frame = buffer.drain(..end + delimiter_len).collect::<Vec<_>>();
                scan_from = 0;
                if !bound {
                    if let Some(response_id) = extract_completed_response_id(&frame) {
                        if let Some(sticky) = sticky.as_ref() {
                            sticky.bind_response_and_session(&response_id, account_id).await;
                        }
                        bound = true;
                    }
                }
                yield Bytes::from(frame);
            }
            if buffer.len() > crate::sub2_sse::MAX_FRAME_BYTES {
                Err(std::io::Error::other("上游 SSE 单个事件超过 32MB 限制"))?;
            }
        }
        if !buffer.is_empty() {
            yield Bytes::from(buffer);
        }
    })
}

fn extract_completed_response_id(frame: &[u8]) -> Option<String> {
    let Some(crate::sub2_sse::Frame::Event(event)) = crate::sub2_sse::decode(frame).ok()? else {
        return None;
    };
    if event["type"] != "response.completed"
        || event
            .pointer("/response/status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "completed")
    {
        return None;
    }
    event
        .pointer("/response/id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
fn response_event_to_chat_chunks(
    event: &Value,
    response_id: &str,
    model: &str,
    created: i64,
) -> Vec<Value> {
    crate::sub2_chat_bridge::ChatBridge::default()
        .convert(event, response_id, model, created)
        .unwrap_or_default()
}

fn protocol_sse_to_chat_stream(
    upstream: reqwest::Response,
    guard: LeaseGuard,
    request: &Value,
    protocol: EngineProtocol,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
    protocol_chat_stream_from_chunks(
        upstream
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other)),
        Some(guard),
        request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        protocol,
    )
}

fn protocol_chat_stream_from_chunks<S>(
    upstream: S,
    guard: Option<LeaseGuard>,
    model: String,
    protocol: EngineProtocol,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    Box::pin(async_stream::stream! {
        let _guard = guard;
        let mut events = crate::sub2_sse::events(upstream);
        let mut chat_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
        let created = chrono::Utc::now().timestamp();
        let mut finish: Option<&'static str> = None;
        let mut opened = false;
        let mut tool_calls_seen = false;
        let mut pending_empty_tools = std::collections::BTreeSet::new();
        let mut gemini_tools: Vec<Value> = Vec::new();
        let mut upstream_usage = serde_json::Map::new();
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    yield Ok(chat_stream_error(&error.to_string()));
                    yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                    return;
                }
            };
            let event = if is_gemini(protocol) { event.get("response").cloned().unwrap_or(event) } else { event };
            if has_upstream_error(&event) {
                let mut error = protocol_error_to_openai(event);
                if let Some(message) = error.pointer("/error/message").and_then(Value::as_str) {
                    error["error"]["message"] = Value::String(message.chars().take(400).collect());
                }
                yield Ok(chat_data_chunk(error));
                yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                return;
            }
            if is_anthropic(protocol) {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                match event.get("type").and_then(Value::as_str) {
                    Some("message_start") => {
                        if let Some(id) = event.pointer("/message/id").and_then(Value::as_str) {
                            chat_id = id.to_owned();
                        }
                        if let Some(usage) = event.pointer("/message/usage").and_then(Value::as_object) {
                            upstream_usage.extend(usage.clone());
                        }
                        if !opened {
                            opened = true;
                            yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({"role":"assistant"}), Value::Null, Value::Null)));
                        }
                    }
                    Some("content_block_start") => {
                        if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                            tool_calls_seen = true;
                            let input = event.pointer("/content_block/input");
                            let arguments = if input.and_then(Value::as_object).is_some_and(|input| !input.is_empty()) {
                                input.unwrap().to_string()
                            } else {
                                pending_empty_tools.insert(index);
                                String::new()
                            };
                            let delta = serde_json::json!({"tool_calls":[{
                                "index":index,"id":event.pointer("/content_block/id"),"type":"function",
                                "function":{"name":event.pointer("/content_block/name"),"arguments":arguments}
                            }]});
                            yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, delta, Value::Null, Value::Null)));
                        }
                    }
                    Some("content_block_delta") => {
                        let delta = match event.pointer("/delta/type").and_then(Value::as_str) {
                            Some("text_delta") => serde_json::json!({"content":event.pointer("/delta/text")}),
                            Some("input_json_delta") => {
                                let arguments = event.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or_default();
                                if !arguments.is_empty() { pending_empty_tools.remove(&index); }
                                serde_json::json!({"tool_calls":[{"index":index,"function":{"arguments":arguments}}]})
                            }
                            _ => continue,
                        };
                        yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, delta, Value::Null, Value::Null)));
                    }
                    Some("content_block_stop") => {
                        if pending_empty_tools.remove(&index) {
                            yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created,
                                serde_json::json!({"tool_calls":[{"index":index,"function":{"arguments":"{}"}}]}), Value::Null, Value::Null)));
                        }
                    }
                    Some("message_delta") => {
                        if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                            finish = Some(anthropic_finish_reason(Some(reason)));
                        }
                        // Usage updates are cumulative; replace fields rather than adding them.
                        if let Some(usage) = event.get("usage").and_then(Value::as_object) {
                            upstream_usage.extend(usage.clone());
                        }
                    }
                    Some("message_stop") => {
                        for index in pending_empty_tools {
                            yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created,
                                serde_json::json!({"tool_calls":[{"index":index,"function":{"arguments":"{}"}}]}), Value::Null, Value::Null)));
                        }
                        let finish_reason = match finish {
                            Some("stop") | None if tool_calls_seen => "tool_calls",
                            Some(value) => value,
                            None => "stop",
                        };
                        yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({}), Value::String(finish_reason.into()),
                            anthropic_chat_usage(&Value::Object(upstream_usage)))));
                        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                        return;
                    }
                    _ => {}
                }
            } else if is_gemini(protocol) {
                if !opened {
                    opened = true;
                    yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({"role":"assistant"}), Value::Null, Value::Null)));
                }
                let candidate = &event["candidates"][0];
                for part in candidate.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(text) = part.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()) {
                        let field = if part.get("thought").and_then(Value::as_bool) == Some(true) { "reasoning_content" } else { "content" };
                        let mut delta = serde_json::Map::new();
                        delta.insert(field.into(), Value::String(text.to_owned()));
                        yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, Value::Object(delta), Value::Null, Value::Null)));
                    }
                    if let Some(call) = part.get("functionCall") {
                        tool_calls_seen = true;
                        let mut converted = gemini_tool_call(part);
                        let existing = call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())
                            .and_then(|id| gemini_tools.iter().position(|tool| tool["id"] == id));
                        if let Some(index) = existing {
                            if converted.get("extra_content").is_none() {
                                if let Some(extra) = gemini_tools[index].get("extra_content") { converted["extra_content"] = extra.clone(); }
                            }
                            gemini_tools[index] = converted;
                        } else {
                            gemini_tools.push(converted);
                        }
                    }
                }
                if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str).filter(|reason| !reason.is_empty()) {
                    finish = Some(gemini_finish_reason(Some(reason)));
                }
                if gemini_prompt_blocked(&event) {
                    finish = Some("content_filter");
                }
                if let Some(usage) = event.get("usageMetadata").and_then(Value::as_object) {
                    upstream_usage.extend(usage.clone());
                }
            }
        }
        if is_gemini(protocol) && finish.is_some() {
            for (index, mut tool) in gemini_tools.into_iter().enumerate() {
                tool["index"] = Value::from(index);
                yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({"tool_calls":[tool]}), Value::Null, Value::Null)));
            }
            let finish_reason = match finish {
                Some("stop") if tool_calls_seen => "tool_calls",
                Some(value) => value,
                None => unreachable!(),
            };
            yield Ok(chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({}), Value::String(finish_reason.into()), gemini_chat_usage(&Value::Object(upstream_usage)))));
        } else {
            yield Ok(chat_stream_error("上游响应提前结束，未收到终止事件"));
        }
        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
    })
}

fn chat_data_chunk(chunk: Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        serde_json::to_string(&chunk).unwrap_or_default()
    ))
}

fn terminal_response_from_sse(body: &[u8]) -> AppResult<Value> {
    // Codex(ChatGPT OAuth) 后端的 response.completed 事件中 output 为空数组，
    // 完整输出只出现在 response.output_item.done 增量事件里，需要组装。
    let mut output_items: std::collections::BTreeMap<i64, Value> =
        std::collections::BTreeMap::new();
    let mut remaining = body;
    while !remaining.is_empty() {
        let Some((end, delimiter)) = crate::sub2_sse::frame_end(remaining) else {
            return Err(AppError::BadGateway("上游 SSE 事件未完整结束".into()));
        };
        let frame = crate::sub2_sse::decode(&remaining[..end])
            .map_err(|error| AppError::BadGateway(error.to_string()))?;
        remaining = &remaining[end + delimiter..];
        let event = match frame {
            Some(crate::sub2_sse::Frame::Event(event)) => event,
            Some(crate::sub2_sse::Frame::Done) => break,
            None => continue,
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                let item = event
                    .get("item")
                    .filter(|item| item.is_object())
                    .ok_or_else(|| AppError::BadGateway("上游输出项目格式无效".into()))?;
                let previous_index = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .and_then(|id| {
                        output_items
                            .iter()
                            .find_map(|(index, previous)| (previous["id"] == id).then_some(*index))
                    });
                let index = match previous_index {
                    Some(index) => index,
                    None => event
                        .get("output_index")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| {
                            AppError::BadGateway(
                                "上游 output_item.done 缺少有效 output_index".into(),
                            )
                        })?,
                };
                if !(0..1024).contains(&index)
                    || (!output_items.contains_key(&index) && output_items.len() >= 1024)
                {
                    return Err(AppError::BadGateway("上游输出项目超出有效范围".into()));
                }
                if output_items
                    .get(&index)
                    .is_some_and(|previous| previous != item)
                {
                    return Err(AppError::BadGateway(
                        "上游在同一 output_index 返回了不同输出项目".into(),
                    ));
                }
                output_items.insert(index, item.clone());
            }
            Some("response.completed") | Some("response.incomplete") => {
                let terminal_kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut response = event.get("response").cloned().ok_or_else(|| {
                    AppError::BadGateway(format!("{terminal_kind} 缺少 response"))
                })?;
                if !response.is_object() {
                    return Err(AppError::BadGateway(format!(
                        "{terminal_kind} 的 response 格式无效"
                    )));
                }
                if response.get("status").and_then(Value::as_str) == Some("failed") {
                    return Err(AppError::BadGateway(response_error_message(&event)));
                }
                if response
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| !matches!(status, "completed" | "incomplete"))
                {
                    return Err(AppError::BadGateway("上游终止响应的状态无效".into()));
                }
                if terminal_kind == "response.incomplete" {
                    response["status"] = Value::String("incomplete".into());
                }
                let completed_output_empty = response
                    .get("output")
                    .and_then(Value::as_array)
                    .is_none_or(|items| items.is_empty());
                if completed_output_empty && !output_items.is_empty() {
                    if output_items
                        .keys()
                        .enumerate()
                        .any(|(expected, actual)| *actual != expected as i64)
                    {
                        return Err(AppError::BadGateway(
                            "上游输出项目的 output_index 不连续".into(),
                        ));
                    }
                    response["output"] = Value::Array(output_items.into_values().collect());
                }
                return Ok(response);
            }
            Some("response.failed") | Some("error") => {
                return Err(AppError::BadGateway(response_error_message(&event)));
            }
            _ => {}
        }
    }
    Err(AppError::BadGateway("上游 SSE 缺少终止事件".into()))
}

struct LeaseGuard {
    rate_lease: Option<crate::gateway_limits::Lease>,
    cache: Option<SchedulerCache>,
    account_id: i64,
    user_id: i64,
    api_key_id: i64,
    request_id: String,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
}

impl LeaseGuard {
    fn new(
        cache: SchedulerCache,
        route: &EngineKeyRoute,
        account: &EngineAccount,
        request_id: String,
        slot_ttl_seconds: i64,
    ) -> Self {
        let heartbeat_cache = cache.clone();
        let heartbeat_request_id = request_id.clone();
        let account_id = account.id;
        let account_limit = account_concurrency(account);
        let user_id = route.user_id;
        let api_key_id = route.api_key_id;
        let heartbeat_seconds = (slot_ttl_seconds / 3).clamp(1, 60);
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(heartbeat_seconds as u64)).await;
                let account_ok = heartbeat_cache
                    .acquire_account_slot(account_id, account_limit, &heartbeat_request_id)
                    .await;
                let user_ok = heartbeat_cache
                    .acquire_user_slot(user_id, i32::MAX, &heartbeat_request_id)
                    .await;
                let key_ok = heartbeat_cache
                    .track_api_key_slot(api_key_id, &heartbeat_request_id)
                    .await;
                if !matches!(account_ok, Ok(true))
                    || !matches!(user_ok, Ok(true))
                    || key_ok.is_err()
                {
                    tracing::warn!(
                        account_id,
                        user_id,
                        api_key_id,
                        "failed to refresh gateway concurrency lease"
                    );
                }
            }
        });
        Self {
            rate_lease: None,
            cache: Some(cache),
            account_id,
            user_id,
            api_key_id,
            request_id,
            heartbeat: Some(heartbeat),
        }
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let heartbeat = self.heartbeat.take();
        if let Some(heartbeat) = heartbeat.as_ref() {
            heartbeat.abort();
        }
        let Some(cache) = self.cache.take() else {
            return;
        };
        let account_id = self.account_id;
        let user_id = self.user_id;
        let api_key_id = self.api_key_id;
        let request_id = std::mem::take(&mut self.request_id);
        tokio::spawn(async move {
            // A refresh already in progress must stop before the final Redis
            // release; otherwise it can reacquire a slot after cleanup.
            if let Some(heartbeat) = heartbeat {
                let _ = heartbeat.await;
            }
            if let Err(error) = cache.release_account_slot(account_id, &request_id).await {
                tracing::warn!(account_id, request_id, error = %error, "failed to release Rust gateway account slot");
            }
            if let Err(error) = cache.release_api_key_slot(api_key_id, &request_id).await {
                tracing::warn!(api_key_id,request_id,error=%error,"failed to release Rust gateway API key slot");
            }
            if let Err(error) = cache.release_user_slot(user_id, &request_id).await {
                tracing::warn!(user_id,request_id,error=%error,"failed to release Rust gateway user slot");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn completed_response_binding_uses_full_multiline_event_and_valid_status() {
        let frame = b"data: {\"type\":\"response.completed\",\n data: ignored\ndata: \"response\":{\"id\":\"r1\",\"status\":\"completed\"}}\n\n";
        assert_eq!(extract_completed_response_id(frame), Some("r1".into()));
        assert_eq!(extract_completed_response_id(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"failed\"}}\n\n"), None);
    }

    #[test]
    fn unary_sse_output_items_are_ordered_and_not_duplicated() {
        let events = [
            serde_json::json!({"type":"response.output_item.done","output_index":1,"item":{"id":"b","type":"function_call","call_id":"b","name":"lookup","arguments":"{}"}}),
            serde_json::json!({"type":"response.output_item.done","output_index":0,"item":{"id":"a","type":"message","content":[{"type":"output_text","text":"Answer"}]}}),
            serde_json::json!({"type":"response.output_item.done","output_index":1,"item":{"id":"b","type":"function_call","call_id":"b","name":"lookup","arguments":"{}"}}),
            serde_json::json!({"type":"response.completed","response":{"output":[]}}),
        ];
        let body = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let response = terminal_response_from_sse(body.as_bytes()).unwrap();
        assert_eq!(response["output"].as_array().unwrap().len(), 2);
        assert_eq!(response["output"][0]["id"], "a");
        assert_eq!(response["output"][1]["id"], "b");
        let error = terminal_response_from_sse(
            b"data: {\"type\":\"error\",\"message\":\"quota exhausted\"}\n\n",
        )
        .unwrap_err();
        assert!(matches!(error, AppError::BadGateway(ref message) if message == "quota exhausted"));
    }

    #[tokio::test]
    async fn null_error_in_native_stream_does_not_cancel_generation() {
        for (protocol, events) in [
            (
                EngineProtocol::Anthropic,
                vec![
                    serde_json::json!({"type":"content_block_delta","error":null,"delta":{"type":"text_delta","text":"Answer"}}),
                    serde_json::json!({"type":"message_stop","error":null}),
                ],
            ),
            (
                EngineProtocol::Gemini,
                vec![
                    serde_json::json!({"error":null,"candidates":[{"content":{"parts":[{"text":"Answer"}]},"finishReason":"STOP"}]}),
                ],
            ),
        ] {
            let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
                review_fixture_chunks(events),
                None,
                "test".into(),
                protocol,
            ))
            .await;
            assert_eq!(done, 1);
            assert!(!frames.iter().any(has_upstream_error));
            assert_eq!(
                frames.last().unwrap()["choices"][0]["finish_reason"],
                "stop"
            );
        }
    }
    #[tokio::test]
    async fn dropping_lease_guard_cancels_an_inflight_heartbeat() {
        struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for NotifyOnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        let heartbeat = tokio::spawn(async move {
            let _notify = NotifyOnDrop(Some(stopped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.unwrap();
        let guard = LeaseGuard {
            rate_lease: None,
            cache: None,
            account_id: 1,
            user_id: 1,
            api_key_id: 1,
            request_id: "fixture".into(),
            heartbeat: Some(heartbeat),
        };
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), stopped_rx)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn review_collected_bodies_enforce_size_limit_across_chunks() {
        let make_chunks = || {
            futures_util::stream::iter(vec![
                Ok(Bytes::from_static(b"12345")),
                Ok(Bytes::from_static(b"67890")),
            ])
        };
        assert!(matches!(
            collect_gateway_body(make_chunks(), 8).await,
            Err(AppError::BadGateway(_))
        ));
        let body = collect_gateway_body(make_chunks(), 10).await.unwrap();
        assert_eq!(body.as_ref(), b"1234567890");
    }

    #[test]
    fn retry_after_supports_http_dates() {
        let mut headers = reqwest::header::HeaderMap::new();
        let date = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
        headers.insert(reqwest::header::RETRY_AFTER, date.parse().unwrap());
        assert!((88..=90).contains(&super::retry_after_seconds(
            &headers,
            reqwest::StatusCode::TOO_MANY_REQUESTS
        )));
    }
    use super::*;
    use chrono::Utc;

    fn account(kind: &str, credentials: Value) -> EngineAccount {
        EngineAccount {
            id: 1,
            name: "test".into(),
            notes: None,
            platform: "openai".into(),
            kind: kind.into(),
            credentials,
            extra: serde_json::json!({}),
            proxy_id: None,
            proxy_fallback_origin_id: None,
            concurrency: 1,
            load_factor: None,
            priority: 50,
            rate_multiplier: 1.0,
            status: "active".into(),
            schedulable: true,
            error_message: None,
            last_used_at: None,
            expires_at: None,
            auto_pause_on_expired: false,
            rate_limited_at: None,
            rate_limit_reset_at: None,
            overload_until: None,
            temp_unschedulable_until: None,
            temp_unschedulable_reason: None,
            session_window_start: None,
            session_window_end: None,
            session_window_status: None,
            parent_account_id: None,
            quota_dimension: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn code_assist_diagnostic_uses_eligibility_endpoint_and_preserves_rejection() {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route(
            "/v1internal:loadCodeAssist",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer test-token");
                assert_eq!(body["metadata"]["pluginType"], "GEMINI");
                Json(serde_json::json!({"ineligibleTiers":[{"reasonCode":"UNSUPPORTED_CLIENT"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut account = account(
            "oauth",
            serde_json::json!({"access_token":"test-token","oauth_type":"code_assist","base_url":base}),
        );
        account.platform = "gemini".into();
        assert!(is_code_assist(&account).unwrap());
        let result = check_code_assist_connection(&reqwest::Client::new(), &account).await;
        server.abort();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("UNSUPPORTED_CLIENT"));
        account.credentials["oauth_type"] = Value::String("ai_studio".into());
        assert!(!is_code_assist(&account).unwrap());
    }

    #[test]
    fn code_assist_reachability_does_not_claim_generation_access() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"currentTier":{"id":"standard-tier"}}),
        ] {
            let report = code_assist_connection_report(&value).unwrap();
            assert_eq!(report["outcome"], "warning");
            assert_eq!(report["ok"], false);
            assert_eq!(report["status_code"], 200);
        }
    }

    #[tokio::test]
    async fn upstream_count_and_compact_requests_preserve_model_and_payload() {
        let client = reqwest::Client::new();
        for (kind, credentials, base) in [
            (
                "apikey",
                serde_json::json!({"api_key":"fixture","base_url":"https://supplier.test/v1"}),
                "https://supplier.test/v1",
            ),
            (
                "oauth",
                serde_json::json!({"access_token":"fixture"}),
                "https://chatgpt.com/backend-api/codex",
            ),
        ] {
            for suffix in ["/input_tokens", "/compact"] {
                let payload = serde_json::json!({"model":"gpt-original","input":[{"role":"user","content":[{"type":"input_image","image_url":"https://image.test/photo.png"}]}]});
                let request = build_request(
                    &client,
                    &account(kind, credentials.clone()),
                    Method::POST,
                    &format!("/v1/responses{suffix}"),
                    &HeaderMap::new(),
                    &payload,
                )
                .unwrap()
                .build()
                .unwrap();
                assert_eq!(request.url().as_str(), format!("{base}/responses{suffix}"));
                let sent: Value =
                    serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
                assert_eq!(sent, payload);
            }
        }
    }

    #[test]
    fn responses_subpath_guard_matches_closed_allowlist() {
        assert_eq!(responses_suffix("/v1/responses"), Some(""));
        assert_eq!(responses_suffix("/responses/compact"), Some("/compact"));
        assert_eq!(
            responses_suffix("/backend-api/codex/responses/input_tokens"),
            Some("/input_tokens")
        );
        assert_eq!(responses_suffix("/v1/responses/../admin"), None);
        assert_eq!(responses_suffix("/v1/responses/unknown"), None);
        assert_eq!(responses_suffix("/v1/responses/compact/detail"), None);
    }

    #[test]
    fn retry_policy_covers_auth_limits_and_transient_upstream_failures() {
        for status in [401, 403, 408, 409, 425, 429, 500, 502, 503, 504] {
            assert!(should_failover_status(
                reqwest::StatusCode::from_u16(status).unwrap()
            ));
        }
        assert!(!should_failover_status(reqwest::StatusCode::BAD_REQUEST));
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(
            retry_after_seconds(&headers, reqwest::StatusCode::TOO_MANY_REQUESTS),
            120
        );
    }

    #[test]
    fn sticky_candidate_respects_rate_limit_cooldown() {
        let mut candidate = account("apikey", serde_json::json!({"api_key":"test"}));
        candidate.rate_limit_reset_at = Some(Utc::now() + chrono::Duration::minutes(1));
        assert!(!promotable(&candidate, "openai"));
        candidate.rate_limit_reset_at = Some(Utc::now() - chrono::Duration::minutes(1));
        assert!(promotable(&candidate, "openai"));
    }

    #[test]
    fn model_list_parser_supports_openai_and_gemini_shapes() {
        assert_eq!(
            extract_model_ids(&serde_json::json!({"data":[{"id":"gpt-test"}]})).unwrap(),
            vec!["gpt-test"]
        );
        assert_eq!(
            extract_model_ids(&serde_json::json!({"models":[{"name":"models/gemini-test"}]}))
                .unwrap(),
            vec!["gemini-test"]
        );
        assert_eq!(
            ensure_v1_base("https://cli-chat-proxy.grok.com/v1"),
            "https://cli-chat-proxy.grok.com/v1"
        );
    }

    #[test]
    fn malformed_or_failed_terminal_sse_is_reported_as_bad_gateway() {
        for body in [
            "data: {\"type\":\"response.completed\"}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"upstream failed\"}}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        ] {
            assert!(matches!(
                terminal_response_from_sse(body.as_bytes()),
                Err(AppError::BadGateway(_))
            ));
        }
    }

    #[test]
    fn incomplete_terminal_sse_keeps_partial_response_status() {
        let response = terminal_response_from_sse(
            b"data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"partial\"}]}],\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        ).unwrap();
        assert_eq!(response["status"], "incomplete");
        let chat = response_to_chat_completion(&response, &serde_json::json!({"model":"gpt-test"}));
        assert_eq!(chat["choices"][0]["message"]["content"], "partial");
        assert_eq!(chat["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn response_delta_converts_to_chat_chunk() {
        let chunks = response_event_to_chat_chunks(
            &serde_json::json!({"type":"response.output_text.delta","delta":"hi"}),
            "resp_1",
            "gpt-5.4",
            123,
        );
        assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "hi");
        assert_eq!(
            crate::sub2_sse::frame_end(b"data: x\r\n\r\nrest"),
            Some((7, 4))
        );
    }

    #[test]
    fn credential_values_are_required_and_trimmed() {
        let account = account("apikey", serde_json::json!({"api_key":" secret "}));
        assert_eq!(credential(&account, "api_key").unwrap(), "secret");
        assert!(credential(&account, "access_token").is_err());
    }

    #[test]
    fn oauth_accounts_route_to_their_native_protocols() {
        for (platform, expected) in [
            ("anthropic", EngineProtocol::ClaudeOAuth),
            ("gemini", EngineProtocol::GeminiOAuth),
            ("grok", EngineProtocol::GrokOAuth),
            ("antigravity", EngineProtocol::Antigravity),
        ] {
            let mut value = account("oauth", serde_json::json!({"access_token":"token"}));
            value.platform = platform.into();
            assert_eq!(engine_protocol(&value).unwrap(), expected);
        }
    }

    async fn review_collect_stream(
        stream: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
    ) -> (Vec<Value>, usize) {
        let chunks = stream.collect::<Vec<_>>().await;
        let bytes = chunks
            .into_iter()
            .flat_map(|chunk| chunk.unwrap().to_vec())
            .collect::<Vec<_>>();
        let text = String::from_utf8(bytes).unwrap();
        let done = text.lines().filter(|line| *line == "data: [DONE]").count();
        let frames = text
            .lines()
            .filter_map(|line| {
                let data = line.strip_prefix("data: ")?;
                (data != "[DONE]").then(|| serde_json::from_str(data).unwrap())
            })
            .collect();
        (frames, done)
    }

    fn review_fixture_chunks(
        events: Vec<Value>,
    ) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
        let text = events
            .into_iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect::<String>();
        let chunks = text
            .as_bytes()
            .chunks(3)
            .map(|part| Ok(Bytes::copy_from_slice(part)))
            .collect::<Vec<_>>();
        futures_util::stream::iter(chunks)
    }

    #[tokio::test]
    async fn review_responses_stream_reports_truncation_and_completes_only_once() {
        let input = review_fixture_chunks(vec![
            serde_json::json!({"type":"response.output_text.delta","delta":"partial"}),
        ]);
        let (frames, done) = review_collect_stream(chat_completion_stream_from_chunks(
            input,
            None,
            "test".into(),
            None,
            0,
        ))
        .await;
        assert_eq!(done, 1);
        assert!(frames.last().unwrap().get("error").is_some());
        assert!(!frames.iter().any(|frame| {
            frame
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null())
        }));
        let input = review_fixture_chunks(vec![
            serde_json::json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
            serde_json::json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
            serde_json::json!({"type":"response.failed"}),
        ]);
        let (frames, done) = review_collect_stream(chat_completion_stream_from_chunks(
            input,
            None,
            "test".into(),
            None,
            0,
        ))
        .await;
        assert_eq!(done, 1);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["choices"][0]["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn review_native_chat_bodies_and_http_errors_are_preserved() {
        for protocol in [EngineProtocol::OpenAiCompatible, EngineProtocol::GrokOAuth] {
            assert!(!should_project_responses_to_chat(
                protocol,
                true,
                reqwest::StatusCode::OK
            ));
        }
        assert!(should_project_responses_to_chat(
            EngineProtocol::OpenAiCodex,
            true,
            reqwest::StatusCode::OK
        ));
        assert!(!should_project_responses_to_chat(
            EngineProtocol::OpenAiCodex,
            true,
            reqwest::StatusCode::UNAUTHORIZED
        ));
        for body in [
            "{\"choices\":[{\"message\":{\"content\":\"Answer\"}}],\"usage\":{\"completion_tokens\":3}}",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Answer\"}}]}\n\ndata: [DONE]\n\n",
            "{\"error\":{\"message\":\"invalid key\",\"type\":\"authentication_error\"}}",
        ] {
            let input = futures_util::stream::iter(
                body.as_bytes()
                    .chunks(3)
                    .map(|part| Ok(Bytes::copy_from_slice(part)))
                    .collect::<Vec<_>>(),
            );
            let chunks = passthrough_stream_from_chunks(input, None)
                .collect::<Vec<_>>()
                .await;
            let bytes = chunks
                .into_iter()
                .flat_map(|chunk| chunk.unwrap().to_vec())
                .collect::<Vec<_>>();
            assert_eq!(bytes, body.as_bytes());
        }
    }

    #[tokio::test]
    async fn review_provider_stream_errors_are_not_successful_answers() {
        for (protocol, error) in [
            (
                EngineProtocol::Anthropic,
                serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
            ),
            (
                EngineProtocol::Gemini,
                serde_json::json!({"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}}),
            ),
        ] {
            let expected = error["error"]["message"].clone();
            let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
                review_fixture_chunks(vec![error]),
                None,
                "test".into(),
                protocol,
            ))
            .await;
            assert_eq!(done, 1);
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0]["error"]["message"], expected);
        }
        for protocol in [EngineProtocol::Anthropic, EngineProtocol::Gemini] {
            let input = futures_util::stream::iter(vec![Err(std::io::Error::other(
                "private-transport-detail",
            ))]);
            let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
                input,
                None,
                "test".into(),
                protocol,
            ))
            .await;
            assert_eq!(done, 1);
            assert!(frames[0].get("error").is_some());
            assert!(!frames[0].to_string().contains("private-transport-detail"));
        }
    }

    #[tokio::test]
    async fn review_gemini_parallel_tool_snapshots_keep_identity_and_usage() {
        let input = review_fixture_chunks(vec![
            serde_json::json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"Summary"},{"functionCall":{"id":"a","name":"lookup","args":{}},"thoughtSignature":"opaque"}]}}]}),
            serde_json::json!({"candidates":[{"content":{"parts":[{"text":"Answer"},{"functionCall":{"id":"a","name":"lookup","args":{"city":"A"}}},{"functionCall":{"id":"b","name":"lookup","args":{"city":"B"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":7,"cachedContentTokenCount":4,"totalTokenCount":22}}),
        ]);
        let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
            input,
            None,
            "test".into(),
            EngineProtocol::Gemini,
        ))
        .await;
        assert_eq!(done, 1);
        let calls = frames
            .iter()
            .filter_map(|frame| frame.pointer("/choices/0/delta/tool_calls/0"))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["index"], 0);
        assert_eq!(calls[1]["index"], 1);
        assert_eq!(calls[0]["id"], "a");
        assert_eq!(calls[1]["id"], "b");
        assert_eq!(calls[0]["function"]["arguments"], "{\"city\":\"A\"}");
        assert_eq!(
            calls[0]["extra_content"]["google"]["thought_signature"],
            "opaque"
        );
        assert!(frames
            .iter()
            .any(|frame| frame.pointer("/choices/0/delta/reasoning_content")
                == Some(&serde_json::json!("Summary"))));
        let text = frames
            .iter()
            .filter_map(|frame| {
                frame
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
            })
            .collect::<String>();
        assert_eq!(text, "Answer");
        let terminal = frames.last().unwrap();
        assert_eq!(terminal["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(terminal["usage"]["completion_tokens"], 12);
        assert_eq!(
            terminal["usage"]["completion_tokens_details"]["reasoning_tokens"],
            7
        );
        assert_eq!(
            terminal["usage"]["prompt_tokens_details"]["cached_tokens"],
            4
        );
    }

    #[tokio::test]
    async fn review_native_truncated_streams_have_explicit_errors() {
        for (protocol, event) in [
            (
                EngineProtocol::Anthropic,
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}}),
            ),
            (
                EngineProtocol::Gemini,
                serde_json::json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}),
            ),
        ] {
            let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
                review_fixture_chunks(vec![event]),
                None,
                "test".into(),
                protocol,
            ))
            .await;
            assert_eq!(done, 1);
            assert!(frames.last().unwrap().get("error").is_some());
        }
    }

    #[tokio::test]
    async fn anthropic_stream_usage_preserves_cache_and_latest_cumulative_counts() {
        let input = review_fixture_chunks(vec![
            serde_json::json!({"type":"message_start","message":{"id":"msg_usage","usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":1}}}),
            serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":12,"cache_read_input_tokens":110,"output_tokens":8}}),
            serde_json::json!({"type":"message_stop"}),
        ]);
        let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
            input,
            None,
            "test".into(),
            EngineProtocol::Anthropic,
        ))
        .await;
        assert_eq!(done, 1);
        assert_eq!(
            frames.last().unwrap()["usage"],
            anthropic_chat_usage(&serde_json::json!({
                "input_tokens":12,"cache_read_input_tokens":110,"cache_creation_input_tokens":20,"output_tokens":8
            }))
        );
        assert_eq!(frames.last().unwrap()["usage"]["prompt_tokens"], 142);
        assert_eq!(frames.last().unwrap()["usage"]["total_tokens"], 150);
    }

    #[tokio::test]
    async fn review_anthropic_no_argument_tool_call_has_valid_json() {
        let input = review_fixture_chunks(vec![
            serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"a","name":"lookup","input":{}}}),
            serde_json::json!({"type":"content_block_stop","index":1}),
            serde_json::json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
            serde_json::json!({"type":"message_stop"}),
        ]);
        let (frames, done) = review_collect_stream(protocol_chat_stream_from_chunks(
            input,
            None,
            "test".into(),
            EngineProtocol::Anthropic,
        ))
        .await;
        assert_eq!(done, 1);
        let arguments = frames
            .iter()
            .filter_map(|frame| {
                frame
                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .and_then(Value::as_str)
            })
            .collect::<String>();
        assert_eq!(arguments, "{}");
        assert_eq!(
            frames.last().unwrap()["choices"][0]["finish_reason"],
            "tool_calls"
        );
    }

    #[test]
    fn session_seed_prefers_client_session_identity() {
        assert_eq!(
            session_seed(&serde_json::json!({"session_id":" s1 "})),
            Some("s1".into())
        );
        assert_eq!(
            session_seed(&serde_json::json!({"prompt_cache_key":"pk"})),
            Some("pk".into())
        );
        assert_eq!(session_seed(&serde_json::json!({"model":"gpt"})), None);
    }

    #[test]
    fn codex_sse_with_empty_completed_output_assembles_items() {
        let sse = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"GO-BASELINE-OK\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_x\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":16,\"output_tokens\":10}}}\n\n"
        );
        let response = terminal_response_from_sse(sse.as_bytes()).unwrap();
        let chat = response_to_chat_completion(&response, &serde_json::json!({"model":"gpt-5.6"}));
        assert_eq!(chat["choices"][0]["message"]["content"], "GO-BASELINE-OK");
        assert_eq!(chat["usage"]["total_tokens"], 26);
    }
}
