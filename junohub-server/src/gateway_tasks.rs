//! Tenant-scoped task handles keep polling on the credential used for submission.
use crate::{
    console::{GatewayKey, UserModelRoute},
    direct_provider::Key,
    error::{AppError, AppResult},
    model_api::{self, AuthType},
    state::AppState,
    store,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;

pub async fn submit(
    state: &AppState,
    caller: &GatewayKey,
    model: &UserModelRoute,
    account: &Key,
    request: reqwest::RequestBuilder,
) -> AppResult<Response> {
    // Persist before sending: a storage failure must not create an untrackable billed task.
    let id = format!("task_{}", uuid::Uuid::new_v4().simple());
    let now = store::now();
    sqlx::query("INSERT INTO gateway_tasks(id,user_id,virtual_key_id,model_id,model_alias,provider_id,provider_key_id,base_url,encrypted_api_key,auth_type,task_path,input_price,output_price,created_at,expires_at,api_type,billing_config) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&id).bind(caller.user_id).bind(caller.id).bind(model.id).bind(&model.alias).bind(model.provider_id).bind(account.id).bind(&account.base_url).bind(&account.encrypted_api_key).bind(serde_json::to_string(&model.api_config.auth_type)?).bind(&model.api_config.task_path)
        .bind(model.input_price_per_million).bind(model.output_price_per_million).bind(now).bind(now+3600)
        .bind(serde_json::to_value(&model.api_config.api_type)?.as_str().unwrap_or("auto")).bind(serde_json::to_string(&model.billing_config)?).execute(&state.pool).await?;
    // Task creation is never replayed against another credential on an uncertain failure.
    let upstream = request.send().await.map_err(|_| {
        AppError::GatewayTimeout("任务提交未获确认，请勿自动重试，以免重复生成和计费".into())
    })?;
    let status = upstream.status();
    if !status.is_success() {
        sqlx::query(
            "UPDATE gateway_tasks SET status='submit_failed',encrypted_api_key='' WHERE id=?",
        )
        .bind(&id)
        .execute(&state.pool)
        .await?;
        return Ok((status, Json(json!({"error":{"message":"上游未接受任务，请检查接口参数、鉴权和额度；504 时请先核对上游任务记录，避免重复提交","type":"upstream_error"}}))).into_response());
    }
    let bytes = limited_bytes(upstream).await?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Internal("上游任务响应不是 JSON，请核对接口返回方式；请勿重复提交".into())
    })?;
    let upstream_id = value
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::Internal("上游未返回 task_id，请核对返回方式；请勿重复提交".into())
        })?
        .to_owned();
    model_api::task_target(&account.base_url, &model.api_config.task_path, &upstream_id)?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "queued" | "processing" | "succeeded" | "failed"))
        .unwrap_or("processing")
        .to_owned();
    // Some providers return a completed task in the submit response and no longer
    // expose it to polling. Preserve that result before attaching the original log.
    let terminal = matches!(status.as_str(), "succeeded" | "failed");
    let cached = if terminal {
        value["task_id"] = json!(id);
        value["model"] = json!(model.alias);
        if status == "failed" {
            value["error"] = json!({"message":"上游生成任务失败，请检查请求参数或联系供应商"});
        }
        Some(value.to_string())
    } else {
        None
    };
    sqlx::query("UPDATE gateway_tasks SET upstream_task_id=?,status=?,result_json=?,encrypted_api_key=CASE WHEN ? THEN '' ELSE encrypted_api_key END WHERE id=?")
        .bind(upstream_id)
        .bind(&status)
        .bind(cached)
        .bind(terminal)
        .bind(&id)
        .execute(&state.pool)
        .await?;
    let mut response = (StatusCode::ACCEPTED, Json(json!({"task_id":id,"model":model.alias,"status":status,"query_url":format!("/v1/tasks/{id}"),"expires_at":now+3600}))).into_response();
    response
        .headers_mut()
        .insert("x-junohub-task-id", id.parse().unwrap());
    Ok(response)
}

pub async fn attach_log(state: &AppState, id: &str, log_id: i64) -> AppResult<()> {
    sqlx::query("UPDATE gateway_tasks SET log_id=? WHERE id=? AND log_id IS NULL")
        .bind(log_id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    settle(state, id).await?;
    Ok(())
}

pub async fn query(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let key = crate::proxy::authenticate(&state, &headers).await?;
    // Accepted jobs remain queryable with their original key even if an admin
    // edits/deletes the route or account. Authentication and key scope still apply.
    let row = sqlx::query("SELECT * FROM gateway_tasks WHERE id=? AND user_id=? AND virtual_key_id=? AND expires_at>?")
        .bind(&id).bind(key.user_id).bind(key.id).bind(store::now()).fetch_optional(&state.pool).await?
        .ok_or_else(|| AppError::NotFound("任务不存在、已过期或无权访问".into()))?;
    let alias: String = row.get("model_alias");
    if !crate::proxy::allows_model(&key, &alias) {
        return Err(AppError::Forbidden(format!(
            "访问密钥未授权使用模型 {alias}，请在控制台调整该密钥的模型范围"
        )));
    }
    poll_task(&state, row).await
}

async fn poll_task(state: &AppState, row: sqlx::sqlite::SqliteRow) -> AppResult<Response> {
    let id: String = row.get("id");
    let alias: String = row.get("model_alias");
    let upstream_id: String = row.get("upstream_task_id");
    if upstream_id.is_empty() {
        return Err(AppError::BadRequest(
            "任务提交未获确认，无法查询结果；请先核对上游记录，避免重复生成".into(),
        ));
    }
    if let Some(cached) = row.get::<Option<String>, _>("result_json") {
        let value: Value = serde_json::from_str(&cached)?;
        settle(state, &id).await?;
        return Ok(Json(value).into_response());
    }
    let provider_id: i64 = row.get("provider_id");
    let model_id: i64 = row.get("model_id");
    let account_id: i64 = row.get("provider_key_id");
    let scopes = [
        (
            format!("provider:{provider_id}"),
            crate::gateway_limits::read(state, "provider", provider_id).await?,
        ),
        (
            format!("model:{model_id}"),
            crate::gateway_limits::read(state, "model", model_id).await?,
        ),
        (
            format!("task-poll:{account_id}"),
            crate::gateway_limits::Limits {
                max_concurrency: 10,
                rpm: 120,
            },
        ),
    ];
    let lease = crate::gateway_limits::acquire(state, &scopes).await?;
    let encrypted: String = row.get("encrypted_api_key");
    let secret = if encrypted.is_empty() {
        String::new()
    } else {
        crate::secrets::decrypt(&state.config.encryption_key, &encrypted)?
    };
    let auth: AuthType = serde_json::from_str(row.get("auth_type"))?;
    let url = model_api::task_target(row.get("base_url"), row.get("task_path"), &upstream_id)?;
    let upstream = model_api::authenticate(
        state
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(20)),
        &auth,
        &secret,
    )
    .send()
    .await
    .map_err(|_| {
        AppError::GatewayTimeout("任务查询超时，请稍后继续查询，不要重新提交生成任务".into())
    })?;
    let status = upstream.status();
    if !status.is_success() {
        return Ok(crate::gateway_limits::hold((status,Json(json!({"error":{"message":"上游任务查询失败，请检查凭证、任务有效期和上游状态","type":"upstream_error"}}))).into_response(),lease));
    }
    let mut value: Value = serde_json::from_slice(&limited_bytes(upstream).await?)
        .map_err(|_| AppError::Internal("上游任务查询响应不是 JSON".into()))?;
    let task_status = value
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "queued" | "processing" | "succeeded" | "failed"))
        .ok_or_else(|| AppError::Internal("上游返回了无法识别的任务状态".into()))?
        .to_owned();
    value["task_id"] = json!(id);
    value["model"] = json!(alias);
    if task_status == "failed" {
        value["error"] = json!({"message":"上游生成任务失败，请检查请求参数或联系供应商"});
    }
    if matches!(task_status.as_str(), "succeeded" | "failed") {
        sqlx::query("UPDATE gateway_tasks SET status=?,result_json=?,encrypted_api_key='' WHERE id=? AND result_json IS NULL")
            .bind(&task_status).bind(value.to_string()).bind(&id).execute(&state.pool).await?;
        settle(state, &id).await?;
    } else {
        sqlx::query("UPDATE gateway_tasks SET status=? WHERE id=? AND result_json IS NULL")
            .bind(&task_status)
            .bind(&id)
            .execute(&state.pool)
            .await?;
    }
    // Another poll may have committed a terminal result while this request was
    // waiting upstream. Deliver the persisted winner, including to a late poll
    // that still reports processing, so clients and accounting agree.
    if let Some(cached) =
        sqlx::query_scalar::<_, Option<String>>("SELECT result_json FROM gateway_tasks WHERE id=?")
            .bind(&id)
            .fetch_one(&state.pool)
            .await?
    {
        value = serde_json::from_str(&cached)?;
    }
    Ok(crate::gateway_limits::hold(
        Json(value).into_response(),
        lease,
    ))
}

pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            crate::monitor::heartbeat(&state, "task_poller", true, None).await;
            let result = run_background_once(&state).await;
            crate::monitor::heartbeat(&state, "task_poller", true, Some(result.is_ok())).await;
            if result.is_err() {
                tracing::warn!("gateway task reconciliation delayed");
            }
        }
    })
}

pub(crate) async fn run_background_once(state: &AppState) -> AppResult<()> {
    use futures_util::{stream, StreamExt};
    let now = store::now();
    // Expired/uncertain submissions never get replayed. Keep an unpriced final
    // log instead of deleting an unfinished job and its credential silently.
    let expired=sqlx::query("SELECT id,model_alias FROM gateway_tasks WHERE expires_at<=? AND result_json IS NULL AND poll_lease_until<=? LIMIT 100")
        .bind(now).bind(now).fetch_all(&state.pool).await?;
    for row in expired {
        let id: String = row.get("id");
        let value = json!({"task_id":id,"model":row.get::<String,_>("model_alias"),"status":"failed","error":{"type":"task_expired","message":"任务超过查询有效期，最终结果和用量未获确认；请先核对供应商任务记录，避免重复生成"}});
        sqlx::query("UPDATE gateway_tasks SET status='failed',result_json=?,encrypted_api_key='' WHERE id=? AND result_json IS NULL")
            .bind(value.to_string()).bind(&id).execute(&state.pool).await?;
        settle(state, &id).await?;
    }
    // Retry settlement independently if a prior DB write or process shutdown
    // interrupted it after the terminal response had been persisted.
    let unsettled: Vec<String>=sqlx::query_scalar("SELECT id FROM gateway_tasks WHERE result_json IS NOT NULL AND log_id IS NOT NULL AND settled=0 LIMIT 100")
        .fetch_all(&state.pool).await?;
    for id in unsettled {
        settle(state, &id).await?;
    }
    let due: Vec<String>=sqlx::query_scalar("SELECT id FROM gateway_tasks WHERE result_json IS NULL AND upstream_task_id!='' AND log_id IS NOT NULL AND expires_at>? AND poll_after<=? AND poll_lease_until<=? ORDER BY poll_after,created_at LIMIT 16")
        .bind(now).bind(now).bind(now).fetch_all(&state.pool).await?;
    let results=stream::iter(due.into_iter().map(|id|async move {
        let owner=uuid::Uuid::new_v4().to_string();
        let row=sqlx::query("UPDATE gateway_tasks SET poll_owner=?,poll_lease_until=? WHERE id=? AND result_json IS NULL AND expires_at>? AND poll_after<=? AND poll_lease_until<=? RETURNING *")
            .bind(&owner).bind(store::now()+60).bind(&id).bind(store::now()).bind(store::now()).bind(store::now()).fetch_optional(&state.pool).await?;
        let Some(row)=row else {return AppResult::Ok(())};
        let failures: i64=row.get("poll_failures");
        let success=matches!(tokio::time::timeout(std::time::Duration::from_secs(25),poll_task(state,row)).await,Ok(Ok(response)) if response.status().is_success());
        let failures=if success {0} else {failures.saturating_add(1).min(30)};
        let delay=if success {10} else {10_i64.saturating_mul(1_i64<<failures.min(5)).min(300)};
        sqlx::query("UPDATE gateway_tasks SET poll_after=?,poll_failures=?,poll_lease_until=0,poll_owner='' WHERE id=? AND poll_owner=?")
            .bind(store::now()+delay).bind(failures).bind(&id).bind(owner).execute(&state.pool).await?;
        AppResult::Ok(())
    })).buffer_unordered(4).collect::<Vec<_>>().await;
    for result in results {
        result?;
    }
    // A seven-day result window allows operations to inspect failed jobs. Never
    // purge a terminal job whose associated log still needs settlement.
    sqlx::query("DELETE FROM gateway_tasks WHERE expires_at<? AND result_json IS NOT NULL AND (settled=1 OR log_id IS NULL)")
        .bind(now-7*86400).execute(&state.pool).await?;
    Ok(())
}

async fn limited_bytes(response: reqwest::Response) -> AppResult<Vec<u8>> {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| AppError::Internal("上游任务响应读取失败，请稍后继续查询".into()))?;
        if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
            return Err(AppError::Internal("上游任务响应超过 32MB 限制".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

// Complete the original submission log once. Polling neither adds requests nor double-counts usage.
async fn settle(state: &AppState, id: &str) -> AppResult<()> {
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("UPDATE gateway_tasks SET settled=1 WHERE id=? AND settled=0 AND log_id IS NOT NULL AND result_json IS NOT NULL RETURNING result_json,log_id,virtual_key_id,user_id,input_price,output_price,created_at,api_type,billing_config")
        .bind(id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else { return Ok(()) };
    let value: Value = serde_json::from_str(row.get("result_json"))?;
    let api_type = serde_json::from_value(json!(row.get::<String, _>("api_type")))?;
    let (top_input, top_output) = crate::proxy::extract_usage_for_api(&value, &api_type);
    let (nested_input, nested_output) =
        crate::proxy::extract_usage_for_api(value.get("result").unwrap_or(&value), &api_type);
    let (top_reported_input, top_reported_output) =
        crate::proxy::extract_reported_usage(&value, &api_type);
    let (nested_reported_input, nested_reported_output) =
        crate::proxy::extract_reported_usage(value.get("result").unwrap_or(&value), &api_type);
    let usage_complete = top_reported_input.or(nested_reported_input).is_some()
        && top_reported_output.or(nested_reported_output).is_some();
    let (input, output) = (top_input.max(nested_input), top_output.max(nested_output));
    let total = input.saturating_add(output);
    let failed = value["status"] == "failed";
    let config = crate::gateway_billing::Config::stored(row.get("billing_config"))?;
    let bill = crate::gateway_billing::calculate(
        &config,
        row.get("input_price"),
        row.get("output_price"),
        input,
        output,
        usage_complete,
        Some(&value),
        !failed,
    );
    let cost = bill.cost;
    let expired = value.pointer("/error/type").and_then(Value::as_str) == Some("task_expired");
    sqlx::query("UPDATE user_request_logs SET status_code=?,duration_ms=?,error_message=?,input_tokens=?,output_tokens=?,total_tokens=?,cost_usd=?,billing_details=? WHERE id=? AND user_id=?")
        .bind(if expired {504} else if failed {502} else {200}).bind((store::now()-row.get::<i64,_>("created_at"))*1000).bind(if expired {"任务超过查询有效期，最终用量未获确认"} else if failed {"上游生成任务失败"} else {""}).bind(input).bind(output).bind(total).bind(cost).bind(bill.details.to_string()).bind(row.get::<i64,_>("log_id")).bind(row.get::<i64,_>("user_id")).execute(&mut *tx).await?;
    sqlx::query("UPDATE virtual_keys SET input_tokens=input_tokens+?,output_tokens=output_tokens+?,cost_usd=cost_usd+?,unpriced_requests=MAX(0,unpriced_requests-?),last_used_at=? WHERE id=? AND user_id=?")
        .bind(input).bind(output).bind(cost.unwrap_or(0.0)).bind(i64::from(cost.is_some())).bind(store::now()).bind(row.get::<i64,_>("virtual_key_id")).bind(row.get::<i64,_>("user_id")).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
