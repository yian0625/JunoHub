use std::pin::Pin;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, Response},
};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::Value;

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
        sticky_account_id = sticky.session_account().await?;
    }
    let request_id = format!("junohub-gateway-{}", uuid::Uuid::new_v4());
    let mut selected = None;
    for candidate_id in [previous_response_account_id, sticky_account_id]
        .into_iter()
        .flatten()
    {
        if let Some(account) = repository.account_by_id(candidate_id).await? {
            if promotable(&account, &route.platform)
                && acquire_slot(&cache, &account, &request_id).await?
            {
                selected = Some(account);
                break;
            }
        }
    }
    if selected.is_none() {
        if let Some(ranked) = snapshot_candidate_ids(
            &cache,
            &route,
            previous_response_account_id,
            sticky_account_id,
        )
        .await?
        {
            for candidate_id in ranked {
                let Some(account) = repository.account_by_id(candidate_id).await? else {
                    continue;
                };
                if acquire_slot(&cache, &account, &request_id).await? {
                    selected = Some(account);
                    break;
                }
            }
        }
    }
    if selected.is_none() {
        let account_ids = repository
            .schedulable_account_ids(route.group_id, &route.platform, false)
            .await?;
        if account_ids.is_empty() {
            return Err(AppError::Internal("账户池当前没有可调度账户".into()));
        }
        for account_id in account_ids {
            let Some(account) = repository.account_by_id(account_id).await? else {
                continue;
            };
            if acquire_slot(&cache, &account, &request_id).await? {
                selected = Some(account);
                break;
            }
        }
    }
    let mut account = selected.ok_or_else(|| AppError::Internal("账户池并发已满".into()))?;
    mark_account_used(state, &cache, account.id);
    if let Err(error) = refresh_openai_if_needed(state, &repository, &mut account).await {
        let _ = cache.release_account_slot(account.id, &request_id).await;
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
        let _ = cache.release_account_slot(account.id, &request_id).await;
        return Err(AppError::BadRequest(format!(
            "该平台账户仅支持 Chat Completions 协议: {}/{}",
            account.platform, account.kind
        )));
    }
    let guard = LeaseGuard {
        cache: Some(cache),
        account_id: account.id,
        request_id: request_id.clone(),
    };
    if responses_suffix(path).as_deref() == Some("/input_tokens") {
        let input_tokens = estimate_input_tokens(payload)?;
        drop(guard);
        let body = serde_json::to_vec(&serde_json::json!({
            "object":"response.input_tokens",
            "input_tokens":input_tokens
        }))?;
        return Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("x-junohub-engine-account-id", account.id)
            .body(Body::from(body))
            .map_err(|error| AppError::Internal(error.to_string()));
    }
    let upstream_request = match build_request(state, &account, method.clone(), path, headers, payload) {
        Ok(request) => request,
        Err(error) => {
            drop(guard);
            return Err(error);
        }
    };
    let client_stream = payload.get("stream").and_then(Value::as_bool) == Some(true);
    let client_chat = matches!(path, "/v1/chat/completions" | "/chat/completions");
    let responses_unary = responses_suffix(path).is_some_and(|suffix| !suffix.is_empty());
    let mut upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(error) => {
            drop(guard);
            return Err(AppError::Http(error));
        }
    };
    if protocol == EngineProtocol::OpenAiCodex && upstream.status() == reqwest::StatusCode::BAD_REQUEST
    {
        let error_headers = upstream.headers().clone();
        let error_status = upstream.status();
        let error_body = upstream.bytes().await.map_err(AppError::Http)?;
        match codex_model_fallback(&error_body, payload) {
            Some(mapped_model) => {
                let mut retry_payload = payload.clone();
                retry_payload["model"] = Value::String(mapped_model);
                let retry_request =
                    build_request(state, &account, method, path, headers, &retry_payload);
                match retry_request {
                    Ok(request) => match request.send().await {
                        Ok(response) => upstream = response,
                        Err(error) => {
                            drop(guard);
                            return Err(AppError::Http(error));
                        }
                    },
                    Err(_) => {
                        drop(guard);
                        return Err(AppError::Internal("Codex 别名重试构建失败".into()));
                    }
                }
            }
            None => {
                drop(guard);
                let mut response = Response::new(Body::from(error_body));
                *response.status_mut() = error_status;
                for (name, value) in error_headers {
                    if let Some(name) = name {
                        if !matches!(name.as_str(), "content-length" | "connection" | "transfer-encoding") {
                            response.headers_mut().insert(name, value);
                        }
                    }
                }
                response.headers_mut().insert(
                    "x-junohub-engine-account-id",
                    account.id.to_string().parse().unwrap(),
                );
                return Ok(response);
            }
        }
    }
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    if !status.is_success()
        && matches!(protocol, EngineProtocol::Anthropic | EngineProtocol::Gemini)
    {
        let body = upstream.bytes().await;
        drop(guard);
        let error_body = body
            .ok()
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .map(protocol_error_to_openai)
            .unwrap_or_else(|| {
                serde_json::json!({"error":{"message":"上游请求失败","type":"api_error"}})
            });
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
        let body = upstream.bytes().await?;
        let response_value = terminal_response_from_sse(&body)?;
        bind_sticky_artifacts(sticky.as_ref(), &response_value, account.id).await;
        let response_value = if client_chat {
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
    if matches!(protocol, EngineProtocol::Anthropic | EngineProtocol::Gemini) && client_chat {
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
        let body = upstream
            .bytes()
            .await
                       .map_err(AppError::Http)?;
        let parsed: Value = serde_json::from_slice(&body)
            .map_err(|_| AppError::Internal("上游返回无效 JSON".into()))?;
        let chat = if protocol == EngineProtocol::Anthropic {
            anthropic_to_chat_completion(&parsed, payload)
        } else {
            gemini_to_chat_completion(&parsed, payload)
        };
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
    let stream: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> = if client_chat {
        chat_completion_sse_stream(upstream, stream_guard, payload, sticky, account.id)
    } else {
        sticky_binding_stream(upstream, stream_guard, sticky.as_ref(), account.id)
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

async fn refresh_openai_if_needed(
    state: &AppState,
    repository: &Sub2Repository,
    account: &mut EngineAccount,
) -> AppResult<()> {
    if account.platform != "openai" || account.kind != "oauth" {
        return Ok(());
    }
    if credentials_expire_at(account)
        .is_some_and(|expires| expires > chrono::Utc::now().timestamp() + 60)
    {
        return Ok(());
    }
    let old_refresh_token = credential(account, "refresh_token")?;
    let client_id = account.credentials.get("client_id").and_then(Value::as_str);
    let token =
        crate::oauth::refresh_openai_token(&state.client, &old_refresh_token, client_id).await?;
    let expires_at = chrono::Utc::now()
        .timestamp()
        .saturating_add(token.expires_in);
    let mut patch = serde_json::json!({
        "access_token":token.access_token,
        "refresh_token":token.refresh_token,
        "token_type":token.token_type,
        "expires_at":expires_at,
        "client_id":token.client_id
    });
    if !token.id_token.trim().is_empty() {
        patch["id_token"] = Value::String(token.id_token);
    }
    if repository
        .update_openai_tokens_cas(account.id, &old_refresh_token, &patch)
        .await?
    {
        merge_json_object(&mut account.credentials, &patch);
    } else if let Some(fresh) = repository.account_by_id(account.id).await? {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineProtocol {
    OpenAiCodex,
    OpenAiCompatible,
    Anthropic,
    Gemini,
}

fn engine_protocol(account: &EngineAccount) -> AppResult<EngineProtocol> {
    match (account.kind.as_str(), account.platform.as_str()) {
        ("oauth", "openai") => Ok(EngineProtocol::OpenAiCodex),
        ("apikey", "anthropic") => Ok(EngineProtocol::Anthropic),
        ("apikey", "gemini") => Ok(EngineProtocol::Gemini),
        ("apikey", _) => Ok(EngineProtocol::OpenAiCompatible),
        (kind, platform) => Err(AppError::BadRequest(format!(
            "账户类型尚未迁移到 Rust Gateway: {platform}/{kind}"
        ))),
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
        && account
            .expires_at
            .is_none_or(|until| !(account.auto_pause_on_expired && now >= until))
}

async fn acquire_slot(
    cache: &SchedulerCache,
    account: &EngineAccount,
    request_id: &str,
) -> AppResult<bool> {
    let concurrency = account
        .load_factor
        .filter(|value| *value > 0)
        .unwrap_or_else(|| account.concurrency.max(1));
    cache
        .acquire_account_slot(account.id, concurrency, request_id)
        .await
}

async fn snapshot_candidate_ids(
    cache: &SchedulerCache,
    route: &EngineKeyRoute,
    previous_response_account_id: Option<i64>,
    sticky_account_id: Option<i64>,
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
            bucket_key: format!("sched:{}:{}:{}:v{}", route.group_id, route.platform, mode, version),
            platform: route.platform.clone(),
            excluded_ids: Vec::new(),
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

async fn bind_sticky_artifacts(
    sticky: Option<&EngineSticky>,
    response: &Value,
    account_id: i64,
) {
    let Some(sticky) = sticky else { return };
    if let Some(response_id) = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        sticky.bind_response_and_session(response_id, account_id).await;
    }
}

fn protocol_error_to_openai(body: Value) -> Value {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("上游请求失败");
    let kind = body
        .pointer("/error/type")
        .or_else(|| body.pointer("/error/status"))
        .and_then(Value::as_str)
        .unwrap_or("api_error");
    serde_json::json!({"error":{"message":message,"type":kind}})
}

enum Auth {
    Bearer(String),
    ApiKey(String),
    Google(String),
}

fn build_request(
    state: &AppState,
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
    }
    let mut request = state.client.request(method, url).json(&outbound_payload);
    request = match auth {
        Auth::Bearer(token) => request.bearer_auth(token),
        Auth::ApiKey(key) => request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        Auth::Google(key) => request.header("x-goog-api-key", key),
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
    Ok(request)
}

fn estimate_input_tokens(payload: &Value) -> AppResult<usize> {
    let codec = tiktoken_rs::o200k_base()
        .map_err(|error| AppError::Internal(format!("加载 tokenizer 失败: {error}")))?;
    let mut total = 0;
    if let Some(instructions) = payload.get("instructions").and_then(Value::as_str) {
        total += codec.encode_with_special_tokens(instructions.trim()).len();
    }
    if let Some(input) = payload.get("input") {
        match input {
            Value::String(text) => total += codec.encode_with_special_tokens(text.trim()).len(),
            Value::Array(items) => {
                for item in items {
                    total += estimate_input_item_tokens(&codec, item);
                }
            }
            value => {
                total += codec
                    .encode_with_special_tokens(&serde_json::to_string(value)?)
                    .len()
            }
        }
    }
    for field in ["tools", "tool_choice"] {
        if let Some(value) = payload.get(field) {
            total += codec
                .encode_with_special_tokens(&serde_json::to_string(value)?)
                .len();
        }
    }
    Ok(total.max(1))
}

fn estimate_input_item_tokens(codec: &tiktoken_rs::CoreBPE, item: &Value) -> usize {
    let mut total = 3;
    let count = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| codec.encode_with_special_tokens(value).len())
            .unwrap_or(0)
    };
    total += count(item.get("role"));
    if item.get("type").and_then(Value::as_str) != Some("message") {
        total += count(item.get("type"));
    }
    for key in ["name", "arguments", "output", "call_id", "id"] {
        total += count(item.get(key));
    }
    match item.get("content") {
        Some(Value::String(text)) => total += codec.encode_with_special_tokens(text.trim()).len(),
        Some(Value::Array(parts)) => {
            for part in parts {
                total += 1;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => {
                        total += count(part.get("text"));
                    }
                    Some("input_image") => {
                        if let Some(url) = part.get("image_url").and_then(Value::as_str) {
                            let trimmed = url.trim();
                            let estimated = if trimmed.to_ascii_lowercase().starts_with("data:") {
                                trimmed
                                    .split_once(',')
                                    .map(|(prefix, _)| prefix)
                                    .unwrap_or(trimmed)
                            } else {
                                trimmed
                            };
                            total += codec.encode_with_special_tokens(estimated).len();
                        }
                    }
                    _ => total += count(part.get("type")),
                }
            }
        }
        Some(value) => {
            if let Ok(compact) = serde_json::to_string(value) {
                total += codec.encode_with_special_tokens(&compact).len();
            }
        }
        None => {}
    }
    total
}

fn responses_suffix(path: &str) -> Option<String> {
    let suffix = path
        .strip_prefix("/v1/responses")
        .or_else(|| path.strip_prefix("/responses"))
        .or_else(|| path.strip_prefix("/backend-api/codex/responses"))?;
    if suffix.is_empty() {
        return Some(String::new());
    }
    let segments = suffix.strip_prefix('/')?.split('/').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.len() > 8
        || !matches!(segments[0], "compact" | "input_tokens")
        || segments.iter().any(|segment| {
            segment.is_empty()
                || segment.len() > 128
                || segment.chars().all(|character| character == '.')
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return None;
    }
    Some(format!("/{}", segments.join("/")))
}

fn chat_completion_sse_stream(
    upstream: reqwest::Response,
    guard: LeaseGuard,
    request: &Value,
    sticky: Option<EngineSticky>,
    account_id: i64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Box::pin(async_stream::try_stream! {
        let _guard = guard;
        let mut upstream = upstream.bytes_stream();
        let mut buffer = Vec::new();
        let mut response_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
        let created = chrono::Utc::now().timestamp();
        while let Some(chunk) = upstream.next().await {
            buffer.extend_from_slice(&chunk.map_err(std::io::Error::other)?);
            while let Some((end, delimiter_len)) = find_sse_frame_end(&buffer) {
                let frame = buffer.drain(..end).collect::<Vec<_>>();
                buffer.drain(..delimiter_len);
                let text = String::from_utf8_lossy(&frame);
                for line in text.lines() {
                    let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else { continue; };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" { continue; }
                    let Ok(event) = serde_json::from_str::<Value>(data) else { continue; };
                    if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                        response_id = id.to_owned();
                    }
                    if event.get("type").and_then(Value::as_str) == Some("response.completed") {
                        if let Some(response) = event.get("response") {
                            bind_sticky_artifacts(sticky.as_ref(), response, account_id).await;
                        }
                    }
                    for output in response_event_to_chat_chunks(&event, &response_id, &model, created) {
                        yield Bytes::from(format!("data: {}\n\n", serde_json::to_string(&output).unwrap_or_default()));
                    }
                    if event.get("type").and_then(Value::as_str) == Some("response.completed") {
                        yield Bytes::from_static(b"data: [DONE]\n\n");
                    }
                }
            }
        }
    })
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
            buffer.extend_from_slice(&chunk);
            while let Some((end, delimiter_len)) = find_sse_frame_end(&buffer) {
                let frame = buffer.drain(..end + delimiter_len).collect::<Vec<_>>();
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
        }
        if !buffer.is_empty() {
            if !bound {
                if let Some(response_id) = extract_completed_response_id(&buffer) {
                    if let Some(sticky) = sticky.as_ref() {
                        sticky.bind_response_and_session(&response_id, account_id).await;
                    }
                }
            }
            yield Bytes::from(buffer);
        }
    })
}

fn extract_completed_response_id(frame: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(frame).ok()?;
    for line in text.lines() {
        let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) == Some("response.completed") {
            if let Some(id) = event
                .pointer("/response/id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn find_sse_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

fn response_event_to_chat_chunks(
    event: &Value,
    response_id: &str,
    model: &str,
    created: i64,
) -> Vec<Value> {
    let base = |delta: Value, finish_reason: Value, usage: Value| {
        serde_json::json!({
            "id":response_id,
            "object":"chat.completion.chunk",
            "created":created,
            "model":model,
            "choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}],
            "usage":usage
        })
    };
    match event.get("type").and_then(Value::as_str) {
        Some("response.created") => vec![base(
            serde_json::json!({"role":"assistant"}),
            Value::Null,
            Value::Null,
        )],
        Some("response.output_text.delta") => vec![base(
            serde_json::json!({"content":event.get("delta").cloned().unwrap_or(Value::String(String::new()))}),
            Value::Null,
            Value::Null,
        )],
        Some("response.output_item.added")
            if event.pointer("/item/type").and_then(Value::as_str) == Some("function_call") =>
        {
            vec![base(
                serde_json::json!({"tool_calls":[{
                    "index":event.get("output_index").and_then(Value::as_i64).unwrap_or(0),
                    "id":event.pointer("/item/call_id").cloned().unwrap_or(Value::Null),
                    "type":"function",
                    "function":{"name":event.pointer("/item/name").cloned().unwrap_or(Value::Null),"arguments":""}
                }]}),
                Value::Null,
                Value::Null,
            )]
        }
        Some("response.function_call_arguments.delta") => vec![base(
            serde_json::json!({"tool_calls":[{
                "index":event.get("output_index").and_then(Value::as_i64).unwrap_or(0),
                "function":{"arguments":event.get("delta").cloned().unwrap_or(Value::String(String::new()))}
            }]}),
            Value::Null,
            Value::Null,
        )],
        Some("response.completed") => {
            let response = event.get("response").unwrap_or(&Value::Null);
            let input = response
                .pointer("/usage/input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let output = response
                .pointer("/usage/output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let has_tools = response
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call")
                    })
                });
            vec![base(
                serde_json::json!({}),
                Value::String(if has_tools { "tool_calls" } else { "stop" }.into()),
                serde_json::json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input+output}),
            )]
        }
        _ => vec![],
    }
}

fn chat_to_responses(payload: &Value) -> AppResult<Value> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if matches!(role, "system" | "developer") {
            if let Some(text) = content_as_text(&content) {
                instructions.push(text);
            }
            continue;
        }
        if role == "tool" {
            input.push(serde_json::json!({
                "type":"function_call_output",
                "call_id":message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default(),
                "output":content_as_text(&content).unwrap_or_default()
            }));
            continue;
        }
        input.push(serde_json::json!({"type":"message","role":role,"content":content}));
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                input.push(serde_json::json!({
                    "type":"function_call",
                    "call_id":call.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name":call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                    "arguments":call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}")
                }));
            }
        }
    }
    let tools = payload
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let function = tool.get("function")?;
                    Some(serde_json::json!({
                        "type":"function",
                        "name":function.get("name")?,
                        "description":function.get("description").cloned().unwrap_or(Value::Null),
                        "parameters":function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({}))
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut response = serde_json::json!({
        "model":payload.get("model").cloned().unwrap_or(Value::Null),
        "instructions":instructions.join("\n\n"),
        "input":input,
        "stream":true,
        "store":payload.get("store").and_then(Value::as_bool).unwrap_or(false)
    });
    if !tools.is_empty() {
        response["tools"] = Value::Array(tools);
    }
    // Codex(ChatGPT OAuth) 后端不接受 temperature / max_output_tokens 等参数，
    // 与 Go 网关保持一致：转换时直接丢弃这些客户端字段。
    Ok(response)
}

fn content_as_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    content.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn response_to_chat_completion(response: &Value, request: &Value) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
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
                }
            }
            Some("function_call") => tool_calls.push(serde_json::json!({
                "id":item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or(Value::Null),
                "type":"function",
                "function":{
                    "name":item.get("name").cloned().unwrap_or(Value::Null),
                    "arguments":item.get("arguments").cloned().unwrap_or_else(|| Value::String("{}".into()))
                }
            })),
            _ => {}
        }
    }
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let input_tokens = response
        .pointer("/usage/input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = response
        .pointer("/usage/output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    serde_json::json!({
        "id":response.get("id").cloned().unwrap_or_else(|| Value::String(format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()))),
        "object":"chat.completion",
        "created":response.get("created_at").cloned().unwrap_or_else(|| Value::from(chrono::Utc::now().timestamp())),
        "model":request.get("model").cloned().unwrap_or(Value::Null),
        "choices":[{"index":0,"message":message,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"}}],
        "usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens+output_tokens}
    })
}

fn anthropic_messages_url(base_url: &str) -> AppResult<String> {
    let trimmed = base_url.trim_end_matches('/');
    let base = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    Ok(format!("{base}/v1/messages"))
}

fn gemini_generate_url(base_url: &str, model: &str, stream: bool) -> AppResult<String> {
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

fn anthropic_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    content.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn anthropic_content_blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![serde_json::json!({"type":"text","text":text})]
            }
        }
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => Some(serde_json::json!({
                    "type":"text",
                    "text":part.get("text").and_then(Value::as_str).unwrap_or_default()
                })),
                Some("image_url") => {
                    let url = part
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    if let Some((metadata, data)) = url.split_once(',') {
                        if metadata.starts_with("data:") {
                            let mime = metadata
                                .strip_prefix("data:")
                                .and_then(|value| value.split_once(';').map(|(mime, _)| mime.to_owned()))
                                .unwrap_or_else(|| "image/png".into());
                            return Some(serde_json::json!({
                                "type":"image",
                                "source":{"type":"base64","media_type":mime,"data":data}
                            }));
                        }
                    }
                    Some(serde_json::json!({
                        "type":"image",
                        "source":{"type":"url","url":url}
                    }))
                }
                _ => {
                    let encoded = serde_json::to_string(part).unwrap_or_default();
                    if encoded.is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({"type":"text","text":encoded}))
                    }
                }
            })
            .collect(),
        Value::Null => Vec::new(),
        value => {
            vec![serde_json::json!({
                "type":"text",
                "text":serde_json::to_string(value).unwrap_or_default()
            })]
        }
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

fn chat_to_anthropic(payload: &Value) -> AppResult<Value> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err(AppError::BadRequest("model 不能为空".into()));
    }
    let mut system = Vec::new();
    let mut converted: Vec<Value> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "system" | "developer" => {
                if let Some(text) = anthropic_text(&content) {
                    if !text.trim().is_empty() {
                        system.push(text);
                    }
                }
            }
            "tool" => {
                append_protocol_message(
                    &mut converted,
                    "user",
                    serde_json::json!({
                        "type":"tool_result",
                        "tool_use_id":message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default(),
                        "content":[{"type":"text","text":anthropic_text(&content).unwrap_or_default()}]
                    }),
                );
            }
            "assistant" => {
                if let Some(text) = anthropic_text(&content).filter(|text| !text.trim().is_empty()) {
                    append_protocol_message(
                        &mut converted,
                        "assistant",
                        serde_json::json!({"type":"text","text":text}),
                    );
                }
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let arguments = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    append_protocol_message(
                        &mut converted,
                        "assistant",
                        serde_json::json!({
                            "type":"tool_use",
                            "id":call.get("id").and_then(Value::as_str).unwrap_or_default(),
                            "name":call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                            "input":serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| serde_json::json!({}))
                        }),
                    );
                }
            }
            _ => {
                for block in anthropic_content_blocks(&content) {
                    append_protocol_message(&mut converted, "user", block);
                }
            }
        }
    }
    if converted.is_empty() {
        return Err(AppError::BadRequest("messages 不能为空".into()));
    }
    let max_tokens = payload
        .get("max_tokens")
        .or_else(|| payload.get("max_completion_tokens"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(8192);
    let mut body = serde_json::json!({
        "model":model,
        "max_tokens":max_tokens,
        "messages":converted
    });
    if !system.is_empty() {
        body["system"] = Value::String(system.join("\n\n"));
    }
    if let Some(stream) = payload.get("stream").and_then(Value::as_bool) {
        body["stream"] = Value::Bool(stream);
    }
    for field in ["temperature", "top_p", "stop_sequences"] {
        if let Some(value) = payload.get(field) {
            body[field] = value.clone();
        }
    }
    if let Some(stop) = payload.get("stop").cloned() {
        body["stop_sequences"] = match stop {
            Value::String(text) => serde_json::json!([text]),
            Value::Array(items) => Value::Array(items),
            _ => Value::Null,
        };
    }
    if let Some(tools) = payload
        .get("tools")
        .and_then(Value::as_array)
        .filter(|tools| !tools.is_empty())
    {
        let converted_tools: Vec<Value> = tools
            .iter()
            .filter_map(|tool| {
                let function = tool.get("function")?;
                let name = function.get("name")?.as_str()?;
                Some(serde_json::json!({
                    "name":name,
                    "description":function.get("description").cloned().unwrap_or(Value::Null),
                    "input_schema":function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({"type":"object"}))
                }))
            })
            .collect();
        if !converted_tools.is_empty() {
            body["tools"] = Value::Array(converted_tools);
            match payload.get("tool_choice") {
                Some(Value::String(mode)) if mode == "auto" => {
                    body["tool_choice"] = serde_json::json!({"type":"auto"});
                }
                Some(Value::String(mode)) if mode == "required" => {
                    body["tool_choice"] = serde_json::json!({"type":"any"});
                }
                Some(choice) if choice.is_object() => {
                    if let Some(name) = choice.pointer("/function/name").and_then(Value::as_str) {
                        body["tool_choice"] = serde_json::json!({"type":"tool","name":name});
                    }
                }
                _ => {}
            }
        }
    }
    Ok(body)
}

fn anthropic_finish_reason(stop_reason: Option<&str>) -> &'static str {
    match stop_reason {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

fn anthropic_to_chat_completion(body: &Value, request: &Value) -> Value {
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
                "id":block.get("id").cloned().unwrap_or(Value::Null),
                "type":"function",
                "function":{
                    "name":block.get("name").cloned().unwrap_or(Value::Null),
                    "arguments":serde_json::to_string(block.get("input").unwrap_or(&serde_json::json!({}))).unwrap_or_else(|_| "{}".into())
                }
            })),
            _ => {}
        }
    }
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let input_tokens = body.pointer("/usage/input_tokens").and_then(Value::as_i64).unwrap_or(0);
    let output_tokens = body
        .pointer("/usage/output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
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
        "model":request.get("model").cloned().unwrap_or(Value::Null),
        "choices":[{"index":0,"message":message,"finish_reason":if message.get("tool_calls").is_some() && finish == "stop" {"tool_calls"} else {finish}}],
        "usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens+output_tokens}
    })
}

fn chat_to_gemini(payload: &Value) -> AppResult<(Value, bool)> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("messages 必须是数组".into()))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err(AppError::BadRequest("model 不能为空".into()));
    }
    let mut system = Vec::new();
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
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "system" | "developer" => {
                if let Some(text) = anthropic_text(&content) {
                    if !text.trim().is_empty() {
                        system.push(text);
                    }
                }
            }
            "tool" => {
                let output = anthropic_text(&content).unwrap_or_default();
                append(
                    "user",
                    serde_json::json!({
                        "functionResponse":{
                            "name":message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default(),
                            "response":{"result":output}
                        }
                    }),
                );
            }
            "assistant" => {
                if let Some(text) = anthropic_text(&content).filter(|text| !text.trim().is_empty()) {
                    append("model", serde_json::json!({"text":text}));
                }
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let arguments = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    append(
                        "model",
                        serde_json::json!({
                            "functionCall":{
                                "name":call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default(),
                                "args":serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| serde_json::json!({}))
                            }
                        }),
                    );
                }
            }
            _ => {
                match &content {
                    Value::String(text) => {
                        if !text.trim().is_empty() {
                            append("user", serde_json::json!({"text":text}));
                        }
                    }
                    Value::Array(parts) => {
                        for part in parts {
                            match part.get("type").and_then(Value::as_str) {
                                Some("text") => append(
                                    "user",
                                    serde_json::json!({"text":part.get("text").and_then(Value::as_str).unwrap_or_default()}),
                                ),
                                Some("image_url") => {
                                    if let Some(url) = part
                                        .pointer("/image_url/url")
                                        .and_then(Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| !value.is_empty())
                                    {
                                        if let Some((metadata, data)) = url.split_once(',') {
                                            if metadata.starts_with("data:") {
                                                let mime = metadata
                                                    .strip_prefix("data:")
                                                    .and_then(|value| value.split_once(';').map(|(mime, _)| mime.to_owned()))
                                                    .unwrap_or_else(|| "image/png".into());
                                                append(
                                                    "user",
                                                    serde_json::json!({"inlineData":{"mimeType":mime,"data":data}}),
                                                );
                                                continue;
                                            }
                                        }
                                        return Err(AppError::BadRequest(
                                            "Gemini 仅支持 Base64 数据 URL 图片".into(),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Value::Null => {}
                    value => append(
                        "user",
                        serde_json::json!({"text":serde_json::to_string(value).unwrap_or_default()}),
                    ),
                }
            }
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
    if let Some(value) = payload
        .get("max_tokens")
        .or_else(|| payload.get("max_completion_tokens"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
    {
        generation.insert("maxOutputTokens".into(), Value::from(value));
    }
    if !generation.is_empty() {
        body["generationConfig"] = Value::Object(generation);
    }
    if let Some(tools) = payload
        .get("tools")
        .and_then(Value::as_array)
        .filter(|tools| !tools.is_empty())
    {
        let declarations: Vec<Value> = tools
            .iter()
            .filter_map(|tool| {
                let function = tool.get("function")?;
                Some(serde_json::json!({
                    "name":function.get("name").cloned().unwrap_or(Value::Null),
                    "description":function.get("description").cloned().unwrap_or(Value::Null),
                    "parameters":function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({"type":"object"}))
                }))
            })
            .collect();
        if !declarations.is_empty() {
            body["tools"] = serde_json::json!([{"functionDeclarations":declarations}]);
        }
    }
    Ok((body, stream))
}

fn gemini_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("MAX_TOKENS") => "length",
        Some("SAFETY") | Some("RECITATION") | Some("BLOCKLIST") | Some("PROHIBITED_CONTENT") => {
            "content_filter"
        }
        _ => "stop",
    }
}

fn gemini_to_chat_completion(body: &Value, request: &Value) -> Value {
    let candidate = body
        .pointer("/candidates/0")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for part in candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
        if let Some(call) = part.get("functionCall") {
            tool_calls.push(serde_json::json!({
                "id":format!("call_{}",uuid::Uuid::new_v4().simple()),
                "type":"function",
                "function":{
                    "name":call.get("name").cloned().unwrap_or(Value::Null),
                    "arguments":serde_json::to_string(call.get("args").unwrap_or(&serde_json::json!({}))).unwrap_or_else(|_| "{}".into())
                }
            }));
        }
    }
    let mut message = serde_json::json!({"role":"assistant","content":text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish = gemini_finish_reason(
        candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    );
    let input_tokens = body
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = body
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    serde_json::json!({
        "id":format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),
        "object":"chat.completion",
        "created":chrono::Utc::now().timestamp(),
        "model":request.get("model").cloned().unwrap_or(Value::Null),
        "choices":[{"index":0,"message":message,"finish_reason":if message.get("tool_calls").is_some() && finish == "stop" {"tool_calls"} else {finish}}],
        "usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens+output_tokens}
    })
}

fn chat_chunk(
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

fn protocol_sse_to_chat_stream(
    upstream: reqwest::Response,
    guard: LeaseGuard,
    request: &Value,
    protocol: EngineProtocol,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Box::pin(async_stream::try_stream! {
        let _guard = guard;
        let mut upstream = upstream.bytes_stream();
        let mut buffer = Vec::new();
        let mut chat_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
        let created = chrono::Utc::now().timestamp();
        let mut finish: Option<&'static str> = None;
        let mut input_tokens = 0i64;
        let mut output_tokens = 0i64;
        let mut opened = false;
        let mut tool_calls_seen = false;
        while let Some(chunk) = upstream.next().await {
            buffer.extend_from_slice(&chunk.map_err(std::io::Error::other)?);
            while let Some((end, delimiter_len)) = find_sse_frame_end(&buffer) {
                let frame = buffer.drain(..end).collect::<Vec<_>>();
                buffer.drain(..delimiter_len);
                let text = String::from_utf8_lossy(&frame);
                for line in text.lines() {
                    let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else { continue; };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" { continue; }
                    let Ok(event) = serde_json::from_str::<Value>(data) else { continue; };
                    match protocol {
                        EngineProtocol::Anthropic => {
                            match event.get("type").and_then(Value::as_str) {
                                Some("message_start") => {
                                    if let Some(id) = event.pointer("/message/id").and_then(Value::as_str) {
                                        chat_id = id.to_owned();
                                    }
                                    input_tokens = event.pointer("/message/usage/input_tokens").and_then(Value::as_i64).unwrap_or(0);
                                    if !opened {
                                        opened = true;
                                        yield chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({"role":"assistant"}), Value::Null, Value::Null));
                                    }
                                }
                                Some("content_block_start") => {
                                    if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                                        tool_calls_seen = true;
                                        let delta = serde_json::json!({
                                            "tool_calls":[{
                                                "index":event.get("index").and_then(Value::as_i64).unwrap_or(0),
                                                "id":event.pointer("/content_block/id").cloned().unwrap_or(Value::Null),
                                                "type":"function",
                                                "function":{"name":event.pointer("/content_block/name").cloned().unwrap_or(Value::Null),"arguments":""}
                                            }]
                                        });
                                        yield chat_data_chunk(chat_chunk(&chat_id, &model, created, delta, Value::Null, Value::Null));
                                    }
                                }
                                Some("content_block_delta") => {
                                    let delta_type = event.pointer("/delta/type").and_then(Value::as_str).unwrap_or_default();
                                    let chunk = if delta_type == "text_delta" {
                                        chat_chunk(&chat_id, &model, created, serde_json::json!({
                                            "content":event.pointer("/delta/text").cloned().unwrap_or(Value::String(String::new()))
                                        }), Value::Null, Value::Null)
                                    } else if delta_type == "input_json_delta" {
                                        chat_chunk(&chat_id, &model, created, serde_json::json!({
                                            "tool_calls":[{
                                                "index":event.get("index").and_then(Value::as_i64).unwrap_or(0),
                                                "function":{"arguments":event.pointer("/delta/partial_json").cloned().unwrap_or(Value::String(String::new()))}
                                            }]
                                        }), Value::Null, Value::Null)
                                    } else {
                                        continue;
                                    };
                                    yield chat_data_chunk(chunk);
                                }
                                Some("message_delta") => {
                                    if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                                        finish = Some(anthropic_finish_reason(Some(reason)));
                                    }
                                    output_tokens = event.pointer("/usage/output_tokens").and_then(Value::as_i64).unwrap_or(output_tokens);
                                }
                                Some("message_stop") => {
                                    let finish_reason = match finish {
                                        Some("tool_calls") | None if tool_calls_seen => "tool_calls",
                                        Some(value) => value,
                                        None => "stop",
                                    };
                                    let chunk = chat_chunk(&chat_id, &model, created, serde_json::json!({}), Value::String(finish_reason.into()), serde_json::json!({
                                        "prompt_tokens":input_tokens,
                                        "completion_tokens":output_tokens,
                                        "total_tokens":input_tokens+output_tokens
                                    }));
                                    yield chat_data_chunk(chunk);
                                    yield Bytes::from_static(b"data: [DONE]\n\n");
                                }
                                _ => {}
                            }
                        }
                        EngineProtocol::Gemini => {
                            if !opened {
                                opened = true;
                                yield chat_data_chunk(chat_chunk(&chat_id, &model, created, serde_json::json!({"role":"assistant"}), Value::Null, Value::Null));
                            }
                            let candidate = event.pointer("/candidates/0").cloned().unwrap_or_else(|| serde_json::json!({}));
                            let mut delta = serde_json::Map::new();
                            let mut tool_index = 0i64;
                            for part in candidate.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        delta.insert("content".into(), Value::String(text.to_owned()));
                                        yield chat_data_chunk(chat_chunk(&chat_id, &model, created, Value::Object(delta.clone()), Value::Null, Value::Null));
                                        delta.remove("content");
                                    }
                                }
                                if let Some(call) = part.get("functionCall") {
                                    tool_calls_seen = true;
                                    tool_index += 1;
                                    let tool_delta = serde_json::json!({
                                        "tool_calls":[{
                                            "index":tool_index - 1,
                                            "id":format!("call_{}",uuid::Uuid::new_v4().simple()),
                                            "type":"function",
                                            "function":{
                                                "name":call.get("name").cloned().unwrap_or(Value::Null),
                                                "arguments":serde_json::to_string(call.get("args").unwrap_or(&serde_json::json!({}))).unwrap_or_else(|_| "{}".into())
                                            }
                                        }]
                                    });
                                    yield chat_data_chunk(chat_chunk(&chat_id, &model, created, tool_delta, Value::Null, Value::Null));
                                }
                            }
                            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                                finish = Some(gemini_finish_reason(Some(reason)));
                            }
                            input_tokens = event.pointer("/usageMetadata/promptTokenCount").and_then(Value::as_i64).unwrap_or(input_tokens);
                            output_tokens = event.pointer("/usageMetadata/candidatesTokenCount").and_then(Value::as_i64).unwrap_or(output_tokens);
                        }
                        _ => {}
                    }
                }
            }
        }
        if protocol == EngineProtocol::Gemini {
            let finish_reason = match finish {
                Some("tool_calls") | None if tool_calls_seen => "tool_calls",
                Some(value) => value,
                None => "stop",
            };
            let chunk = chat_chunk(&chat_id, &model, created, serde_json::json!({}), Value::String(finish_reason.into()), serde_json::json!({
                "prompt_tokens":input_tokens,
                "completion_tokens":output_tokens,
                "total_tokens":input_tokens+output_tokens
            }));
            yield chat_data_chunk(chunk);
            yield Bytes::from_static(b"data: [DONE]\n\n");
        }
    })
}

fn chat_data_chunk(chunk: Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        serde_json::to_string(&chunk).unwrap_or_default()
    ))
}

fn codex_model_fallback(error_body: &[u8], payload: &Value) -> Option<String> {
    let parsed: Value = serde_json::from_slice(error_body).ok()?;
    let detail = parsed.get("detail").and_then(Value::as_str)?;
    let requested = payload
        .get("model")
        .and_then(Value::as_str)?
        .trim()
        .to_owned();
    if requested.is_empty()
        || !requested.starts_with("gpt-5")
        || !detail.contains("is not supported when using Codex")
        || !detail.contains(&requested)
    {
        return None;
    }
    let already_variant = requested
        .rsplit_once('-')
        .map(|(_, suffix)| {
            matches!(
                suffix,
                "sol" | "terra" | "luna" | "mini" | "spark" | "review"
            )
        })
        .unwrap_or(false);
    if already_variant {
        return None;
    }
    Some(format!("{requested}-sol"))
}

fn terminal_response_from_sse(body: &[u8]) -> AppResult<Value> {
    let text = std::str::from_utf8(body)
        .map_err(|_| AppError::Internal("上游 SSE 不是有效 UTF-8".into()))?;
    // Codex(ChatGPT OAuth) 后端的 response.completed 事件中 output 为空数组，
    // 完整输出只出现在 response.output_item.done 增量事件里，需要组装。
    let mut output_items: Vec<Value> = Vec::new();
    let mut failed = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item") {
                    output_items.push(item.clone());
                }
            }
            Some("response.completed") => {
                let mut response = event
                    .get("response")
                    .cloned()
                    .ok_or_else(|| AppError::Internal("response.completed 缺少 response".into()))?;
                let completed_output_empty = response
                    .get("output")
                    .and_then(Value::as_array)
                    .is_none_or(|items| items.is_empty());
                if completed_output_empty && !output_items.is_empty() {
                    response["output"] = Value::Array(output_items.clone());
                }
                return Ok(response);
            }
            Some("response.failed") => {
                let mut response = event.get("response").cloned();
                if let Some(response) = response.as_mut() {
                    let failed_output_empty = response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_none_or(|items| items.is_empty());
                    if failed_output_empty && !output_items.is_empty() {
                        response["output"] = Value::Array(output_items.clone());
                    }
                }
                failed = response;
            }
            _ => {}
        }
    }
    if let Some(response) = failed {
        return Ok(response);
    }
    Err(AppError::Internal("上游 SSE 缺少终止事件".into()))
}

fn credential(account: &EngineAccount, key: &str) -> AppResult<String> {
    account
        .credentials
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::Internal(format!("账户缺少凭证字段 {key}")))
}

struct LeaseGuard {
    cache: Option<SchedulerCache>,
    account_id: i64,
    request_id: String,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(cache) = self.cache.take() else {
            return;
        };
        let account_id = self.account_id;
        let request_id = std::mem::take(&mut self.request_id);
        tokio::spawn(async move {
            if let Err(error) = cache.release_account_slot(account_id, &request_id).await {
                tracing::warn!(account_id, request_id, error = %error, "failed to release Rust gateway account slot");
            }
        });
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn responses_subpath_guard_matches_closed_allowlist() {
        assert_eq!(responses_suffix("/v1/responses"), Some(String::new()));
        assert_eq!(
            responses_suffix("/responses/compact/detail"),
            Some("/compact/detail".into())
        );
        assert_eq!(
            responses_suffix("/backend-api/codex/responses/input_tokens"),
            Some("/input_tokens".into())
        );
        assert_eq!(responses_suffix("/v1/responses/../admin"), None);
        assert_eq!(responses_suffix("/v1/responses/unknown"), None);
    }

    #[test]
    fn chat_request_converts_messages_and_tools() {
        let converted = chat_to_responses(&serde_json::json!({
            "model":"gpt-5.4",
            "messages":[
                {"role":"system","content":"Be concise"},
                {"role":"user","content":"hello"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"done"}
            ],
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]
        })).unwrap();
        assert_eq!(converted["instructions"], "Be concise");
        assert_eq!(converted["input"].as_array().unwrap().len(), 4);
        assert_eq!(converted["tools"][0]["name"], "lookup");
        assert_eq!(converted["stream"], true);
    }

    #[test]
    fn completed_response_converts_to_chat_completion() {
        let response = serde_json::json!({
            "id":"resp_1","created_at":123,"status":"completed",
            "output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}],
            "usage":{"input_tokens":2,"output_tokens":1}
        });
        let chat = response_to_chat_completion(&response, &serde_json::json!({"model":"gpt-5.4"}));
        assert_eq!(chat["choices"][0]["message"]["content"], "OK");
        assert_eq!(chat["choices"][0]["finish_reason"], "stop");
        assert_eq!(chat["usage"]["total_tokens"], 3);
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
        assert_eq!(find_sse_frame_end(b"data: x\r\n\r\nrest"), Some((7, 4)));
    }

    #[test]
    fn credential_values_are_required_and_trimmed() {
        let account = account("apikey", serde_json::json!({"api_key":" secret "}));
        assert_eq!(credential(&account, "api_key").unwrap(), "secret");
        assert!(credential(&account, "access_token").is_err());
    }

    #[test]
    fn chat_request_converts_to_anthropic_messages() {
        let converted = chat_to_anthropic(&serde_json::json!({
            "model":"claude-sonnet-4-5",
            "messages":[
                {"role":"system","content":"Be concise"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"done"},
                {"role":"user","content":"bye"}
            ],
            "stream":true,
            "max_tokens":128
        }))
        .unwrap();
        assert_eq!(converted["model"], "claude-sonnet-4-5");
        assert_eq!(converted["system"], "Be concise");
        assert_eq!(converted["max_tokens"], 128);
        assert_eq!(converted["stream"], true);
        let messages = converted["messages"].as_array().unwrap();
        // user(hi) + assistant(text-less tool_use) + user(tool_result + bye merged)
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["input"]["q"], 1);
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][1]["type"], "text");
        assert!(anthropic_messages_url("https://api.anthropic.com").unwrap().ends_with("/v1/messages"));
        assert!(anthropic_messages_url("https://proxy.example.com/v1/").unwrap().ends_with("/v1/messages"));
    }

    #[test]
    fn anthropic_response_converts_to_chat_completion() {
        let chat = anthropic_to_chat_completion(
            &serde_json::json!({
                "id":"msg_1",
                "content":[
                    {"type":"text","text":"Hello"},
                    {"type":"tool_use","id":"toolu_1","name":"lookup","input":{"q":2}}
                ],
                "stop_reason":"tool_use",
                "usage":{"input_tokens":4,"output_tokens":2}
            }),
            &serde_json::json!({"model":"claude-sonnet-4-5"}),
        );
        assert_eq!(chat["id"], "msg_1");
        assert_eq!(chat["choices"][0]["message"]["content"], "Hello");
        assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":2}"
        );
        assert_eq!(chat["usage"]["total_tokens"], 6);
    }

    #[test]
    fn chat_request_converts_to_gemini_contents() {
        let (body, stream) = chat_to_gemini(&serde_json::json!({
            "model":"gemini-2.5-pro",
            "messages":[
                {"role":"system","content":"Be nice"},
                {"role":"user","content":"hi"},
                {"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"c1","content":"ok"}
            ],
            "stream":true
        }))
        .unwrap();
        assert_eq!(stream, true);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be nice");
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "lookup");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "c1");
        let url = gemini_generate_url("https://generativelanguage.googleapis.com", "gemini-2.5-pro", true)
            .unwrap();
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert!(gemini_generate_url("https://example.com", "bad model", false).is_err());
    }

    #[test]
    fn gemini_response_converts_to_chat_completion() {
        let chat = gemini_to_chat_completion(
            &serde_json::json!({
                "candidates":[{
                    "content":{"parts":[{"text":"Hey"}]},
                    "finishReason":"STOP"
                }],
                "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}
            }),
            &serde_json::json!({"model":"gemini-2.5-pro"}),
        );
        assert_eq!(chat["choices"][0]["message"]["content"], "Hey");
        assert_eq!(chat["choices"][0]["finish_reason"], "stop");
        assert_eq!(chat["usage"]["total_tokens"], 5);
    }

    #[test]
    fn upstream_protocol_errors_map_to_openai_error_shape() {
        let anthropic = protocol_error_to_openai(serde_json::json!({
            "type":"error","error":{"type":"overloaded_error","message":"Overloaded"}
        }));
        assert_eq!(anthropic["error"]["message"], "Overloaded");
        assert_eq!(anthropic["error"]["type"], "overloaded_error");
        let gemini = protocol_error_to_openai(serde_json::json!({
            "error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}
        }));
        assert_eq!(gemini["error"]["message"], "quota");
        assert_eq!(gemini["error"]["type"], "RESOURCE_EXHAUSTED");
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
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"GO-BASELINE-OK\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_x\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":16,\"output_tokens\":10}}}\n\n"
        );
        let response = terminal_response_from_sse(sse.as_bytes()).unwrap();
        let chat = response_to_chat_completion(
            &response,
            &serde_json::json!({"model":"gpt-5.6"}),
        );
        assert_eq!(chat["choices"][0]["message"]["content"], "GO-BASELINE-OK");
        assert_eq!(chat["usage"]["total_tokens"], 26);
    }

    #[test]
    fn codex_model_fallback_maps_family_alias_to_sol_variant() {
        let body = b"{\"detail\":\"The 'gpt-5.6' model is not supported when using Codex with a ChatGPT account.\"}";
        assert_eq!(
            codex_model_fallback(body, &serde_json::json!({"model":"gpt-5.6"})),
            Some("gpt-5.6-sol".into())
        );
        assert_eq!(
            codex_model_fallback(body, &serde_json::json!({"model":"gpt-5.6-sol"})),
            None
        );
        assert_eq!(
            codex_model_fallback(
                b"{\"detail\":\"Unsupported parameter: max_output_tokens\"}",
                &serde_json::json!({"model":"gpt-5.4"})
            ),
            None
        );
        assert_eq!(
            codex_model_fallback(body, &serde_json::json!({"model":"gpt-image-2"})),
            None
        );
    }
}
