use std::time::Instant;

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::TryStreamExt;
use serde_json::{json, Value};

use crate::{
    console::{self, GatewayKey},
    error::{AppError, AppResult},
    state::AppState,
    store,
};

pub async fn models(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    let key = authenticate(&state, &headers).await?;
    let aliases = console::gateway_models(&state, key.user_id).await?;
    let data = aliases
        .into_iter()
        .filter(|model| allows_model(&key, model))
        .map(|id| json!({"id":id,"object":"model","owned_by":"user"}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"object":"list","data":data})))
}

pub async fn passthrough(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    let started = Instant::now();
    let headers = request.headers().clone();
    let key = authenticate(&state, &headers).await?;
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/v1/chat/completions")
        .to_owned();
    let method = request.method().clone();
    let body = to_bytes(request.into_body(), 32 * 1024 * 1024)
        .await
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    let mut payload: Value = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("当前端点要求 JSON 请求体".into()))?;
    let alias = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if alias.is_empty() {
        return Err(AppError::BadRequest("model 不能为空".into()));
    }
    if !allows_model(&key, &alias) {
        return Err(AppError::Unauthorized);
    }

    let model = console::resolve_model(&state, key.user_id, &alias).await?;
    let provider = console::load_provider(&state, key.user_id, model.provider_id).await?;
    payload["model"] = json!(model.upstream_model);
    if provider.provider_type == "sub2api" {
        if !state.config.account_engine_gateway_enabled
            || !crate::sub2_gateway::supports_path(&path)
        {
            return Err(AppError::BadRequest(
                "该端点未纳入 Rust 账户引擎，请使用 /v1/chat/completions 或 /v1/responses".into(),
            ));
        }
        let response = crate::sub2_gateway::forward(
            &state,
            &provider.api_key,
            method,
            &path,
            &headers,
            &payload,
        )
        .await?;
        console::record_request(
            &state,
            &key,
            &model,
            "rust-account-engine",
            response.status().as_u16(),
            started.elapsed().as_millis() as i64,
            "",
        )
        .await;
        return Ok(response);
    }
    let upstream_path = path.strip_prefix("/v1").unwrap_or(&path);
    let url = format!(
        "{}{}",
        provider.base_url.trim_end_matches('/'),
        upstream_path
    );
    let mut upstream_request = state
        .client
        .request(method, url)
        .body(serde_json::to_vec(&payload)?);
    for (name, value) in &headers {
        if !matches!(name.as_str(), "host" | "content-length" | "authorization") {
            upstream_request = upstream_request.header(name, value);
        }
    }
    if !provider.api_key.is_empty() {
        upstream_request = upstream_request.bearer_auth(&provider.api_key);
    }

    let upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(error) => {
            console::record_request(
                &state,
                &key,
                &model,
                &provider.provider_type,
                502,
                started.elapsed().as_millis() as i64,
                &error.to_string(),
            )
            .await;
            return Err(AppError::Http(error));
        }
    };
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
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
    console::record_request(
        &state,
        &key,
        &model,
        &provider.provider_type,
        status.as_u16(),
        started.elapsed().as_millis() as i64,
        "",
    )
    .await;
    Ok(response)
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> AppResult<GatewayKey> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    let key = console::gateway_key(state, token)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if key.expires_at > 0 && key.expires_at < store::now() {
        return Err(AppError::Unauthorized);
    }
    Ok(key)
}

fn allows_model(key: &GatewayKey, model: &str) -> bool {
    key.allowed_models.is_empty()
        || key.allowed_models.iter().any(|allowed| {
            allowed == "*"
                || allowed.eq_ignore_ascii_case("all-proxy-models")
                || allowed == model
                || allowed
                    .strip_suffix('*')
                    .is_some_and(|prefix| model.starts_with(prefix))
        })
}

pub async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error":{"message":"route not found","type":"invalid_request_error"}})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_key_model_scope_supports_exact_and_prefix_rules() {
        let key = GatewayKey {
            id: 1,
            user_id: 2,
            allowed_models: vec!["gpt-*".into(), "exact".into()],
            expires_at: 0,
        };
        assert!(allows_model(&key, "gpt-4o"));
        assert!(allows_model(&key, "exact"));
        assert!(!allows_model(&key, "claude"));
    }
}
