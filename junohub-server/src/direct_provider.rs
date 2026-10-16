//! Direct OpenAI-compatible connections share the same dispatch policy as OAuth pools.
use crate::{
    error::{AppError, AppResult},
    gateway_limits::{self, Limits},
    secrets,
    state::AppState,
    sub2_policy::{self, AccountDispatch},
};
use axum::{
    body::Body,
    http::{HeaderMap, Method},
    response::Response,
};
use futures_util::StreamExt;
use redis::AsyncCommands;
use serde_json::{json, Value};
use sqlx::{FromRow, Row};

#[derive(FromRow)]
pub struct Key {
    pub id: i64,
    pub name: String,
    pub encrypted_api_key: String,
    pub base_url: String,
    pub priority: i32,
    pub member_order: i32,
    pub weight: i32,
    pub concurrency: i32,
    pub rpm: i32,
    pub allowed_models: String,
    pub cooldown_until: i64,
    pub health: String,
}
pub async fn keys(state: &AppState, provider: i64) -> AppResult<Vec<Key>> {
    Ok(
        sqlx::query_as("SELECT id,name,encrypted_api_key,(SELECT base_url FROM user_providers WHERE id=provider_keys.provider_id) AS base_url,CASE WHEN assigned=1 THEN priority ELSE 0 END AS priority,member_order,weight,concurrency,rpm,allowed_models,cooldown_until,health FROM provider_keys WHERE provider_id=? ORDER BY priority,id")
            .bind(provider)
            .fetch_all(&state.pool)
            .await?,
    )
}
pub async fn list(state: &AppState, provider: i64) -> AppResult<Value> {
    let list=keys(state,provider).await?.into_iter().map(|k|json!({"id":k.id,"name":k.name,"platform":"custom","kind":"apikey","base_url":k.base_url,"has_api_key":!k.encrypted_api_key.is_empty(),"priority":k.priority,"member_order":k.member_order,"weight":k.weight,"concurrency":k.concurrency,"rpm":k.rpm,"allowed_models":serde_json::from_str::<Value>(&k.allowed_models).unwrap_or(json!([])),"health":if k.cooldown_until>crate::store::now(){k.health.as_str()}else{"ready"},"cooldown_until":k.cooldown_until,"status":"active","schedulable":k.priority>0})).collect::<Vec<_>>();
    Ok(json!({"total":list.len(),"list":list}))
}
// Direct accounts inherit the provider endpoint, including accounts created before this rule.
async fn shared_base_url(state: &AppState, provider: i64, payload: &Value) -> AppResult<String> {
    let base: String = sqlx::query_scalar("SELECT base_url FROM user_providers WHERE id=?")
        .bind(provider)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("供应商不存在".into()))?;
    if let Some(requested) = payload.get("base_url") {
        let requested = crate::console::validate_base_url(requested.as_str().unwrap_or_default())?;
        if requested != crate::console::validate_base_url(&base)? {
            return Err(AppError::BadRequest(
                "同一供应商的账户共用 Base URL，不能单独修改账户地址".into(),
            ));
        }
    }
    Ok(base)
}
pub async fn create(state: &AppState, provider: i64, payload: &Value) -> AppResult<Value> {
    let name = payload["name"].as_str().unwrap_or_default().trim();
    if name.is_empty() || name.len() > 200 {
        return Err(AppError::BadRequest("请输入 1–200 字符的 Key 名称".into()));
    }
    let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_keys WHERE provider_id=? AND lower(trim(name))=lower(?) AND id<>?").bind(provider).bind(name).bind(0).fetch_one(&state.pool).await?;
    if duplicate > 0 {
        return Err(AppError::BadRequest(
            "此供应商下已有同名账户，请换一个名称".into(),
        ));
    }
    let base = shared_base_url(state, provider, payload).await?;
    let key = payload["api_key"].as_str().unwrap_or_default().trim();
    let encrypted = if key.is_empty() {
        String::new()
    } else {
        secrets::encrypt(&state.config.encryption_key, key)?
    };
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_keys WHERE provider_id=?")
        .bind(provider)
        .fetch_one(&state.pool)
        .await?;
    if count >= 100 {
        return Err(AppError::BadRequest("每个供应商最多 100 个 Key".into()));
    }
    let row = sqlx::query(
        "INSERT INTO provider_keys(provider_id,name,encrypted_api_key,base_url) VALUES(?,?,?,?)",
    )
    .bind(provider)
    .bind(name)
    .bind(encrypted)
    .bind(base)
    .execute(&state.pool)
    .await
    .map_err(|_| AppError::BadRequest("无法添加 Key，请检查名称是否重复".into()))?;
    Ok(json!({"id":row.last_insert_rowid()}))
}
pub async fn update(state: &AppState, provider: i64, payload: &Value) -> AppResult<Value> {
    let id = payload["account_id"].as_i64().unwrap_or(0);
    let row = sqlx::query(
        "SELECT name,base_url,encrypted_api_key FROM provider_keys WHERE id=? AND provider_id=?",
    )
    .bind(id)
    .bind(provider)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("Key 不存在".into()))?;
    let name = payload["name"].as_str().unwrap_or(row.get("name")).trim();
    if name.is_empty() || name.len() > 200 {
        return Err(AppError::BadRequest("请输入 Key 名称".into()));
    }
    let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_keys WHERE provider_id=? AND lower(trim(name))=lower(?) AND id<>?").bind(provider).bind(name).bind(id).fetch_one(&state.pool).await?;
    if duplicate > 0 {
        return Err(AppError::BadRequest(
            "此供应商下已有同名账户，请换一个名称".into(),
        ));
    }
    let base = shared_base_url(state, provider, payload).await?;
    let encrypted = if let Some(key) = payload["api_key"].as_str().filter(|k| !k.trim().is_empty())
    {
        secrets::encrypt(&state.config.encryption_key, key.trim())?
    } else {
        row.get("encrypted_api_key")
    };
    let changed = payload.get("api_key").is_some();
    sqlx::query("UPDATE provider_keys SET name=?,base_url=?,encrypted_api_key=?,cooldown_until=CASE WHEN ? THEN 0 ELSE cooldown_until END WHERE id=? AND provider_id=?").bind(name).bind(base).bind(encrypted).bind(changed).bind(id).bind(provider).execute(&state.pool).await.map_err(|_|AppError::BadRequest("无法更新 Key，请检查名称是否重复".into()))?;
    Ok(json!({"updated":true}))
}
pub async fn update_dispatch(
    state: &AppState,
    provider: i64,
    accounts: &[AccountDispatch],
) -> AppResult<()> {
    sub2_policy::validate_accounts(accounts)?;
    let mut tx = state.pool.begin().await?;
    for a in accounts {
        let result=sqlx::query("UPDATE provider_keys SET assigned=?,priority=?,member_order=?,weight=?,concurrency=?,rpm=?,allowed_models=? WHERE id=? AND provider_id=?").bind(a.priority > 0).bind(a.priority.max(1)).bind(a.member_order).bind(a.weight).bind(a.concurrency).bind(a.rpm).bind(serde_json::to_string(&a.allowed_models)?).bind(a.id).bind(provider).execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(AppError::NotFound("Key 不属于当前供应商".into()));
        }
    }
    tx.commit().await?;
    Ok(())
}
fn retryable(status: u16) -> bool {
    matches!(status, 401 | 403 | 408 | 429 | 500..=599)
}
#[cfg(test)]
pub async fn forward(
    state: &AppState,
    provider: i64,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    payload: &Value,
) -> AppResult<Response> {
    forward_with_path(state, provider, method, path, headers, payload, "").await
}

#[cfg(test)]
pub async fn forward_with_path(
    state: &AppState,
    provider: i64,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    payload: &Value,
    request_path: &str,
) -> AppResult<Response> {
    let config = crate::model_api::ApiConfig::default();
    forward_route(
        state,
        ForwardRequest {
            provider,
            method,
            path,
            headers,
            payload,
            route: ForwardRoute {
                request_path,
                config: &config,
                task_owner: None,
            },
            upload: None,
        },
    )
    .await
}

pub struct ForwardRoute<'a> {
    pub request_path: &'a str,
    pub config: &'a crate::model_api::ApiConfig,
    pub task_owner: Option<(
        &'a crate::console::GatewayKey,
        &'a crate::console::UserModelRoute,
    )>,
}

pub struct ForwardRequest<'a> {
    pub provider: i64,
    pub method: Method,
    pub path: &'a str,
    pub headers: &'a HeaderMap,
    pub payload: &'a Value,
    pub route: ForwardRoute<'a>,
    pub upload: Option<&'a crate::gateway_upload::Upload>,
}

#[cfg(test)]
pub async fn forward_route(state: &AppState, request: ForwardRequest<'_>) -> AppResult<Response> {
    forward_route_with_upload(state, request).await
}

pub async fn forward_route_with_upload(
    state: &AppState,
    request: ForwardRequest<'_>,
) -> AppResult<Response> {
    let policy = sub2_policy::for_provider(state, request.provider).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(policy.request_timeout_seconds as u64),
        forward_inner(state, &policy, request),
    )
    .await
    .map_err(|_| AppError::GatewayTimeout("供应商响应超过总超时预算".into()))?
}
async fn forward_inner(
    state: &AppState,
    policy: &sub2_policy::DispatchPolicy,
    request: ForwardRequest<'_>,
) -> AppResult<Response> {
    let ForwardRequest {
        provider,
        method,
        path,
        headers,
        payload,
        route,
        upload,
    } = request;
    let request_path = route.config.upstream_path(route.request_path);
    // Generation may already have started when a timeout/5xx reaches the gateway.
    // Retry media only when a provider adapter can establish submission safety.
    let replay_safe = !route.config.is_task()
        && !matches!(
            route.config.upstream_type(),
            crate::model_api::ApiType::ImagesGenerations
                | crate::model_api::ApiType::ImagesEdits
                | crate::model_api::ApiType::VideosGenerations
                | crate::model_api::ApiType::AudioGenerations
                | crate::model_api::ApiType::AudioSpeech
                | crate::model_api::ApiType::AudioTranscriptions
                | crate::model_api::ApiType::AudioTranslations
        )
        && !matches!(
            path.split('?').next().unwrap_or(path),
            "/v1/images/generations"
                | "/v1/images/edits"
                | "/v1/videos/generations"
                | "/v1/audio/generations"
                | "/v1/audio/speech"
                | "/v1/audio/transcriptions"
                | "/v1/audio/translations"
        );
    let keys = keys(state, provider).await?;
    let model = payload["model"].as_str().unwrap_or_default();
    let accounts: Vec<AccountDispatch> = keys
        .iter()
        .filter(|k| k.priority > 0 && k.cooldown_until <= crate::store::now())
        .filter_map(|k| {
            let allowed = match crate::console::parse_key_models(&k.allowed_models) {
                Ok(allowed) => allowed,
                Err(_) => {
                    crate::observability::security_rejection(
                        crate::observability::SecurityReason::InvalidAccountPermissions,
                    );
                    return None;
                }
            };
            if !allowed.is_empty() && !allowed.iter().any(|m| m == "*" || m == model) {
                return None;
            }
            Some(AccountDispatch {
                member_order: k.member_order,
                id: k.id,
                priority: k.priority,
                weight: k.weight,
                concurrency: k.concurrency,
                rpm: k.rpm,
                allowed_models: allowed,
            })
        })
        .collect();
    let mut redis = state
        .sub2_redis
        .clone()
        .ok_or_else(|| AppError::Internal("调度服务不可用".into()))?;
    let cursor = format!("junohub:direct:{provider}:cursor");
    let rotation: usize = redis
        .incr(&cursor, 1)
        .await
        .map_err(|_| AppError::Internal("调度服务不可用".into()))?;
    let _: redis::RedisResult<bool> = redis.expire(&cursor, 86400).await;
    let mut snapshot = vec![];
    for a in &accounts {
        let load: usize = redis
            .zcount(
                format!("junohub:admission:direct-key:{}:active", a.id),
                crate::store::now() * 1000 - 120000,
                "+inf",
            )
            .await
            .unwrap_or(0);
        snapshot.push((a.id, load as f64 / a.concurrency as f64));
    }
    snapshot.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut order = sub2_policy::rank_accounts(
        &accounts,
        &policy.strategy,
        rotation,
        &snapshot.iter().map(|a| a.0).collect::<Vec<_>>(),
    );
    use sha2::{Digest, Sha256};
    let session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty());
    let sticky_key = session.map(|s| {
        format!(
            "junohub:direct:{provider}:session:{}",
            hex::encode(Sha256::digest(s.as_bytes()))
        )
    });
    if policy.sticky_sessions && policy.strategy != "priority" {
        if let Some(key) = &sticky_key {
            let preferred: Option<i64> = redis.get(key).await.unwrap_or(None);
            let preferred = preferred.filter(|id| {
                accounts.iter().find(|a| a.id == *id).map(|a| a.priority)
                    == accounts.iter().map(|a| a.priority).min()
            });
            sub2_policy::order_candidates(&mut order, 0, preferred);
        }
    }
    let owner = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let response_scope = format!(
        "junohub:direct:{provider}:response:{}",
        hex::encode(Sha256::digest(owner.as_bytes()))
    );
    if let Some(previous) = payload
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let bound: Option<i64> = redis
            .get(response_binding_key(&response_scope, previous))
            .await
            .map_err(|_| AppError::Internal("无法读取续接账户".into()))?;
        let bound = bound.ok_or_else(|| {
            AppError::BadRequest("无法确定 previous_response_id 对应的 Key，请重新开始对话".into())
        })?;
        // A response ID belongs to one credential; never replay it against another Key.
        sub2_policy::pin_response_candidates(&mut order, Some(bound))?;
    }
    let mut attempted = 0;
    let mut last = None;
    for id in order {
        if attempted >= policy.max_attempts {
            break;
        }
        let key = keys.iter().find(|k| k.id == id).unwrap();
        let lease = match gateway_limits::acquire(
            state,
            &[(
                format!("direct-key:{id}"),
                Limits {
                    max_concurrency: key.concurrency as i64,
                    rpm: key.rpm as i64,
                },
            )],
        )
        .await
        {
            Ok(lease) => lease,
            Err(AppError::TooManyRequests(_)) => continue,
            Err(e) => return Err(e),
        };
        attempted += 1;
        let raw = if key.encrypted_api_key.is_empty() {
            String::new()
        } else {
            secrets::decrypt(&state.config.encryption_key, &key.encrypted_api_key)?
        };
        if upload.is_none()
            && !route.config.is_task()
            && request_path.is_empty()
            && crate::tokenhub_images::is_endpoint(&key.base_url, path)
        {
            if method != Method::POST {
                return Err(AppError::BadRequest("图片生成接口需要 POST".into()));
            }
            let response = crate::tokenhub_images::forward(
                &state.client,
                &key.base_url,
                &raw,
                headers,
                payload,
            )
            .await?;
            return Ok(gateway_limits::hold(response, lease));
        }
        let request = state.client.request(
            method.clone(),
            route
                .config
                .upstream_target(&key.base_url, path, route.request_path)?,
        );
        let mut request = match upload {
            Some(upload) => request.multipart(upload.form(model)),
            None => request.json(payload),
        };
        for (name, value) in headers {
            if name == "anthropic-beta" && route.config.context_1m {
                continue;
            }
            if !matches!(
                name.as_str(),
                "host"
                    | "content-length"
                    | "content-type"
                    | "authorization"
                    | "x-api-key"
                    | "api-key"
                    | "x-goog-api-key"
                    | "openai-organization"
                    | "openai-project"
                    | "chatgpt-account-id"
                    | "cookie"
                    | "proxy-authorization"
                    | "proxy-authenticate"
                    | "upgrade"
                    | "te"
                    | "trailer"
                    | "connection"
                    | "transfer-encoding"
            ) && !headers
                .get("connection")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case(name.as_str()))
                })
            {
                request = request.header(name, value);
            }
        }
        request = crate::model_api::authenticate(request, &route.config.auth_type, &raw);
        request = crate::model_api::apply_context_1m(request, route.config, headers);
        if *route.config.upstream_type() == crate::model_api::ApiType::Messages
            && !headers.contains_key("anthropic-version")
        {
            request = request.header("anthropic-version", "2023-06-01");
        }
        if route.config.is_task() {
            let (caller, model) = route
                .task_owner
                .ok_or_else(|| AppError::Internal("任务归属缺失".into()))?;
            let response = crate::gateway_tasks::submit(state, caller, model, key, request).await?;
            return Ok(gateway_limits::hold(response, lease));
        }
        match request.send().await {
            Ok(upstream) => {
                let status = upstream.status();
                let response_headers = upstream.headers().clone();
                if retryable(status.as_u16()) {
                    let cooldown = if response_headers.contains_key("retry-after") {
                        crate::sub2_gateway::retry_after_seconds(&response_headers, status)
                    } else if matches!(status.as_u16(), 401 | 403) {
                        600
                    } else {
                        policy.failure_cooldown_seconds
                    };
                    sqlx::query("UPDATE provider_keys SET cooldown_until=?,health=? WHERE id=?")
                        .bind(crate::store::now() + cooldown)
                        .bind(if status.as_u16() == 429 {
                            "rate_limited"
                        } else {
                            "cooling"
                        })
                        .bind(id)
                        .execute(&state.pool)
                        .await?;
                    tracing::warn!(
                        provider_id = provider,
                        key_id = id,
                        attempt = attempted,
                        upstream_status = status.as_u16(),
                        cooldown_seconds = cooldown,
                        retry_allowed = replay_safe && attempted < policy.max_attempts,
                        "Direct upstream attempt failed"
                    );
                    if replay_safe && attempted < policy.max_attempts {
                        last = Some(LastFailure::read(upstream, &raw).await);
                        drop(lease);
                        continue;
                    }
                }
                if status.is_success() {
                    if let Some(key) = &sticky_key {
                        if policy.sticky_sessions && policy.strategy != "priority" {
                            let _: redis::RedisResult<()> = redis.set_ex(key, id, 3600).await;
                        }
                    }
                }
                let mut upstream_stream = upstream.bytes_stream();
                let mut affinity_redis = redis.clone();
                let response_scope = response_scope.clone();
                let capture = status.is_success()
                    && path
                        .split('?')
                        .next()
                        .is_some_and(|path| path.ends_with("/responses"));
                let mut response = Response::new(Body::from_stream(async_stream::stream! {
                    let mut buffer=Vec::new();let mut bound=!capture;
                    while let Some(chunk)=upstream_stream.next().await {
                        if let Ok(bytes)=&chunk {if !bound {
                            if buffer.len()+bytes.len()>1024*1024 {bound=true;buffer.clear();}
                            else {buffer.extend_from_slice(bytes);if let Some(response_id)=response_id(&buffer) {
                                let _:redis::RedisResult<()>=affinity_redis.set_ex(response_binding_key(&response_scope,&response_id),id,86400).await;
                                bound=true;buffer.clear();
                            }}
                        }}
                        yield chunk.map_err(std::io::Error::other);
                    }
                }));
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
                return Ok(gateway_limits::hold(response, lease));
            }
            Err(_) => {
                tracing::warn!(
                    provider_id = provider,
                    key_id = id,
                    attempt = attempted,
                    retry_allowed = replay_safe,
                    "Direct upstream connection failed"
                );
                sqlx::query(
                    "UPDATE provider_keys SET cooldown_until=?,health='cooling' WHERE id=?",
                )
                .bind(crate::store::now() + policy.failure_cooldown_seconds)
                .bind(id)
                .execute(&state.pool)
                .await?;
                if !replay_safe {
                    return Err(AppError::GatewayTimeout(
                        "上游媒体请求结果未获确认；请先核对供应商记录，避免重复生成和计费".into(),
                    ));
                }
                last = Some(LastFailure {
                    status: axum::http::StatusCode::BAD_GATEWAY,
                    message: "上游连接失败".into(),
                    retry_after: None,
                });
            }
        }
    }
    if let Some(failure) = last {
        Ok(failure.into_response())
    } else {
        Err(AppError::TooManyRequests(
            "没有可用 Key：请检查并发、RPM、冷却状态和模型范围".into(),
        ))
    }
}

fn response_binding_key(scope: &str, id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{scope}:{}", hex::encode(Sha256::digest(id.as_bytes())))
}
/// Keep only a bounded public error, never an upstream HTML page or headers containing credentials.
struct LastFailure {
    status: axum::http::StatusCode,
    message: String,
    retry_after: Option<axum::http::HeaderValue>,
}
impl LastFailure {
    async fn read(upstream: reqwest::Response, key: &str) -> Self {
        let status = upstream.status();
        let retry_after = upstream
            .headers()
            .get("retry-after")
            .filter(|value| {
                value.to_str().ok().is_some_and(|value| {
                    value.trim().parse::<u64>().is_ok()
                        || chrono::DateTime::parse_from_rfc2822(value).is_ok()
                })
            })
            .cloned();
        let mut stream = upstream.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(Ok(chunk)) = stream.next().await {
            if bytes.len() + chunk.len() > 64 * 1024 {
                bytes.clear();
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        let detail = crate::upstream_probe::public_error_detail(&bytes, key);
        Self {
            status,
            message: detail
                .unwrap_or_else(|| format!("上游调用失败（HTTP {status}），可用 Key 已尝试完毕")),
            retry_after,
        }
    }
    fn into_response(self) -> Response {
        let mut response = Response::new(Body::from(
            json!({"error":{"message":self.message,"type":"upstream_error"}}).to_string(),
        ));
        *response.status_mut() = self.status;
        response
            .headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        if let Some(value) = self.retry_after {
            response.headers_mut().insert("retry-after", value);
        }
        response
    }
}
fn response_id(bytes: &[u8]) -> Option<String> {
    let extract = |value: Value| {
        if value.get("error").is_some()
            || matches!(
                value.get("type").and_then(Value::as_str),
                Some("response.failed" | "error")
            )
            || value.pointer("/response/status").and_then(Value::as_str) == Some("failed")
            || value.get("status").and_then(Value::as_str) == Some("failed")
        {
            return None;
        }
        value
            .get("response")
            .unwrap_or(&value)
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("resp_"))
            .map(str::to_owned)
    };
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return extract(value);
    }
    let mut remaining = bytes;
    while let Some((end, delimiter)) = crate::sub2_sse::frame_end(remaining) {
        if let Ok(Some(crate::sub2_sse::Frame::Event(value))) =
            crate::sub2_sse::decode(&remaining[..end])
        {
            if let Some(id) = extract(value) {
                return Some(id);
            }
        }
        remaining = &remaining[end + delimiter..];
    }
    None
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn migration_preserves_direct_keys_and_keeps_managed_secrets() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE user_providers(id INTEGER PRIMARY KEY,provider_type TEXT,base_url TEXT,encrypted_api_key TEXT); CREATE TABLE user_models(id INTEGER PRIMARY KEY); CREATE TABLE sub2_managed_pools(provider_id INTEGER); INSERT INTO user_providers VALUES(1,'openai-compatible','https://example.com/v1','encrypted-fixture'),(2,'sub2api','http://engine/v1','managed-fixture'),(3,'openai-compatible','http://local/v1',''); INSERT INTO sub2_managed_pools VALUES(2);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/0011_direct_key_pools.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(keys, 2);
        let key: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM provider_keys WHERE provider_id=1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(key, "encrypted-fixture");
        let old: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM user_providers WHERE id=1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(old.is_empty());
        let managed: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM user_providers WHERE id=2")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(managed, "managed-fixture");
    }

    #[tokio::test]
    async fn exhausted_fallback_preserves_safe_detail_and_retry_after() {
        use axum::body::to_bytes;
        use axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/", get(|| async {
            (axum::http::StatusCode::TOO_MANY_REQUESTS,
             [("retry-after", "17")],
             axum::Json(serde_json::json!({"error":{"message":"Service Unavailable fixture-secret (sk-other-secret)"}})))
        })).route("/large", get(|| async {
            (axum::http::StatusCode::BAD_GATEWAY, "x".repeat(65 * 1024))
        })).route("/html", get(|| async {
            (axum::http::StatusCode::SERVICE_UNAVAILABLE, [("retry-after", "fixture-secret")], "<html>fixture-secret</html>")
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (path, status, detail, retry) in [
            ("/", 429, "Service Unavailable", Some("17")),
            ("/large", 502, "HTTP 502", None),
            ("/html", 503, "HTTP 503", None),
        ] {
            let upstream = client
                .get(format!("http://{address}{path}"))
                .send()
                .await
                .unwrap();
            let response = super::LastFailure::read(upstream, "fixture-secret")
                .await
                .into_response();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok()),
                retry
            );
            let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.contains(detail));
            assert!(!text.contains("fixture-secret"));
            assert!(!text.contains("sk-other-secret"));
            assert!(!text.contains("<html>"));
        }
        server.abort();
    }

    #[test]
    fn response_binding_requires_complete_multiline_event_and_excludes_failure() {
        let event = b"event: response.created\r\ndata: {\"type\":\"response.created\",\r\ndata: \"response\":{\"id\":\"resp_multiline\"}}\r\n\r\n";
        for end in 0..event.len() {
            assert!(
                super::response_id(&event[..end]).is_none(),
                "bound incomplete frame at {end}"
            );
        }
        assert_eq!(super::response_id(event).as_deref(), Some("resp_multiline"));
        for failed in [
            r#"{"id":"resp_failed","status":"failed"}"#,
            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_failed\"}}\n\n",
            "data: {\"type\":\"error\",\"response\":{\"id\":\"resp_failed\"}}\n\n",
        ] {
            assert!(super::response_id(failed.as_bytes()).is_none());
        }
    }

    #[test]
    fn response_binding_reads_json_and_sse() {
        assert_eq!(
            super::response_id(br#"{"id":"resp_one","object":"response"}"#).as_deref(),
            Some("resp_one")
        );
        assert_eq!(
            super::response_id(
                b"event: response.created\ndata: {\"response\":{\"id\":\"resp_two\"}}\n\n"
            )
            .as_deref(),
            Some("resp_two")
        );
        assert!(super::response_id(br#"{"id":"resp_"#).is_none());
    }
}
