use std::{
    sync::{Arc, LazyLock},
    time::Instant,
};

use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    console::{self, GatewayKey, UserModelRoute},
    error::{AppError, AppResult},
    state::AppState,
    store,
};

// A body can be as large as 32 MiB. Bound concurrent parsing/forwarding before
// reading it, including JSON requests whose configured user limits are zero.
static REQUEST_BODY_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(4)));

pub(crate) struct RequestIngress {
    pub(crate) key: GatewayKey,
    leases: Vec<crate::gateway_limits::Lease>,
    body_permit: OwnedSemaphorePermit,
}

pub(crate) async fn begin_ingress(
    state: &AppState,
    headers: &HeaderMap,
) -> AppResult<RequestIngress> {
    let key = authenticate(state, headers).await?;
    // 排队等待而不是立刻拒绝：瞬时多几个并发请求不该变成 429。
    // 仍然设上限，避免积压无限增长。
    let body_permit = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        REQUEST_BODY_SLOTS.clone().acquire_owned(),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or_else(|| AppError::TooManyRequests("网关正在处理较多请求，请稍后重试。".into()))?;
    let scopes = [
        (
            format!("user:{}", key.user_id),
            crate::gateway_limits::read(state, "user", key.user_id).await?,
        ),
        (
            format!("virtual-key:{}", key.id),
            crate::gateway_limits::read(state, "key", key.id).await?,
        ),
    ];
    let leases = crate::gateway_limits::acquire(state, &scopes)
        .await?
        .into_iter()
        .collect();
    Ok(RequestIngress {
        key,
        leases,
        body_permit,
    })
}

// A response body can be dropped before EOF (or before its first poll). Persist
// cancellation independently of that body future, and launch final logging in
// an owned task so cancellation during a DB await cannot create a second log.
struct StreamRequestLog {
    state: AppState,
    key: GatewayKey,
    model: UserModelRoute,
    provider: String,
    started: Instant,
    input: i64,
    output: i64,
    submitted: bool,
}
impl StreamRequestLog {
    fn finish(
        &mut self,
        status: u16,
        error: &str,
        complete: bool,
    ) -> tokio::task::JoinHandle<Option<i64>> {
        self.submitted = true;
        let state = self.state.clone();
        let key = self.key.clone();
        let model = self.model.clone();
        let provider = self.provider.clone();
        let duration = self.started.elapsed().as_millis() as i64;
        let input = self.input;
        let output = self.output;
        let error = error.to_owned();
        tokio::spawn(async move {
            console::record_request_with_usage(
                &state, &key, &model, &provider, status, duration, &error, input, output, complete,
            )
            .await
        })
    }
}
impl Drop for StreamRequestLog {
    fn drop(&mut self) {
        if !self.submitted {
            // 499 is an internal log outcome; no response can be sent to a
            // client that has disconnected. Partial usage is never a full bill.
            drop(self.finish(499, "客户端在流式响应完成前断开", false));
        }
    }
}

pub async fn models(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    let key = authenticate(&state, &headers).await?;
    let catalog = console::available_models(&state, key.user_id).await?;
    let data = catalog["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| allows_model(&key, model["alias"].as_str().unwrap_or_default()))
        .map(public_model)
        .collect::<Vec<_>>();
    Ok(Json(json!({"object":"list","data":data})))
}

fn public_model(model: &Value) -> Value {
    json!({"id":model["alias"],"object":"model","owned_by":"junohub",
        "junohub":{"api_type":model["api_type"],"endpoint":model["endpoint"],
            "request_formats":model["request_formats"],"response_mode":model["response_mode"],
            "capabilities":model["capabilities"],"billing_config":model["billing_config"],
            "input_price_per_million":model["input_price_per_million"],
            "output_price_per_million":model["output_price_per_million"]}})
}

pub async fn model(
    State(state): State<AppState>,
    Path(alias): Path<String>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let key = authenticate(&state, &headers).await?;
    if !allows_model(&key, &alias) {
        return Err(AppError::Forbidden(format!(
            "访问密钥未授权使用模型 {alias}，请在控制台调整该密钥的模型范围"
        )));
    }
    let catalog = console::available_models(&state, key.user_id).await?;
    catalog["list"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|model| model["alias"].as_str() == Some(&alias))
        .map(|model| Json(public_model(model)))
        .ok_or_else(|| AppError::NotFound("模型不存在".into()))
}

pub async fn passthrough(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    let ingress = begin_ingress(&state, request.headers()).await?;
    passthrough_admitted(State(state), request, ingress).await
}

pub(crate) async fn passthrough_admitted(
    State(state): State<AppState>,
    request: Request,
    ingress: RequestIngress,
) -> AppResult<Response> {
    let started = Instant::now();
    let headers = request.headers().clone();
    let RequestIngress {
        key,
        mut leases,
        body_permit,
    } = ingress;
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/v1/chat/completions")
        .to_owned();
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > crate::gateway_upload::MAX_BODY as u64)
    {
        return Err(AppError::PayloadTooLarge(
            "请求体不能超过 32MiB（包含所有文件和表单字段）".into(),
        ));
    }
    let multipart = crate::gateway_upload::is_multipart(&headers);
    if multipart {
        crate::gateway_upload::check_endpoint(&method, &path)?;
    }
    let body = to_bytes(request.into_body(), crate::gateway_upload::MAX_BODY)
        .await
        .map_err(|error| {
            if std::error::Error::source(&error)
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
            {
                AppError::PayloadTooLarge("请求体不能超过 32MiB（包含所有文件和表单字段）".into())
            } else {
                AppError::BadRequest("无法读取请求体".into())
            }
        })?;
    let upload = if multipart {
        Some(crate::gateway_upload::Upload::parse(&headers, body.clone()).await?)
    } else {
        None
    };
    let mut payload: Value = match &upload {
        Some(upload) => upload.metadata(),
        None => serde_json::from_slice(&body).map_err(|_| {
            AppError::BadRequest(
                "当前端点要求 JSON 请求体；图片编辑、语音转写和语音翻译也支持 multipart/form-data"
                    .into(),
            )
        })?,
    };
    drop(body);
    if is_responses_path(&path) {
        normalize_responses_input_content(&mut payload);
    }
    // Only provider-reported usage may enter consumption and cost accounting.
    let estimated_input_tokens = 0;
    let alias = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if alias.is_empty() {
        return Err(AppError::BadRequest("model 不能为空".into()));
    }
    // 401 会让客户端误判为“密钥无效”而去换 key；这里明确是授权范围问题。
    if !allows_model(&key, &alias) {
        return Err(AppError::Forbidden(format!(
            "访问密钥未授权使用模型 {alias}，请在控制台调整该密钥的模型范围"
        )));
    }

    let mut model = console::resolve_model(&state, key.user_id, &alias).await?;
    if let Some(id) = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
    {
        model.request_id = id.to_owned();
    }
    model
        .api_config
        .check_endpoint(&method, &path)
        .inspect_err(|_| {
            crate::observability::security_rejection(
                crate::observability::SecurityReason::InvalidEndpoint,
            )
        })?;
    crate::gateway_resources::validate(&state, &key, &model, &payload).await?;
    let anthropic_source = if model.api_config.converts_messages_to_chat() {
        crate::client_protocol::validate_anthropic_conversion(&payload)?;
        let source = payload.clone();
        payload = crate::client_protocol::anthropic_request_to_chat(&source)?;
        Some(source)
    } else {
        None
    };
    request_stream_usage(&mut payload, &path, &model.api_config);
    if model.api_config.is_task() && payload.get("stream").and_then(Value::as_bool) == Some(true) {
        return Err(AppError::BadRequest(
            "任务返回模式不支持 stream=true".into(),
        ));
    }
    let provider = console::load_provider(&state, model.owner_id, model.provider_id).await?;
    if upload.is_some() && provider.provider_type == "sub2api" {
        return Err(AppError::BadRequest(
            "账户引擎暂不支持文件请求，请使用配置了文件接口的 API Key 供应商".into(),
        ));
    }
    payload["model"] = json!(model.upstream_model);
    let scopes = [
        (
            format!("provider:{}", model.provider_id),
            crate::gateway_limits::read(&state, "provider", model.provider_id).await?,
        ),
        (
            format!("model:{}", model.id),
            crate::gateway_limits::read(&state, "model", model.id).await?,
        ),
    ];
    let resource_admission = match crate::gateway_limits::acquire(&state, &scopes).await {
        Ok(lease) => lease,
        Err(error) => {
            let status = if matches!(error, AppError::TooManyRequests(_)) {
                429
            } else {
                503
            };
            console::record_request(
                &state,
                &key,
                &model,
                &provider.provider_type,
                status,
                started.elapsed().as_millis() as i64,
                &error.to_string(),
                estimated_input_tokens,
                0,
            )
            .await;
            return Err(error);
        }
    };
    leases.extend(resource_admission);

    if provider.provider_type == "sub2api" {
        if !state.config.account_engine_gateway_enabled
            || !crate::sub2_gateway::supports_path(&path)
        {
            return Err(AppError::BadRequest(
                "该端点未纳入 Rust 账户引擎，请使用 /v1/chat/completions 或 /v1/responses".into(),
            ));
        }
        let response = match crate::sub2_gateway::forward(
            &state,
            &provider.api_key,
            method,
            &path,
            &headers,
            &payload,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                let status = match &error {
                    AppError::GatewayTimeout(_) => 504,
                    AppError::TooManyRequests(_) => 429,
                    AppError::BadRequest(_) => 400,
                    _ => 502,
                };
                console::record_request(
                    &state,
                    &key,
                    &model,
                    "rust-account-engine",
                    status,
                    started.elapsed().as_millis() as i64,
                    &error.to_string(),
                    estimated_input_tokens,
                    0,
                )
                .await;
                return Err(error);
            }
        };
        let response = record_response(
            &state,
            &key,
            &model,
            "rust-account-engine",
            started,
            response,
            estimated_input_tokens,
            Some(&provider.api_key),
        )
        .await?;
        drop(body_permit);
        return Ok(crate::gateway_limits::hold_many(response, leases));
    }
    let response = match crate::direct_provider::forward_route_with_upload(
        &state,
        crate::direct_provider::ForwardRequest {
            provider: model.provider_id,
            method,
            path: &path_and_query,
            headers: &headers,
            payload: &payload,
            route: crate::direct_provider::ForwardRoute {
                request_path: &model.request_path,
                config: &model.api_config,
                task_owner: Some((&key, &model)),
            },
            upload: upload.as_ref(),
        },
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let status = match error {
                AppError::BadRequest(_) => 400,
                AppError::TooManyRequests(_) => 429,
                AppError::GatewayTimeout(_) => 504,
                _ => 502,
            };
            console::record_request(
                &state,
                &key,
                &model,
                &provider.provider_type,
                status,
                started.elapsed().as_millis() as i64,
                &error.to_string(),
                estimated_input_tokens,
                0,
            )
            .await;
            return Err(error);
        }
    };
    let response = record_response(
        &state,
        &key,
        &model,
        &provider.provider_type,
        started,
        response,
        estimated_input_tokens,
        None,
    )
    .await?;
    let response = if let Some(source) = anthropic_source {
        crate::client_protocol::adapt_anthropic_response(
            response,
            &source,
            source["stream"] == true,
        )
        .await?
    } else {
        response
    };
    drop(body_permit);
    Ok(crate::gateway_limits::hold_many(response, leases))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_response(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    provider_type: &str,
    started: Instant,
    mut response: Response,
    _estimated_input_tokens: i64,
    engine_api_key: Option<&str>,
) -> AppResult<Response> {
    // Supplier cookies and authentication headers are never client credentials.
    let nominated = response
        .headers()
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| axum::http::HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    for name in nominated {
        response.headers_mut().remove(name);
    }
    for name in [
        "set-cookie",
        "authorization",
        "proxy-authorization",
        "proxy-authenticate",
        "www-authenticate",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "connection",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        response.headers_mut().remove(name);
    }
    let status = response.status();
    let account_id = response
        .headers()
        .get("x-junohub-engine-account-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    response.headers_mut().remove("x-junohub-engine-account-id");
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("text/event-stream") {
        let (parts, body) = response.into_parts();
        let state = state.clone();
        let key = key.clone();
        let model = model.clone();
        let provider_type = provider_type.to_owned();
        let engine_api_key = engine_api_key.map(str::to_owned);
        let mut log = StreamRequestLog {
            state: state.clone(),
            key: key.clone(),
            model: model.clone(),
            provider: provider_type.clone(),
            started,
            input: 0,
            output: 0,
            submitted: false,
        };
        let stream = async_stream::stream! {
            let mut upstream = body.into_data_stream();
            let mut pending = Vec::new();
            let mut stream_api = model.api_config.api_type.clone();
            let mut input_tokens = 0;
            let mut output_tokens = 0;
            let mut input_reported = false;
            let mut output_reported = false;
            let mut logged = false;
            let mut upstream_error = String::new();
            while let Some(chunk) = upstream.next().await {
                match chunk {
                    Ok(bytes) => {
                        pending.extend_from_slice(&bytes);
                        let mut terminal_frame = false;
                        let mut delivery = Vec::new();
                        while let Some((end, delimiter_length)) = find_sse_frame_end(&pending) {
                            let framed = pending.drain(..end + delimiter_length).collect::<Vec<_>>();
                            let frame = &framed[..end];
                            let decoded = crate::sub2_sse::decode(frame);
                            if let Ok(Some(crate::sub2_sse::Frame::Event(ref value))) = decoded {
                                match value.get("type").and_then(Value::as_str).unwrap_or_default() {
                                    kind if kind.starts_with("response.") => stream_api = crate::model_api::ApiType::Responses,
                                    "message_start" | "message_delta" | "content_block_delta" => stream_api = crate::model_api::ApiType::Messages,
                                    _ => {}
                                }
                            }
                            if decoded.is_err() {
                                if !logged {
                                    log.input=input_tokens; log.output=output_tokens;
                                    let _ = log.finish(502, "上游流式事件格式无效", false).await;
                                }
                                if !delivery.is_empty() { yield Ok::<_, axum::Error>(axum::body::Bytes::from(delivery)); }
                                yield Ok::<_, axum::Error>(stream_failure(&stream_api, "上游流式事件格式无效"));
                                return;
                            }
                            let (frame_input, frame_output) = extract_sse_usage_for_api(frame, &model.api_config.api_type);
                            let (has_input,has_output) = sse_usage_presence(frame,&model.api_config.api_type);
                            input_reported |= has_input;
                            output_reported |= has_output;
                            input_tokens = input_tokens.max(frame_input);
                            output_tokens = output_tokens.max(frame_output);
                            if status.is_success()
                                && crate::gateway_resources::remember_sse(&state, &key, &model, frame).await.is_err()
                            {
                                log.input=input_tokens; log.output=output_tokens;
                                let _ = log.finish(502, "响应归属登记失败", false).await;
                                if !delivery.is_empty() { yield Ok::<_, axum::Error>(axum::body::Bytes::from(delivery)); }
                                yield Ok::<_, axum::Error>(stream_failure(&stream_api, "响应归属登记失败"));
                                return;
                            }

                            if let Some(error) = sse_reported_error(frame) { upstream_error = error; terminal_frame = true; }
                            terminal_frame |= is_sse_done(frame);
                            delivery.extend_from_slice(&framed);
                            if terminal_frame {
                                break;
                            }
                        }
                        if !terminal_frame && pending.len() > 32 * 1024 * 1024 {
                            log.input=input_tokens; log.output=output_tokens;
                            let _ = log.finish(502,"上游单个流式事件超过 32MiB",false).await;
                            if !delivery.is_empty() { yield Ok::<_, axum::Error>(axum::body::Bytes::from(delivery)); }
                            yield Ok::<_, axum::Error>(stream_failure(&stream_api, "上游单个流式事件超过 32MiB"));
                            return;
                        }
                        // Many clients close the response as soon as they receive [DONE].
                        // Persist usage before forwarding that frame, or their disconnect
                        // can cancel the body stream before its EOF handler runs.
                        if terminal_frame && !logged {
                            log.input=input_tokens;log.output=output_tokens;
                            let _ = log.finish(
                                if status.is_success() && !upstream_error.is_empty() {502} else {status.as_u16()},
                                &upstream_error,
                                input_reported && output_reported,
                            ).await;
                            if status.is_success() && upstream_error.is_empty() {
                                record_engine_usage(
                                    &state,
                                    engine_api_key.as_deref(),
                                    account_id,
                                    &model,
                                    input_tokens,
                                    output_tokens,
                                    started.elapsed().as_millis() as i64,
                                    true,
                                ).await;
                            }
                            logged = true;
                        }
                        log.input=input_tokens;log.output=output_tokens;
                        if terminal_frame {
                            // The terminal event is the delivery/accounting boundary.
                            // Ignore later supplier bytes, even in this same chunk.
                            yield Ok::<_, axum::Error>(axum::body::Bytes::from(delivery));
                            return;
                        }
                        if !delivery.is_empty() {
                            yield Ok::<_, axum::Error>(axum::body::Bytes::from(delivery));
                        }
                    }
                    Err(_) => {
                        if !logged {
                            log.input=input_tokens;log.output=output_tokens;
                            let _ = log.finish(
                                if status.is_success() {502} else {status.as_u16()},
                                "上游流式响应读取失败",
                                false,
                            ).await;
                            logged = true;
                        }
                        yield Ok::<_, axum::Error>(stream_failure(&stream_api, "上游流式响应读取失败"));
                        break;
                    }
                }
            }
            if !logged {
                let (tail_input, tail_output) = extract_sse_usage_for_api(&pending, &model.api_config.api_type);
                input_tokens = input_tokens.max(tail_input);
                output_tokens = output_tokens.max(tail_output);
                if let Some(error) = sse_reported_error(&pending) { upstream_error = error; }
                if status.is_success() && upstream_error.is_empty() {
                    upstream_error = "上游流式响应在完成事件前结束".into();
                }
                log.input=input_tokens;log.output=output_tokens;
                let _ = log.finish(
                    if status.is_success() && !upstream_error.is_empty() {502} else {status.as_u16()},
                    &upstream_error,
                    false,
                ).await;
                if status.is_success() {
                    yield Ok::<_, axum::Error>(stream_failure(&stream_api, &upstream_error));
                }
            }
        };
        return Ok(Response::from_parts(parts, Body::from_stream(stream)));
    }
    let (mut parts, body) = response.into_parts();
    let task_id = parts
        .headers
        .remove("x-junohub-task-id")
        .and_then(|v| v.to_str().ok().map(str::to_owned));
    let bytes = match to_bytes(body, 32 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            console::record_request_with_usage(
                state,
                key,
                model,
                provider_type,
                502,
                started.elapsed().as_millis() as i64,
                "上游响应读取失败或超过大小限制",
                0,
                0,
                false,
            )
            .await;
            return Err(AppError::BadGateway(
                "上游响应读取失败或超过大小限制，请核对供应商记录后再重试".into(),
            ));
        }
    };
    let parsed = serde_json::from_slice::<Value>(&bytes).ok();
    if status.is_success() {
        let registered = if let Some(value) = &parsed {
            crate::gateway_resources::remember_json(state, key, model, value).await
        } else {
            let mut outcome = Ok(());
            let mut remaining = bytes.as_ref();
            while let Some((end, delimiter)) = find_sse_frame_end(remaining) {
                outcome =
                    crate::gateway_resources::remember_sse(state, key, model, &remaining[..end])
                        .await;
                if outcome.is_err() {
                    break;
                }
                remaining = &remaining[end + delimiter..];
            }
            if outcome.is_ok() {
                outcome =
                    crate::gateway_resources::remember_sse(state, key, model, remaining).await;
            }
            outcome
        };
        if registered.is_err() {
            let (input, output) = parsed
                .as_ref()
                .map(|value| extract_usage_for_api(value, &model.api_config.api_type))
                .unwrap_or_else(|| extract_sse_usage_for_api(&bytes, &model.api_config.api_type));
            let (has_input, has_output) = parsed
                .as_ref()
                .map(|value| {
                    let (input, output) = extract_reported_usage(value, &model.api_config.api_type);
                    (input.is_some(), output.is_some())
                })
                .unwrap_or_else(|| sse_usage_presence(&bytes, &model.api_config.api_type));
            console::record_request_with_billing(
                state,
                key,
                model,
                provider_type,
                502,
                started.elapsed().as_millis() as i64,
                "响应归属登记失败",
                input,
                output,
                has_input && has_output,
                parsed.as_ref(),
            )
            .await;
            return Err(AppError::BadGateway(
                "响应归属登记失败，请重新开始对话".into(),
            ));
        }
    }

    // Some compatible upstreams omit or mislabel their SSE Content-Type.
    let (input_tokens, output_tokens) = parsed
        .as_ref()
        .map(|value| extract_usage_for_api(value, &model.api_config.api_type))
        .unwrap_or_else(|| extract_sse_usage_for_api(&bytes, &model.api_config.api_type));
    let reported_error = parsed
        .as_ref()
        .and_then(reported_error)
        .or_else(|| sse_reported_error(&bytes));
    let buffered_sse_error = if parsed.is_none() && looks_like_sse(&bytes) {
        validate_buffered_sse(&bytes).err()
    } else {
        None
    };
    let error = if let Some(error) = reported_error {
        error
    } else if let Some(error) = buffered_sse_error {
        error.to_owned()
    } else if status.is_success() {
        String::new()
    } else {
        parsed
            .as_ref()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .or_else(|| value.get("detail"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default()
            .chars()
            .take(500)
            .collect()
    };
    let usage_complete = parsed
        .as_ref()
        .map(|value| {
            let (input, output) = extract_reported_usage(value, &model.api_config.api_type);
            input.is_some() && output.is_some()
        })
        .unwrap_or_else(|| {
            let (input, output) = sse_usage_presence(&bytes, &model.api_config.api_type);
            input && output && buffered_sse_error.is_none()
        });
    // A buffered HTTP 200 error envelope is an upstream failure. Streaming
    // responses have already sent their headers; those retain the wire status
    // and communicate failure via their error event and request-log outcome.
    if status.is_success() && !error.is_empty() {
        parts.status = StatusCode::BAD_GATEWAY;
    }
    let log_id = console::record_request_with_billing(
        state,
        key,
        model,
        provider_type,
        if status.is_success() && !error.is_empty() {
            502
        } else {
            status.as_u16()
        },
        started.elapsed().as_millis() as i64,
        &error,
        input_tokens,
        output_tokens,
        usage_complete,
        parsed.as_ref(),
    )
    .await;
    if let (Some(task_id), Some(log_id)) = (task_id, log_id) {
        crate::gateway_tasks::attach_log(state, &task_id, log_id).await?;
    }
    if status.is_success() && error.is_empty() {
        record_engine_usage(
            state,
            engine_api_key,
            account_id,
            model,
            input_tokens,
            output_tokens,
            started.elapsed().as_millis() as i64,
            false,
        )
        .await;
    }
    Ok(Response::from_parts(parts, Body::from(bytes)))
}

#[allow(clippy::too_many_arguments)]
async fn record_engine_usage(
    state: &AppState,
    engine_api_key: Option<&str>,
    account_id: Option<i64>,
    model: &UserModelRoute,
    input_tokens: i64,
    output_tokens: i64,
    duration_ms: i64,
    stream: bool,
) {
    let (Some(engine_api_key), Some(account_id)) = (engine_api_key, account_id) else {
        return;
    };
    if let Err(error) = crate::sub2_repository::Sub2Repository::new(state.sub2_pool.clone())
        .record_usage(
            engine_api_key,
            account_id,
            &model.alias,
            &model.upstream_model,
            input_tokens,
            output_tokens,
            duration_ms,
            stream,
        )
        .await
    {
        tracing::warn!(account_id, %error, "failed to persist account-engine usage");
    }
}

#[cfg(test)]
fn extract_usage(value: &Value) -> (i64, i64) {
    extract_usage_for_api(value, &crate::model_api::ApiType::Auto)
}

pub(crate) fn extract_usage_for_api(
    value: &Value,
    api_type: &crate::model_api::ApiType,
) -> (i64, i64) {
    let (input, output) = extract_reported_usage(value, api_type);
    (input.unwrap_or(0), output.unwrap_or(0))
}

// Preserve missing versus explicit zero. An absent output count cannot be
// treated as free output when calculating the full cost of a dialogue request.
pub(crate) fn extract_reported_usage(
    value: &Value,
    api_type: &crate::model_api::ApiType,
) -> (Option<i64>, Option<i64>) {
    use crate::model_api::ApiType;
    // Native Messages reports cache buckets separately. Converted Messages
    // already has prompt_tokens including them, so do not add them twice.
    if *api_type == ApiType::Messages || *api_type == ApiType::Auto {
        let source = value
            .pointer("/message/usage")
            .or_else(|| value.get("usage"));
        if let Some(source) = source.filter(|usage| {
            usage.get("prompt_tokens").is_none()
                && (*api_type == ApiType::Messages
                    || usage.get("cache_read_input_tokens").is_some()
                    || usage.get("cache_creation_input_tokens").is_some()
                    || matches!(
                        value["type"].as_str(),
                        Some("message" | "message_start" | "message_delta")
                    ))
        }) {
            let usage = crate::sub2_protocol::anthropic_chat_usage(source);
            return (
                usage.get("prompt_tokens").and_then(Value::as_i64),
                usage.get("completion_tokens").and_then(Value::as_i64),
            );
        }
    }
    let input = [
        "/usage/prompt_tokens",
        "/usage/input_tokens",
        "/input_tokens",
        "/usageMetadata/promptTokenCount",
        "/response/usage/prompt_tokens",
        "/response/usage/input_tokens",
        "/message/usage/input_tokens",
    ]
    .into_iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_i64)
            .filter(|count| *count >= 0)
    });
    // Only the embeddings contract makes total_tokens an input-only usage count.
    // The response marker also supports legacy automatic routes, without guessing by model name.
    let embeddings = *api_type == ApiType::Embeddings
        || (*api_type == ApiType::Auto
            && value
                .get("data")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    !items.is_empty() && items.iter().all(|item| item["object"] == "embedding")
                }));
    if embeddings {
        let total = [
            "/usage/total_tokens",
            "/total_tokens",
            "/usageMetadata/totalTokenCount",
            "/response/usage/total_tokens",
        ]
        .into_iter()
        .find_map(|pointer| {
            value
                .pointer(pointer)
                .and_then(Value::as_i64)
                .filter(|count| *count >= 0)
        });
        return (input.or(total), Some(0));
    }
    let output = [
        "/usage/completion_tokens",
        "/usage/output_tokens",
        "/usageMetadata/candidatesTokenCount",
        "/response/usage/completion_tokens",
        "/response/usage/output_tokens",
        "/message/usage/output_tokens",
    ]
    .into_iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_i64)
            .filter(|count| *count >= 0)
    });
    (input, output)
}

fn sse_usage_presence(bytes: &[u8], api_type: &crate::model_api::ApiType) -> (bool, bool) {
    sse_frames(bytes)
        .filter_map(|frame| match frame {
            crate::sub2_sse::Frame::Event(value) => Some(value),
            crate::sub2_sse::Frame::Done => None,
        })
        .map(|value| extract_reported_usage(&value, api_type))
        .fold((false, false), |(input, output), (i, o)| {
            (input || i.is_some(), output || o.is_some())
        })
}

#[cfg(test)]
fn extract_sse_usage(bytes: &[u8]) -> (i64, i64) {
    extract_sse_usage_for_api(bytes, &crate::model_api::ApiType::Auto)
}

fn extract_sse_usage_for_api(bytes: &[u8], api_type: &crate::model_api::ApiType) -> (i64, i64) {
    sse_frames(bytes)
        .filter_map(|frame| match frame {
            crate::sub2_sse::Frame::Event(value) => Some(value),
            crate::sub2_sse::Frame::Done => None,
        })
        .map(|event| extract_usage_for_api(&event, api_type))
        .fold((0, 0), |(input, output), (event_input, event_output)| {
            (input.max(event_input), output.max(event_output))
        })
}

fn reported_error(value: &Value) -> Option<String> {
    let failed = value.get("error").is_some_and(|error| !error.is_null())
        || matches!(value["type"].as_str(), Some("error" | "response.failed"))
        || value.pointer("/response/status").and_then(Value::as_str) == Some("failed");
    if !failed {
        return None;
    }
    Some(
        value
            .pointer("/error/message")
            .or_else(|| value.pointer("/response/error/message"))
            .or_else(|| value.get("error"))
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or("上游返回错误结果")
            .chars()
            .take(500)
            .collect(),
    )
}

fn sse_reported_error(bytes: &[u8]) -> Option<String> {
    sse_frames(bytes).find_map(|frame| match frame {
        crate::sub2_sse::Frame::Event(value) => reported_error(&value),
        crate::sub2_sse::Frame::Done => None,
    })
}

fn is_sse_done(bytes: &[u8]) -> bool {
    sse_frames(bytes).any(|frame| match frame {
        crate::sub2_sse::Frame::Done => true,
        crate::sub2_sse::Frame::Event(event) => event
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "response.completed"
                        | "response.incomplete"
                        | "response.failed"
                        | "message_stop"
                        | "error"
                )
            }),
    })
}

// Use the same decoder as protocol conversion so multiline JSON, CRLF and
// UTF-8 have identical meanings in delivery, accounting and error handling.
fn sse_frames(mut bytes: &[u8]) -> impl Iterator<Item = crate::sub2_sse::Frame> + '_ {
    std::iter::from_fn(move || {
        while !bytes.is_empty() {
            let (frame, remaining) = match crate::sub2_sse::frame_end(bytes) {
                Some((end, delimiter)) => (&bytes[..end], &bytes[end + delimiter..]),
                None => (bytes, &[][..]),
            };
            bytes = remaining;
            if let Ok(Some(frame)) = crate::sub2_sse::decode(frame) {
                return Some(frame);
            }
        }
        None
    })
}

fn looks_like_sse(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        let text = text.trim_start_matches('\u{feff}').trim_start();
        text.starts_with("data:") || text.starts_with("event:") || text.starts_with(':')
    })
}

fn validate_buffered_sse(mut bytes: &[u8]) -> Result<(), &'static str> {
    while let Some((end, delimiter)) = crate::sub2_sse::frame_end(bytes) {
        let frame = &bytes[..end];
        crate::sub2_sse::decode(frame).map_err(|_| "上游流式事件格式无效")?;
        if is_sse_done(frame) || sse_reported_error(frame).is_some() {
            return Ok(());
        }
        bytes = &bytes[end + delimiter..];
    }
    if bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err("上游 SSE 事件未完整结束");
    }
    Err("上游流式响应在完成事件前结束")
}

fn request_stream_usage(payload: &mut Value, path: &str, config: &crate::model_api::ApiConfig) {
    use crate::model_api::ApiType;
    let chat = *config.upstream_type() == ApiType::ChatCompletions
        || (*config.upstream_type() == ApiType::Auto
            && matches!(path, "/v1/chat/completions" | "/chat/completions"));
    if !chat || payload.get("stream").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let options = object.entry("stream_options").or_insert_with(|| json!({}));
    if options.is_null() {
        *options = json!({});
    }
    if let Some(options) = options.as_object_mut() {
        options.insert("include_usage".into(), Value::Bool(true));
    }
}

/// A readable failure event, without a fabricated completion or upstream transport details.
fn stream_failure(api: &crate::model_api::ApiType, message: &str) -> axum::body::Bytes {
    use crate::model_api::ApiType;
    let value = match api {
        ApiType::Messages => {
            serde_json::json!({"type":"error","error":{"type":"api_error","message":message}})
        }
        ApiType::Responses => {
            serde_json::json!({"type":"error","code":"upstream_error","message":message,"param":null})
        }
        _ => {
            serde_json::json!({"error":{"type":"upstream_error","code":"upstream_error","message":message}})
        }
    };
    let event = if matches!(api, ApiType::Messages | ApiType::Responses) {
        "event: error\n"
    } else {
        ""
    };
    axum::body::Bytes::from(format!("{event}data: {value}\n\n"))
}
fn find_sse_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    crate::sub2_sse::frame_end(buffer)
}

fn is_responses_path(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    [
        "/v1/responses",
        "/responses",
        "/backend-api/codex/responses",
    ]
    .into_iter()
    .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
}

/// Pi and a few OpenAI-compatible clients use the chat-style `text` content
/// discriminator when calling the Responses API. OpenAI requires a directional
/// discriminator there, so normalize only legacy text blocks and preserve every
/// native Responses content type unchanged.
fn normalize_responses_input_content(payload: &mut Value) {
    let Some(items) = payload.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    items.retain(|item| {
        let message = item.get("type").and_then(Value::as_str) == Some("message")
            || item.get("role").and_then(Value::as_str).is_some();
        !message
            || !matches!(item.get("content"), Some(Value::Null))
                && !item
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
    });
    for item in items {
        let role = item
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_owned();
        if let Some(content) = item.get_mut("content").filter(|v| v.is_array()) {
            *content =
                crate::sub2_protocol::normalize_chat_content_for_responses(content.take(), &role);
        }
    }
}

pub(crate) async fn authenticate(state: &AppState, headers: &HeaderMap) -> AppResult<GatewayKey> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::bearer_token)
        .ok_or(AppError::Unauthorized)?;
    let key = console::gateway_key(state, token)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if key.expires_at > 0 && key.expires_at <= store::now() {
        return Err(AppError::Unauthorized);
    }
    Ok(key)
}

pub(crate) fn allows_model(key: &GatewayKey, model: &str) -> bool {
    allows_scope(&key.allowed_models, model)
}

pub(crate) fn allows_scope(scope: &[String], model: &str) -> bool {
    scope.is_empty()
        || scope.iter().any(|allowed| {
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
    fn messages_cache_usage_preserves_missing_counts_and_avoids_double_counting() {
        use crate::model_api::ApiType;
        for api in [ApiType::Messages, ApiType::Auto] {
            assert_eq!(
                extract_reported_usage(
                    &json!({"type":"message", "usage":{
                        "input_tokens":0,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":5
                    }}),
                    &api
                ),
                (Some(120), Some(5))
            );
            assert_eq!(
                extract_reported_usage(
                    &json!({"type":"message_delta", "usage":{
                        "cache_read_input_tokens":100,"output_tokens":5
                    }}),
                    &api
                ),
                (None, Some(5))
            );
            assert_eq!(
                extract_reported_usage(
                    &json!({"usage":{
                        "prompt_tokens":130,"completion_tokens":5,
                        "prompt_tokens_details":{"cached_tokens":100,"cache_write_tokens":20}
                    }}),
                    &api
                ),
                (Some(130), Some(5))
            );
        }
    }

    #[test]
    fn extracts_usage_from_supported_response_shapes() {
        assert_eq!(
            extract_usage(&json!({"usage":{"prompt_tokens":12,"completion_tokens":7}})),
            (12, 7)
        );
        assert_eq!(
            extract_usage(
                &json!({"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":3}})
            ),
            (8, 3)
        );
    }

    #[test]
    fn embedding_usage_has_no_output_and_only_embedding_totals_are_input() {
        use crate::model_api::ApiType;
        for usage in [
            json!({"usage":{"prompt_tokens":12,"total_tokens":12}}),
            json!({"usage":{"input_tokens":12,"output_tokens":999}}),
            json!({"usage":{"total_tokens":12}}),
            json!({"usageMetadata":{"totalTokenCount":12}}),
        ] {
            assert_eq!(extract_usage_for_api(&usage, &ApiType::Embeddings), (12, 0));
        }
        assert_eq!(
            extract_usage_for_api(
                &json!({"usage":{"prompt_tokens":0,"total_tokens":12}}),
                &ApiType::Embeddings
            ),
            (0, 0)
        );
        assert_eq!(
            extract_usage_for_api(&json!({"usage":{"total_tokens":-12}}), &ApiType::Embeddings),
            (0, 0)
        );
        let total_only = json!({"usage":{"total_tokens":12}});
        for api in [
            ApiType::ChatCompletions,
            ApiType::Responses,
            ApiType::ImagesGenerations,
            ApiType::Auto,
        ] {
            assert_eq!(extract_usage_for_api(&total_only, &api), (0, 0));
        }
        assert_eq!(
            extract_usage(
                &json!({"data":[{"object":"embedding","embedding":[0.1,0.2]}],"usage":{"total_tokens":12}})
            ),
            (12, 0)
        );
        assert_eq!(
            extract_usage_for_api(
                &json!({"data":[{"embedding":[0.1,0.2]}]}),
                &ApiType::Embeddings
            ),
            (0, 0)
        );
        assert_eq!(
            extract_sse_usage_for_api(
                b"data: {\"usage\":{\"total_tokens\":12}}\n\ndata: [DONE]\n\n",
                &ApiType::Embeddings
            ),
            (12, 0)
        );
    }

    #[test]
    fn extracts_final_usage_from_sse() {
        let body = b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9}}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":5}}}\n\ndata: [DONE]\n\n";
        assert_eq!(extract_sse_usage(body), (9, 5));
        assert!(is_sse_done(b"data: [DONE]"));
        assert!(is_sse_done(b"data:  [DONE]\r\n"));
        assert!(!is_sse_done(b"data: {\"content\":\"[DONE]\"}"));
        assert!(is_sse_done(b"data: {\"type\":\"response.completed\"}\n\n"));
        assert!(is_sse_done(b"data: {\"type\":\"message_stop\"}\n\n"));
        assert!(!is_sse_done(
            b"data: {\"type\":\"response.output_text.done\"}\n\n"
        ));
    }

    #[test]
    fn streaming_chat_requests_usage_without_changing_other_protocols() {
        use crate::model_api::{ApiConfig, ApiType};
        let original = json!({"stream":true,"stream_options":{"include_obfuscation":false}});
        let mut chat = original.clone();
        request_stream_usage(&mut chat, "/v1/chat/completions", &ApiConfig::default());
        assert_eq!(
            chat["stream_options"],
            json!({"include_obfuscation":false,"include_usage":true})
        );
        for (kind, path) in [
            (ApiType::Responses, "/v1/responses"),
            (ApiType::Messages, "/v1/messages"),
        ] {
            let mut other = original.clone();
            request_stream_usage(
                &mut other,
                path,
                &ApiConfig {
                    api_type: kind,
                    ..Default::default()
                },
            );
            assert_eq!(other, original);
        }
        let mut unary = json!({"stream":false});
        request_stream_usage(&mut unary, "/v1/chat/completions", &ApiConfig::default());
        assert_eq!(unary, json!({"stream":false}));
        let mut custom = json!({"stream":true,"stream_options":null});
        request_stream_usage(
            &mut custom,
            "/vendor/chat",
            &ApiConfig {
                api_type: ApiType::ChatCompletions,
                ..Default::default()
            },
        );
        assert_eq!(custom["stream_options"]["include_usage"], true);
    }

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

    #[test]
    fn normalizes_legacy_pi_text_blocks_for_responses() {
        let mut payload = json!({
            "model":"route",
            "input":[
                {"role":"user","content":[{"type":"text","text":"question"},{"type":"input_image","image_url":"data:image/png;base64,abc"}]},
                {"type":"message","role":"assistant","content":[{"type":"text","text":"answer"},{"type":"refusal","refusal":"no"}]}
            ]
        });
        normalize_responses_input_content(&mut payload);
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(payload["input"][1]["content"][0]["type"], "output_text");
        assert_eq!(payload["input"][1]["content"][1]["type"], "refusal");
    }

    #[test]
    fn pi_read_image_history_is_responses_compatible_and_lossless() {
        let mut payload = json!({"input":[
            {"type":"function_call","call_id":"read-1","name":"read","arguments":"{}"},
            {"type":"function_call_output","call_id":"read-1","output":"Image loaded"},
            {"role":"user","content":[
                {"type":"text","text":"tool image"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,YWJj","detail":"high"}},
                {"type":"image_url","image_url":"https://example.test/image.png"},
                {"type":"input_image","file_id":"file-1","detail":"auto"}
            ]}
        ]});
        let tools = payload["input"].as_array().unwrap()[..2].to_vec();
        normalize_responses_input_content(&mut payload);
        assert_eq!(&payload["input"].as_array().unwrap()[..2], tools.as_slice());
        assert_eq!(
            payload["input"][2]["content"],
            json!([
                {"type":"input_text","text":"tool image"},
                {"type":"input_image","image_url":"data:image/png;base64,YWJj","detail":"high"},
                {"type":"input_image","image_url":"https://example.test/image.png"},
                {"type":"input_image","file_id":"file-1","detail":"auto"}
            ])
        );
        let once = payload.clone();
        normalize_responses_input_content(&mut payload);
        assert_eq!(payload, once);
    }

    #[test]
    fn removes_empty_message_items_from_responses_history() {
        let mut payload = json!({
            "input":[
                {"role":"user","content":[{"type":"text","text":"question"}]},
                {"type":"message","role":"assistant","content":null},
                {"type":"function_call","call_id":"call-1","name":"bash","arguments":"{}"},
                {"type":"message","role":"assistant","content":[]}
            ]
        });
        normalize_responses_input_content(&mut payload);
        assert_eq!(payload["input"].as_array().unwrap().len(), 2);
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["input"][1]["type"], "function_call");
    }

    #[test]
    fn recognizes_only_responses_endpoint_family() {
        assert!(is_responses_path("/v1/responses"));
        assert!(is_responses_path("/responses/input_tokens"));
        assert!(is_responses_path("/backend-api/codex/responses/compact"));
        assert!(!is_responses_path("/v1/chat/completions"));
        assert!(!is_responses_path("/v1/responses-invalid"));
    }
}
