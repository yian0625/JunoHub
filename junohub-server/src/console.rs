use std::collections::HashSet;

use axum::{
    extract::{Extension, Path, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{QueryBuilder, Row, Sqlite};
use uuid::Uuid;

use crate::{
    auth::{issue_user_token_at_version, CurrentUser},
    error::{AppError, AppResult},
    models::Envelope,
    secrets,
    state::AppState,
    store,
    sub2_repository::Sub2Repository,
};

#[derive(Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RegistrationInput {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub verification_code: String,
}

pub async fn ensure_bootstrap_admin(state: &AppState) -> AppResult<()> {
    let email = state.config.bootstrap_admin_email.trim();
    let password = state.config.bootstrap_admin_password.as_str();
    if email.is_empty() && password.is_empty() {
        return Ok(());
    }
    if email.is_empty() || password.is_empty() {
        return Err(AppError::Internal(
            "引导管理员邮箱和密码必须同时设置".into(),
        ));
    }
    let email = normalize_email(email)?;
    validate_password(password)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email=?)")
        .bind(&email)
        .fetch_one(&state.pool)
        .await?;
    if exists {
        return Ok(());
    }
    let password_hash = secrets::hash_password(password)?;
    let timestamp = store::now();
    sqlx::query(
        "INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) \
         VALUES(?,?,'JunoHub','admin',1,?,?) \
         ON CONFLICT(email) DO NOTHING",
    )
    .bind(email)
    .bind(password_hash)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&state.pool)
    .await?;
    Ok(())
}

pub async fn register(
    State(state): State<AppState>,
    Json(input): Json<RegistrationInput>,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let email = normalize_email(&input.email)?;
    validate_password(&input.password)?;
    let display_name = if input.display_name.trim().is_empty() {
        email.split('@').next().unwrap_or("user").to_owned()
    } else {
        input.display_name.trim().to_owned()
    };
    let timestamp = store::now();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Err(error) = crate::email_verification::consume(
        &mut tx,
        &state.config.encryption_key,
        &email,
        &input.verification_code,
        timestamp,
    )
    .await
    {
        // Persist failed attempts instead of rolling them back on validation errors.
        if matches!(error, AppError::FormFailed { .. }) {
            tx.commit().await?;
        }
        return Err(error);
    }
    let password_hash =
        tokio::task::spawn_blocking(move || secrets::hash_password(&input.password))
            .await
            .map_err(|_| AppError::Internal("密码处理失败".into()))??;
    let result = sqlx::query("INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) VALUES(?,?,?,'user',1,?,?)")
        .bind(&email)
        .bind(password_hash)
        .bind(&display_name)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if error.to_string().contains("UNIQUE") {
                AppError::FormFailed { reason: "email_registered", message: "该邮箱已注册，请直接登录。", field: "email" }
            } else {
                AppError::Sql(error)
            }
        })?;
    tx.commit().await?;
    login_response(
        &state,
        result.last_insert_rowid(),
        email,
        display_name,
        String::new(),
        "user".into(),
        0,
    )
}

pub async fn login(
    State(state): State<AppState>,
    Json(input): Json<Credentials>,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let email = normalize_email(&input.email)?;
    let row = sqlx::query(
        "SELECT id,password_hash,display_name,avatar_url,role,status,session_version FROM users WHERE email=?",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LoginFailed {
        reason: "email_not_registered",
        message: "该邮箱尚未注册，请先注册。",
        field: Some("email"),
    })?;
    if input.password.is_empty() || input.password.len() > 128 {
        return Err(AppError::LoginFailed {
            reason: "password_incorrect",
            message: "密码不正确，请重新输入。",
            field: Some("password"),
        });
    }
    let password_hash: String = row.get("password_hash");
    let verified = tokio::task::spawn_blocking(move || {
        secrets::verify_password(&input.password, &password_hash)
    })
    .await
    .map_err(|_| AppError::Internal("密码校验失败".into()))?;
    if !verified {
        return Err(AppError::LoginFailed {
            reason: "password_incorrect",
            message: "密码不正确，请重新输入。",
            field: Some("password"),
        });
    }
    if row.get::<i64, _>("status") != 1 {
        return Err(AppError::LoginFailed {
            reason: "account_disabled",
            message: "账户已停用，请联系管理员。",
            field: None,
        });
    }
    login_response(
        &state,
        row.get("id"),
        email,
        row.get("display_name"),
        row.get("avatar_url"),
        row.get("role"),
        row.get("session_version"),
    )
}

fn login_response(
    state: &AppState,
    uid: i64,
    email: String,
    display_name: String,
    avatar_url: String,
    role: String,
    session_version: i64,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let token =
        issue_user_token_at_version(state, uid, email.clone(), role.clone(), session_version)?;
    let redirect_url = if role == "admin" {
        "/juno-hub"
    } else {
        "/console"
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Jwt-Token",
        token
            .parse()
            .map_err(|_| AppError::Internal("token header".into()))?,
    );
    Ok((
        headers,
        Json(Envelope::ok(json!({
            "uid": uid,
            "email": email,
            "nickname": display_name,
            "display_name": display_name,
            "avatar_url": avatar_url,
            "role": role,
            "token": token,
            "access_token": token,
            "redirect_url": redirect_url
        }))),
    ))
}

pub async fn account_dispatch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(path): Path<String>,
    Json(payload): Json<Value>,
) -> AppResult<Json<Envelope<Value>>> {
    let data = match path.trim_matches('/') {
        "profile" => profile(&state, user.uid).await?,
        "profile/update" => update_account_profile(&state, user.uid, &payload).await?,
        "password/update" => update_account_password(&state, user.uid, &payload).await?,
        path => {
            return Err(AppError::NotFound(format!(
                "账户接口不存在: /account-api/{path}"
            )))
        }
    };
    Ok(Json(Envelope::ok(data)))
}

pub async fn dispatch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(path): Path<String>,
    Json(payload): Json<Value>,
) -> AppResult<Json<Envelope<Value>>> {
    let path = path.trim_matches('/');
    authorize_console_role(&user.role, path)?;
    let data = match path {
        "monitor/overview" => crate::monitor::overview(&state, &payload).await?,
        "monitor/settings" => crate::monitor::settings(&state).await?,
        "monitor/settings/update" => crate::monitor::update_settings(&state, &payload).await?,
        "monitor/alerts" => crate::monitor::alerts(&state, &payload).await?,
        "monitor/alerts/acknowledge" => {
            crate::monitor::acknowledge(&state, user.uid, &payload).await?
        }
        "monitor/tasks/recheck" => crate::monitor::recheck_task(&state, &payload).await?,
        "monitor/tasks" => crate::monitor::tasks(&state, &payload).await?,
        "monitor/audit" => crate::monitor::audit_list(&state, &payload).await?,
        "guide/get" => crate::access_guide::read(&state).await?,
        "profile" => profile(&state, user.uid).await?,
        "provider/list" => list_providers(&state, user.uid).await?,
        "provider/create" => create_provider(&state, user.uid, payload).await?,
        "provider/update" => update_provider(&state, user.uid, payload).await?,
        "provider/delete" => delete_provider(&state, user.uid, id(&payload)?).await?,
        "provider/test-connection" => {
            let report = crate::upstream_probe::probe_configured(
                &state,
                &string(&payload, "base_url"),
                &string(&payload, "api_key"),
                payload["platform"].as_str().unwrap_or("custom"),
                payload["model"].as_str(),
                crate::upstream_probe::test_config(&payload)?.as_ref(),
            )
            .await?;
            serde_json::to_value(report)?
        }
        "provider/test" => test_provider(&state, user.uid, payload).await?,
        "provider/models" => provider_models(&state, user.uid, id(&payload)?).await?,
        "provider/policy" => {
            let provider = id(&payload)?;
            provider_is_managed(&state, user.uid, provider).await?;
            serde_json::to_value(crate::sub2_policy::for_provider(&state, provider).await?)?
        }
        "provider/policy/update" => {
            let provider_id = id(&payload)?;
            provider_is_managed(&state, user.uid, provider_id).await?;
            let policy: crate::sub2_policy::DispatchPolicy =
                serde_json::from_value(payload.clone())
                    .map_err(|_| AppError::BadRequest("请提供完整的调用策略".into()))?;
            policy.validate()?;
            sqlx::query("INSERT INTO provider_dispatch_policy(provider_id,strategy,max_attempts,sticky_sessions,request_timeout_seconds,failure_cooldown_seconds) VALUES(?,?,?,?,?,?) ON CONFLICT(provider_id) DO UPDATE SET strategy=excluded.strategy,max_attempts=excluded.max_attempts,sticky_sessions=excluded.sticky_sessions,request_timeout_seconds=excluded.request_timeout_seconds,failure_cooldown_seconds=excluded.failure_cooldown_seconds")
                .bind(provider_id).bind(&policy.strategy).bind(policy.max_attempts).bind(policy.sticky_sessions).bind(policy.request_timeout_seconds).bind(policy.failure_cooldown_seconds).execute(&state.pool).await?;
            serde_json::to_value(policy)?
        }
        "provider/account/create" => {
            let provider = payload["provider_id"].as_i64().unwrap_or(0);
            if provider_is_managed(&state, user.uid, provider).await? {
                return Err(AppError::BadRequest("内置供应商请使用平台授权入口".into()));
            }
            crate::direct_provider::create(&state, provider, &payload).await?
        }
        "provider/limits"
        | "model/limits"
        | "key/limits"
        | "provider/limits/update"
        | "model/limits/update"
        | "key/limits/update"
        | "user/limits"
        | "user/limits/update" => {
            let kind = if path.starts_with("provider/") {
                "provider"
            } else if path.starts_with("user/") {
                "user"
            } else if path.starts_with("key/") {
                "key"
            } else {
                "model"
            };
            resource_limits(
                &state,
                user.uid,
                kind,
                id(&payload)?,
                path.ends_with("/update"),
                &payload,
            )
            .await?
        }
        "provider/accounts" => list_provider_accounts(&state, user.uid, &payload).await?,
        "provider/accounts/update" => {
            update_provider_account_priorities(&state, user.uid, &payload).await?
        }
        "provider/accounts/dispatch" => {
            let provider_id = payload
                .get("provider_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| AppError::BadRequest("供应商标识无效".into()))?;
            let managed = provider_is_managed(&state, user.uid, provider_id).await?;
            let accounts: Vec<crate::sub2_policy::AccountDispatch> =
                serde_json::from_value(payload.get("accounts").cloned().unwrap_or(Value::Null))
                    .map_err(|_| AppError::BadRequest("账户调度参数格式无效".into()))?;
            if managed {
                let group_id = managed_group_id(&state, user.uid, provider_id).await?;
                Sub2Repository::new(state.sub2_pool.clone())
                    .update_account_dispatch(group_id, &accounts)
                    .await?;
            } else {
                crate::direct_provider::update_dispatch(&state, provider_id, &accounts).await?;
            }
            json!({"updated":accounts.len()})
        }
        "provider/account/delete" => {
            let provider_id = payload
                .get("provider_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("供应商标识无效".into()))?;
            let account_id = payload
                .get("account_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| AppError::BadRequest("账户标识无效".into()))?;
            if provider_is_managed(&state, user.uid, provider_id).await? {
                let group_id = managed_group_id(&state, user.uid, provider_id).await?;
                Sub2Repository::new(state.sub2_pool.clone())
                    .delete_managed_account(group_id, account_id)
                    .await?;
            } else {
                let result = sqlx::query("DELETE FROM provider_keys WHERE id=? AND provider_id=?")
                    .bind(account_id)
                    .bind(provider_id)
                    .execute(&state.pool)
                    .await?;
                if result.rows_affected() != 1 {
                    return Err(AppError::NotFound("Key 不存在".into()));
                }
            }
            json!({"deleted":true})
        }
        "provider/account/update" => {
            update_provider_account_config(&state, user.uid, &payload).await?
        }
        "model/list" => list_models(&state, user.uid).await?,
        "model/available" => available_models(&state, user.uid).await?,
        "model/create" => create_model(&state, user.uid, payload).await?,
        "model/test" => test_model_draft(&state, user.uid, payload).await?,
        "model/preview" => preview_model_draft(&state, user.uid, payload).await?,
        "model/import" => import_models(&state, user.uid, payload).await?,
        "model/update" => update_model(&state, user.uid, payload).await?,
        "model/delete" => delete_model(&state, user.uid, id(&payload)?).await?,
        "key/list" => list_keys(&state, user.uid).await?,
        "key/own-list" => list_own_keys(&state, user.uid).await?,
        "key/reveal" => reveal_key(&state, user.uid, id(&payload)?).await?,
        "key/create" => create_key(&state, user.uid, payload).await?,
        "key/update" => update_key(&state, user.uid, payload).await?,
        "key/delete" => delete_key(&state, user.uid, id(&payload)?).await?,
        "client-connect/create" => crate::client_pair::create(&state, user.uid, &payload).await?,
        "integration/list" => crate::client_pair::list_profiles(&state, user.uid).await?,
        "integration/detail" => {
            crate::client_pair::profile_detail(&state, user.uid, &payload).await?
        }
        "integration/upsert" => {
            crate::client_pair::upsert_profile(&state, user.uid, &payload).await?
        }
        "integration/delete" => {
            crate::client_pair::delete_profile(&state, user.uid, &payload).await?
        }
        "integration/sync" => crate::client_pair::sync_profile(&state, user.uid, &payload).await?,
        "integration/test" => crate::client_pair::test_profile(&state, user.uid, &payload).await?,
        "integration/reorder" => {
            crate::client_pair::reorder_profiles(&state, user.uid, &payload).await?
        }
        "logs" => list_logs(&state, user.uid, &payload).await?,
        "stats" => stats(&state, user.uid, &payload).await?,
        "sub2api/accounts/api-key" => {
            crate::sub2api::create_api_key_account(&state, user.uid, &payload).await?
        }
        "sub2api/oauth/openai/start" => serde_json::to_value(
            state
                .oauth_sessions
                .start_openai(
                    user.uid,
                    payload.get("redirect_uri").and_then(Value::as_str),
                )
                .await?,
        )?,
        "sub2api/oauth/openai/complete" => {
            crate::sub2api::complete_openai_oauth(&state, user.uid, &payload).await?
        }
        "sub2api/oauth/anthropic/start"
        | "sub2api/oauth/gemini/start"
        | "sub2api/oauth/grok/start"
        | "sub2api/oauth/antigravity/start" => {
            let provider = path.split('/').nth(2).unwrap_or_default();
            serde_json::to_value(
                state
                    .oauth_sessions
                    .start_provider(user.uid, provider, &payload)
                    .await?,
            )?
        }
        "sub2api/oauth/anthropic/complete"
        | "sub2api/oauth/gemini/complete"
        | "sub2api/oauth/grok/complete"
        | "sub2api/oauth/antigravity/complete" => {
            let provider = path.split('/').nth(2).unwrap_or_default();
            crate::sub2api::complete_provider_oauth(&state, user.uid, provider, &payload).await?
        }
        "sub2api/oauth/anthropic/import"
        | "sub2api/oauth/gemini/import"
        | "sub2api/oauth/grok/import"
        | "sub2api/oauth/antigravity/import" => {
            let provider = path.split('/').nth(2).unwrap_or_default();
            crate::sub2api::import_oauth_token(&state, user.uid, provider, &payload).await?
        }
        _ => {
            return Err(AppError::NotFound(format!(
                "用户接口不存在: /console/{path}"
            )))
        }
    };
    Ok(Json(Envelope::ok(data)))
}

/// 普通用户可用的控制台端点白名单。
///
/// 这里必须是白名单而不是黑名单：以前用前缀排除，任何新增的管理端点
/// 只要忘了加前缀，就会直接对普通用户开放。现在新端点默认仅管理员可用，
/// 要放开必须显式加进这个列表。
const USER_CONSOLE_ENDPOINTS: &[&str] = &[
    "guide/get",
    "profile",
    "logs",
    "stats",
    "model/available",
    "client-connect/create",
    "key/list",
    "key/own-list",
    "key/reveal",
    "key/create",
    "key/update",
    "key/delete",
    "key/limits",
    "key/limits/update",
    "integration/list",
    "integration/detail",
    "integration/upsert",
    "integration/delete",
    "integration/sync",
    "integration/test",
    "integration/reorder",
];

fn authorize_console_role(role: &str, path: &str) -> AppResult<()> {
    if role == "admin" || USER_CONSOLE_ENDPOINTS.contains(&path) {
        return Ok(());
    }
    Err(AppError::Forbidden("此功能仅限管理员".into()))
}

pub(crate) fn normalize_email(value: &str) -> AppResult<String> {
    let email = value.trim().to_lowercase();
    let parts: Vec<_> = email.split('@').collect();
    if email.len() < 5
        || email.len() > 254
        || parts.len() != 2
        || parts.iter().any(|part| part.is_empty())
        || email.chars().any(char::is_whitespace)
        || email.chars().any(char::is_control)
    {
        return Err(AppError::FormFailed {
            reason: "email_invalid",
            message: "请输入有效的邮箱地址，例如 name@example.com。",
            field: "email",
        });
    }
    Ok(email)
}

fn validate_password(value: &str) -> AppResult<()> {
    if value.len() < 8 || value.len() > 128 {
        return Err(AppError::BadRequest("密码长度必须为 8 到 128 位".into()));
    }
    Ok(())
}

fn id(payload: &Value) -> AppResult<i64> {
    payload
        .get("id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("id 不能为空".into()))
}

async fn profile(state: &AppState, uid: i64) -> AppResult<Value> {
    let row = sqlx::query(
        "SELECT id,email,display_name,avatar_url,role,status,created_at FROM users WHERE id=?",
    )
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Unauthorized)?;
    Ok(json!({
        "uid": row.get::<i64,_>("id"),
        "email": row.get::<String,_>("email"),
        "display_name": row.get::<String,_>("display_name"),
        "nickname": row.get::<String,_>("display_name"),
        "avatar_url": row.get::<String,_>("avatar_url"),
        "role": row.get::<String,_>("role"),
        "status": row.get::<i64,_>("status"),
        "create_time": row.get::<i64,_>("created_at")
    }))
}

async fn update_account_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let row = sqlx::query("SELECT display_name,avatar_url FROM users WHERE id=?")
        .bind(uid)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let display_name = if payload.get("display_name").is_some() {
        let value = string(payload, "display_name");
        let length = value.chars().count();
        if !(1..=64).contains(&length) {
            return Err(AppError::BadRequest(
                "显示名称长度必须为 1 到 64 个字符".into(),
            ));
        }
        value
    } else {
        row.get("display_name")
    };
    let avatar_url = if let Some(value) = payload.get("avatar_url") {
        let value = value
            .as_str()
            .ok_or_else(|| AppError::BadRequest("头像数据无效".into()))?;
        crate::image_storage::persist_avatar(
            state,
            uid,
            crate::image_storage::AvatarPurpose::User,
            value,
        )
        .await?
    } else {
        row.get("avatar_url")
    };

    sqlx::query("UPDATE users SET display_name=?,avatar_url=?,updated_at=? WHERE id=?")
        .bind(&display_name)
        .bind(&avatar_url)
        .bind(store::now())
        .bind(uid)
        .execute(&state.pool)
        .await?;
    profile(state, uid).await
}

async fn update_account_password(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let current_password = raw_string(payload, "current_password");
    let new_password = raw_string(payload, "new_password");
    validate_password(&new_password)?;
    let password_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?")
        .bind(uid)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if current_password.len() > 128 {
        return Err(AppError::BadRequest("当前密码不正确".into()));
    }
    let checked_hash = password_hash.clone();
    let new_hash = tokio::task::spawn_blocking(move || {
        if !secrets::verify_password(&current_password, &checked_hash) {
            return Err(AppError::BadRequest("当前密码不正确".into()));
        }
        if current_password == new_password {
            return Err(AppError::BadRequest("新密码不能与当前密码相同".into()));
        }
        secrets::hash_password(&new_password)
    })
    .await
    .map_err(|_| AppError::Internal("密码处理失败".into()))??;
    let changed=sqlx::query("UPDATE users SET password_hash=?,session_version=session_version+1,updated_at=? WHERE id=? AND password_hash=?")
        .bind(new_hash).bind(store::now()).bind(uid).bind(password_hash).execute(&state.pool).await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::BadRequest(
            "密码已发生变化，请重新登录后再修改".into(),
        ));
    }
    Ok(json!({}))
}

fn raw_string(payload: &Value, name: &str) -> String {
    payload
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn string(payload: &Value, name: &str) -> String {
    payload
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

pub(crate) fn validate_base_url(value: &str) -> AppResult<String> {
    let url =
        reqwest::Url::parse(value).map_err(|_| AppError::BadRequest("Base URL 无效".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::BadRequest(
            "Base URL 必须是没有内嵌凭据的 HTTP(S) 地址".into(),
        ));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

async fn list_providers(state: &AppState, uid: i64) -> AppResult<Value> {
    let rows = sqlx::query("SELECT p.id,p.name,p.provider_type,p.base_url,p.avatar_url,CASE WHEN p.provider_type<>'sub2api' THEN COALESCE((SELECT encrypted_api_key FROM provider_keys k WHERE k.provider_id=p.id AND k.encrypted_api_key<>'' LIMIT 1),'') ELSE p.encrypted_api_key END AS encrypted_api_key,p.enabled,p.created_at,p.updated_at,m.platform FROM user_providers p LEFT JOIN sub2_managed_pools m ON m.provider_id=p.id AND m.user_id=p.user_id WHERE p.user_id=? ORDER BY p.id DESC")
        .bind(uid).fetch_all(&state.pool).await?;
    let list = rows
        .into_iter()
        .map(|row| {
            let platform = row.get::<Option<String>, _>("platform");
            json!({
                "id":row.get::<i64,_>("id"), "name":row.get::<String,_>("name"),
                "provider_type":row.get::<String,_>("provider_type"), "base_url":row.get::<String,_>("base_url"), "avatar_url":row.get::<String,_>("avatar_url"),
                "has_api_key":!row.get::<String,_>("encrypted_api_key").is_empty(), "enabled":row.get::<i64,_>("enabled"),
                "managed":platform.is_some(), "platform":platform,
                "create_time":row.get::<i64,_>("created_at"), "update_time":row.get::<i64,_>("updated_at")
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

async fn provider_is_managed(state: &AppState, uid: i64, provider: i64) -> AppResult<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sub2_managed_pools m WHERE m.provider_id=p.id) FROM user_providers p WHERE p.id=? AND p.user_id=?").bind(provider).bind(uid).fetch_optional(&state.pool).await?.ok_or_else(||AppError::NotFound("供应商不存在".into()))
}
fn request_limits(payload: &Value) -> AppResult<Option<crate::gateway_limits::Limits>> {
    payload
        .get("request_limits")
        .map(|value| {
            let limits: crate::gateway_limits::Limits = serde_json::from_value(value.clone())
                .map_err(|_| AppError::BadRequest("请求限制须填写有效的整数".into()))?;
            limits.validate()?;
            Ok(limits)
        })
        .transpose()
}

async fn save_request_limits(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    kind: &str,
    id: i64,
    limits: Option<crate::gateway_limits::Limits>,
) -> AppResult<()> {
    let Some(limits) = limits else {
        return Ok(());
    };
    let table = match kind {
        "provider" => "provider_limits",
        "model" => "model_limits",
        "key" => "key_limits",
        "user" => "user_limits",
        _ => return Err(AppError::BadRequest("资源类型无效".into())),
    };
    sqlx::query(&format!("INSERT INTO {table}(id,max_concurrency,rpm) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET max_concurrency=excluded.max_concurrency,rpm=excluded.rpm"))
        .bind(id).bind(limits.max_concurrency).bind(limits.rpm).execute(&mut **tx).await?;
    Ok(())
}

async fn resource_limits(
    state: &AppState,
    uid: i64,
    kind: &str,
    id: i64,
    update: bool,
    payload: &Value,
) -> AppResult<Value> {
    let (resources, table) = match kind {
        "provider" => ("user_providers", "provider_limits"),
        "model" => ("user_models", "model_limits"),
        "key" => ("virtual_keys", "key_limits"),
        "user" => ("users", "user_limits"),
        _ => return Err(AppError::BadRequest("资源类型无效".into())),
    };
    let owned: bool = if kind == "user" {
        let admin: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=? AND role='admin' AND status=1)",
        )
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
        if !admin {
            return Err(AppError::Forbidden("用户请求限制仅限管理员配置".into()));
        }
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        let uid = if kind == "key" {
            key_owner_for_actor(state, uid, id).await?
        } else {
            uid
        };
        sqlx::query_scalar(&format!(
            "SELECT EXISTS(SELECT 1 FROM {resources} WHERE id=? AND user_id=?)"
        ))
        .bind(id)
        .bind(uid)
        .fetch_one(&state.pool)
        .await?
    };
    if !owned {
        return Err(AppError::NotFound("资源不存在".into()));
    }
    if update {
        let limits: crate::gateway_limits::Limits = serde_json::from_value(payload.clone())
            .map_err(|_| AppError::BadRequest("限制参数格式无效".into()))?;
        limits.validate()?;
        sqlx::query(&format!("INSERT INTO {table}(id,max_concurrency,rpm) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET max_concurrency=excluded.max_concurrency,rpm=excluded.rpm")).bind(id).bind(limits.max_concurrency).bind(limits.rpm).execute(&state.pool).await?;
    }
    Ok(serde_json::to_value(
        crate::gateway_limits::read(state, kind, id).await?,
    )?)
}

async fn managed_group_id(state: &AppState, uid: i64, provider_id: i64) -> AppResult<i64> {
    sqlx::query_scalar("SELECT group_id FROM sub2_managed_pools WHERE provider_id=? AND user_id=?")
        .bind(provider_id)
        .bind(uid)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("内置供应商不存在".into()))
}

async fn list_provider_accounts(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("provider_id 不能为空".into()))?;
    if !provider_is_managed(state, uid, provider_id).await? {
        return crate::direct_provider::list(state, provider_id).await;
    }
    let group_id = managed_group_id(state, uid, provider_id).await?;
    let list = Sub2Repository::new(state.sub2_pool.clone())
        .managed_accounts(group_id, &state.config.encryption_key)
        .await?;
    Ok(json!({"list":list,"total":list.len()}))
}

fn account_priority_updates(payload: &Value) -> AppResult<Vec<(i64, i32)>> {
    let items = payload
        .get("accounts")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty() && items.len() <= 100)
        .ok_or_else(|| AppError::BadRequest("请选择要更新的账户".into()))?;
    let mut seen = HashSet::new();
    let mut updates = Vec::with_capacity(items.len());
    for item in items {
        let account_id = item
            .get("id")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| AppError::BadRequest("账户标识无效".into()))?;
        let priority = item
            .get("priority")
            .and_then(Value::as_i64)
            .filter(|value| (1..=999).contains(value))
            .ok_or_else(|| AppError::BadRequest("优先级必须是 1 到 999 的整数".into()))?;
        if !seen.insert(account_id) {
            return Err(AppError::BadRequest("账户不能重复提交".into()));
        }
        updates.push((account_id, priority as i32));
    }
    Ok(updates)
}

async fn update_provider_account_priorities(
    state: &AppState,
    uid: i64,
    payload: &Value,
) -> AppResult<Value> {
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("provider_id 不能为空".into()))?;
    let updates = account_priority_updates(payload)?;
    let group_id = managed_group_id(state, uid, provider_id).await?;
    Sub2Repository::new(state.sub2_pool.clone())
        .update_managed_account_priorities(group_id, &updates)
        .await?;
    Ok(json!({"updated":updates.len()}))
}

async fn update_provider_account_config(
    state: &AppState,
    uid: i64,
    payload: &Value,
) -> AppResult<Value> {
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("provider_id 不能为空".into()))?;
    let account_id = payload
        .get("account_id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("account_id 不能为空".into()))?;
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let base_url = payload.get("base_url").and_then(Value::as_str);
    let api_key = payload.get("api_key").and_then(Value::as_str);
    if !provider_is_managed(state, uid, provider_id).await? {
        return crate::direct_provider::update(state, provider_id, payload).await;
    }
    let group_id = managed_group_id(state, uid, provider_id).await?;
    Sub2Repository::new(state.sub2_pool.clone())
        .update_managed_account_config(
            group_id,
            account_id,
            name,
            base_url,
            api_key,
            &state.config.encryption_key,
        )
        .await?;
    Ok(json!({"updated":true}))
}

async fn create_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let limits = request_limits(&payload)?;
    let name = string(&payload, "name");
    let provider_type = string(&payload, "provider_type");
    // 托管供应商的地址不参与路由，由账户引擎自己决定，不接受客户端提供的值：
    // 客户端以前会传部署拓扑（容器名 + 端口），换个部署就写进了一条无效地址。
    let base_url = if provider_type == "sub2api" {
        crate::store::MANAGED_PROVIDER_BASE_URL.to_owned()
    } else {
        validate_base_url(&string(&payload, "base_url"))?
    };
    let api_key = string(&payload, "api_key");
    let avatar_url = payload
        .get("avatar_url")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| AppError::BadRequest("头像数据无效".into()))
        })
        .transpose()?
        .unwrap_or_default()
        .to_owned();
    if name.is_empty() || provider_type.is_empty() {
        return Err(AppError::BadRequest("名称和供应商类型不能为空".into()));
    }
    if !avatar_url.is_empty() && provider_type != "openai-compatible" {
        return Err(AppError::BadRequest("只有自定义供应商可以设置头像".into()));
    }
    let encrypted = if api_key.is_empty() {
        String::new()
    } else {
        secrets::encrypt(&state.config.encryption_key, &api_key)?
    };
    let avatar_url = crate::image_storage::persist_avatar(
        state,
        uid,
        crate::image_storage::AvatarPurpose::Provider,
        &avatar_url,
    )
    .await?;
    let timestamp = store::now();
    let mut transaction = state.pool.begin().await?;
    let result = sqlx::query("INSERT INTO user_providers(user_id,name,provider_type,base_url,encrypted_api_key,avatar_url,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,1,?,?)")
        .bind(uid).bind(&name).bind(&provider_type).bind(&base_url).bind(&encrypted).bind(&avatar_url).bind(timestamp).bind(timestamp)
        .execute(&mut *transaction).await.map_err(map_unique("供应商名称已存在"))?;
    if provider_type != "sub2api" {
        sqlx::query("INSERT INTO provider_keys(provider_id,name,encrypted_api_key,base_url) VALUES(?,'默认 Key',?,?)").bind(result.last_insert_rowid()).bind(&encrypted).bind(&base_url).execute(&mut *transaction).await?;
    }
    if provider_type != "sub2api" {
        sqlx::query("UPDATE user_providers SET encrypted_api_key='' WHERE id=?")
            .bind(result.last_insert_rowid())
            .execute(&mut *transaction)
            .await?;
    }
    save_request_limits(
        &mut transaction,
        "provider",
        result.last_insert_rowid(),
        limits,
    )
    .await?;
    transaction.commit().await?;
    Ok(
        json!({"id":result.last_insert_rowid(),"name":name,"provider_type":provider_type,"base_url":base_url,"avatar_url":avatar_url,"has_api_key":!api_key.is_empty(),"managed":false,"enabled":1,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let limits = request_limits(&payload)?;
    let provider_id = id(&payload)?;
    let row = sqlx::query("SELECT name,provider_type,base_url,avatar_url,encrypted_api_key,enabled,created_at FROM user_providers WHERE id=? AND user_id=?")
        .bind(provider_id).bind(uid).fetch_optional(&state.pool).await?.ok_or_else(|| AppError::NotFound("供应商不存在".into()))?;
    let name = nonempty_or(&payload, "name", row.get("name"));
    let provider_type = nonempty_or(&payload, "provider_type", row.get("provider_type"));
    let base_url = if payload.get("base_url").is_some() {
        validate_base_url(&string(&payload, "base_url"))?
    } else {
        row.get("base_url")
    };
    let encrypted = if payload.get("api_key").is_some() {
        let api_key = string(&payload, "api_key");
        if api_key.is_empty() {
            String::new()
        } else {
            secrets::encrypt(&state.config.encryption_key, &api_key)?
        }
    } else {
        row.get("encrypted_api_key")
    };
    let enabled = payload
        .get("enabled")
        .or_else(|| payload.get("is_enabled"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("enabled"));
    let managed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sub2_managed_pools WHERE provider_id=? AND user_id=?)",
    )
    .bind(provider_id)
    .bind(uid)
    .fetch_one(&state.pool)
    .await?;
    let custom_managed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sub2_managed_pools WHERE provider_id=? AND user_id=? AND platform='custom')")
        .bind(provider_id).bind(uid).fetch_one(&state.pool).await?;
    let avatar_url = if payload.get("avatar_url").is_some() {
        let value = payload["avatar_url"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("头像数据无效".into()))?
            .to_owned();
        if !value.is_empty() && provider_type != "openai-compatible" && !custom_managed {
            return Err(AppError::BadRequest("只有自定义供应商可以设置头像".into()));
        }
        value
    } else {
        row.get("avatar_url")
    };
    if managed
        && (provider_type != row.get::<String, _>("provider_type")
            || base_url != row.get::<String, _>("base_url")
            || payload
                .get("api_key")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()))
    {
        return Err(AppError::BadRequest(
            "内置账户供应商的连接地址和内部密钥由账户池管理，只能修改名称或启停状态".into(),
        ));
    }
    if !managed && provider_type != "sub2api" && payload.get("api_key").is_some() {
        return Err(AppError::BadRequest(
            "请在账户管理中选择需要更新的 Key".into(),
        ));
    }
    let avatar_url = crate::image_storage::persist_avatar(
        state,
        uid,
        crate::image_storage::AvatarPurpose::Provider,
        &avatar_url,
    )
    .await?;
    let timestamp = store::now();
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE user_providers SET name=?,provider_type=?,base_url=?,encrypted_api_key=?,avatar_url=?,enabled=?,updated_at=? WHERE id=? AND user_id=?")
        .bind(&name).bind(&provider_type).bind(&base_url).bind(&encrypted).bind(&avatar_url).bind(enabled).bind(timestamp).bind(provider_id).bind(uid)
        .execute(&mut *transaction).await.map_err(map_unique("供应商名称已存在"))?;
    save_request_limits(&mut transaction, "provider", provider_id, limits).await?;
    transaction.commit().await?;
    Ok(
        json!({"id":provider_id,"name":name,"provider_type":provider_type,"base_url":base_url,"avatar_url":avatar_url,"has_api_key":!encrypted.is_empty(),"managed":managed,"enabled":enabled,"create_time":row.get::<i64,_>("created_at"),"update_time":timestamp}),
    )
}

fn nonempty_or(payload: &Value, name: &str, fallback: String) -> String {
    let value = string(payload, name);
    if value.is_empty() {
        fallback
    } else {
        value
    }
}
fn map_unique(message: &'static str) -> impl FnOnce(sqlx::Error) -> AppError {
    move |error| {
        if error.to_string().contains("UNIQUE") {
            AppError::BadRequest(message.into())
        } else {
            AppError::Sql(error)
        }
    }
}

async fn delete_provider(state: &AppState, uid: i64, provider_id: i64) -> AppResult<Value> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_providers WHERE id=? AND user_id=?)")
            .bind(provider_id)
            .bind(uid)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(AppError::NotFound("供应商不存在".into()));
    }
    let managed = sqlx::query(
        "SELECT group_id,api_key_id FROM sub2_managed_pools WHERE provider_id=? AND user_id=?",
    )
    .bind(provider_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(managed) = managed {
        Sub2Repository::new(state.sub2_pool.clone())
            .delete_managed_pool(managed.get("group_id"), managed.get("api_key_id"))
            .await?;
    }
    let mut transaction = state.pool.begin().await?;
    sqlx::query("DELETE FROM user_models WHERE provider_id=? AND user_id=?")
        .bind(provider_id)
        .bind(uid)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM sub2_managed_pools WHERE provider_id=? AND user_id=?")
        .bind(provider_id)
        .bind(uid)
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query("DELETE FROM user_providers WHERE id=? AND user_id=?")
        .bind(provider_id)
        .bind(uid)
        .execute(&mut *transaction)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("供应商不存在".into()));
    }
    transaction.commit().await?;
    Ok(json!({}))
}

pub async fn reconcile_managed_pools(state: &AppState) -> AppResult<u64> {
    let rows = sqlx::query(
        "SELECT p.id,p.user_id,p.encrypted_api_key FROM user_providers p LEFT JOIN sub2_managed_pools m ON m.provider_id=p.id WHERE p.provider_type='sub2api' AND m.provider_id IS NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let mut inserted = 0;
    for row in rows {
        let encrypted: String = row.get("encrypted_api_key");
        if encrypted.is_empty() {
            continue;
        }
        let api_key = secrets::decrypt(&state.config.encryption_key, &encrypted)?;
        let Ok(route) = repository.resolve_engine_key(&api_key).await else {
            continue;
        };
        let timestamp = store::now();
        inserted += sqlx::query(
            "INSERT OR IGNORE INTO sub2_managed_pools(provider_id,user_id,platform,group_id,api_key_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
        )
        .bind(row.get::<i64, _>("id"))
        .bind(row.get::<i64, _>("user_id"))
        .bind(route.platform)
        .bind(route.group_id)
        .bind(route.api_key_id)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&state.pool)
        .await?
        .rows_affected();
    }
    Ok(inserted)
}

async fn provider_models(state: &AppState, uid: i64, provider_id: i64) -> AppResult<Value> {
    let provider = load_provider(state, uid, provider_id).await?;
    if provider.provider_type != "sub2api"
        && crate::direct_provider::keys(state, provider_id)
            .await?
            .iter()
            .all(|key| key.cooldown_until > store::now())
    {
        return Err(AppError::BadRequest(
            "没有可用 Key，请先添加凭证或等待冷却结束".into(),
        ));
    }

    let mut models = if provider.provider_type == "sub2api" {
        crate::sub2_gateway::discover_models(state, &provider.api_key).await?
    } else {
        upstream_models(state, &provider).await?
    };
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(AppError::BadRequest("上游未返回可导入的模型".into()));
    }
    Ok(json!({"models":models,"total":models.len()}))
}

async fn upstream_models(state: &AppState, provider: &ProviderSecret) -> AppResult<Vec<String>> {
    crate::upstream_probe::models(state, provider).await
}

async fn test_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = id(&payload)?;
    provider_is_managed(state, uid, provider_id).await?;
    let kind: String =
        sqlx::query_scalar("SELECT provider_type FROM user_providers WHERE id=? AND user_id=?")
            .bind(provider_id)
            .bind(uid)
            .fetch_one(&state.pool)
            .await?;
    if kind == "sub2api" {
        let provider = load_provider(state, uid, provider_id).await?;
        return crate::sub2_gateway::test_connection(state, &provider.api_key).await;
    }
    // An explicit diagnostic must be able to re-check cooling keys without changing scheduling state.
    let keys = crate::direct_provider::keys(state, provider_id).await?;
    if keys.is_empty() {
        return Err(AppError::BadRequest("请先添加 Key 或无认证连接".into()));
    }
    use futures_util::{stream, StreamExt};
    let names: Vec<_> = keys.iter().map(|key| key.name.clone()).collect();
    let model = payload["model"].as_str();
    let config = crate::upstream_probe::test_config(&payload)?;
    let image_probe = keys
        .iter()
        .any(|key| crate::upstream_probe::tokenhub_image_model(&state.config.tokenhub_image_hosts, &state.config.tokenhub_image_models, &key.base_url, model));
    let pending = stream::iter(keys.into_iter().map(|key| {
        let config = config.clone();
        let kind = kind.clone();
        async move {
        let checked: AppResult<Value> = async {
            let secret = if key.encrypted_api_key.is_empty() { String::new() } else {
                secrets::decrypt(&state.config.encryption_key, &key.encrypted_api_key)?
            };
            let report = crate::upstream_probe::probe_configured(state, &key.base_url, &secret, &kind, model, config.as_ref()).await?;
            Ok(serde_json::to_value(report)?)
        }.await;
        let mut value = checked.unwrap_or_else(|_| json!({"ok":false,"outcome":"error","status_code":0,"message":"无法测试此连接，请检查地址与凭证配置。"}));
        value["name"] = json!(key.name);
        value
    }})).buffered(8);
    tokio::pin!(pending);
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(if image_probe { 130 } else { 30 });
    let mut reports = Vec::new();
    while let Ok(Some(report)) = tokio::time::timeout_at(deadline, pending.next()).await {
        reports.push(report);
    }
    for name in names.into_iter().skip(reports.len()) {
        reports.push(json!({"name":name,"ok":false,"outcome":"warning","status_code":0,"message":"本次批量测试已达到等待上限，请单独测试此连接。"}));
    }
    let successes = reports.iter().filter(|r| r["ok"] == true).count();
    let all = successes == reports.len();
    let outcome = if all {
        "success"
    } else if successes > 0 || reports.iter().any(|r| r["outcome"] == "warning") {
        "warning"
    } else {
        "error"
    };
    Ok(
        json!({"ok":all,"outcome":outcome,"message":if reports.len()==1{reports[0]["message"].as_str().unwrap_or_default().to_owned()}else{format!("已检查 {} 个 Key，{} 个通过{}验证",reports.len(),successes,if model.is_some_and(|m| !m.trim().is_empty()) {"模型调用"} else {"模型列表"})},"status_code":reports[0]["status_code"],"checks":reports}),
    )
}

pub struct ProviderSecret {
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
}

fn draft_account_matches(key: &crate::direct_provider::Key, model: &str) -> bool {
    if key.priority <= 0 || key.cooldown_until > store::now() {
        return false;
    }
    let Ok(allowed) = parse_key_models(&key.allowed_models) else {
        return false;
    };
    model.is_empty()
        || allowed.is_empty()
        || allowed.iter().any(|name| name == "*" || name == model)
}

// Resolves the destination through the same implementation as forwarding. No upstream I/O.
async fn preview_model_draft(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = payload["provider_id"]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::BadRequest("请选择供应商".into()))?;
    let provider = load_provider(state, uid, provider_id).await?;
    let path = crate::model_request_path::for_provider(&provider.provider_type, &payload, "")?;
    let config =
        crate::model_api::ApiConfig::for_provider(&provider.provider_type, &payload, "{}", &path)?;
    let endpoint = config.api_type.endpoint().unwrap_or("/v1/chat/completions");
    if provider.provider_type == "sub2api" {
        return Ok(json!({"managed":true,"upstream_url":null,"available_accounts":null}));
    }
    let model = string(&payload, "upstream_model");
    let accounts = crate::direct_provider::keys(state, provider_id).await?;
    Ok(json!({"managed":false,
        "upstream_url":config.upstream_target(&provider.base_url,endpoint,&path)?.as_str(),
        "available_accounts":accounts.iter().filter(|key| draft_account_matches(key,&model)).count()}))
}

async fn test_model_draft(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = payload["provider_id"]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::BadRequest("请选择供应商".into()))?;
    let provider = load_provider(state, uid, provider_id).await?;
    let model = string(&payload, "upstream_model");
    if model.is_empty() || model.len() > 200 {
        return Err(AppError::BadRequest(
            "请填写 1–200 字符的上游模型名称".into(),
        ));
    }
    let path = crate::model_request_path::for_provider(&provider.provider_type, &payload, "")?;
    let config =
        crate::model_api::ApiConfig::for_provider(&provider.provider_type, &payload, "{}", &path)?;
    let kind = match config.upstream_type() {
        crate::model_api::ApiType::Auto => crate::model_api::ApiType::ChatCompletions,
        kind => kind.clone(),
    };
    let body = crate::upstream_probe::route_test_payload(&kind, &model)?;
    if provider.provider_type == "sub2api" {
        let started = std::time::Instant::now();
        let checked = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let response = crate::sub2_gateway::forward(
                state,
                &provider.api_key,
                axum::http::Method::POST,
                kind.endpoint().unwrap(),
                &HeaderMap::new(),
                &body,
            )
            .await?;
            let status = response.status().as_u16();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .map_err(|_| AppError::BadRequest("测试响应过大或未完整返回".into()))?;
            Ok::<_, AppError>(crate::upstream_probe::route_test_result(
                status, &bytes, &kind, started,
            ))
        })
        .await;
        let report = match checked {
            Ok(Ok(report)) => report,
            _ => crate::upstream_probe::route_test_failure(
                started,
                "授权池模型测试未完成，请检查账户状态或稍后重试。",
            ),
        };
        return Ok(serde_json::to_value(report)?);
    }
    // Explicit testing checks one credential and never mutates dispatch or cooldown state.
    let keys = crate::direct_provider::keys(state, provider_id).await?;
    let key = keys
        .iter()
        .find(|key| draft_account_matches(key, &model))
        .ok_or_else(|| {
            AppError::BadRequest(
                "没有可测试的账户：请检查账户是否参与调度、冷却状态和允许调用的上游模型".into(),
            )
        })?;
    let secret = if key.encrypted_api_key.is_empty() {
        String::new()
    } else {
        secrets::decrypt(&state.config.encryption_key, &key.encrypted_api_key)?
    };
    let report = crate::upstream_probe::probe_route(
        &state.client,
        &key.base_url,
        &secret,
        &config,
        &model,
        &path,
    )
    .await?;
    let mut value = serde_json::to_value(report)?;
    value["account_name"] = json!(key.name);
    Ok(value)
}
pub async fn load_provider(
    state: &AppState,
    uid: i64,
    provider_id: i64,
) -> AppResult<ProviderSecret> {
    let row=sqlx::query("SELECT provider_type,base_url,encrypted_api_key,enabled FROM user_providers WHERE id=? AND user_id=?").bind(provider_id).bind(uid).fetch_optional(&state.pool).await?.ok_or_else(||AppError::NotFound("供应商不存在".into()))?;
    if row.get::<i64, _>("enabled") != 1 {
        return Err(AppError::BadRequest("供应商已停用".into()));
    }
    if row.get::<String, _>("provider_type") != "sub2api" {
        if let Some(key) = crate::direct_provider::keys(state, provider_id)
            .await?
            .into_iter()
            .find(|k| k.priority > 0 && k.cooldown_until <= store::now())
        {
            return Ok(ProviderSecret {
                provider_type: row.get("provider_type"),
                base_url: key.base_url,
                api_key: if key.encrypted_api_key.is_empty() {
                    String::new()
                } else {
                    secrets::decrypt(&state.config.encryption_key, &key.encrypted_api_key)?
                },
            });
        }
        // Forwarding reports unavailable keys; model/test discovery must not resurrect legacy credentials.
        return Ok(ProviderSecret {
            provider_type: row.get("provider_type"),
            base_url: row.get("base_url"),
            api_key: String::new(),
        });
    }
    Ok(ProviderSecret {
        provider_type: row.get("provider_type"),
        base_url: row.get("base_url"),
        api_key: {
            let encrypted: String = row.get("encrypted_api_key");
            if encrypted.is_empty() {
                String::new()
            } else {
                secrets::decrypt(&state.config.encryption_key, &encrypted)?
            }
        },
    })
}

async fn list_models(state: &AppState, uid: i64) -> AppResult<Value> {
    let rows=sqlx::query("SELECT m.id,m.provider_id,m.alias,m.upstream_model,m.request_path,m.api_config,m.billing_config,m.capabilities,m.input_price_per_million,m.output_price_per_million,m.enabled,m.created_at,m.updated_at,p.name provider_name,p.provider_type FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id WHERE m.user_id=? ORDER BY m.id DESC").bind(uid).fetch_all(&state.pool).await?;
    let list=rows.into_iter().map(|row|json!({"id":row.get::<i64,_>("id"),"provider_id":row.get::<i64,_>("provider_id"),"provider_name":row.get::<String,_>("provider_name"),"input_price_per_million":row.get::<Option<f64>,_>("input_price_per_million"),"output_price_per_million":row.get::<Option<f64>,_>("output_price_per_million"),"provider_type":row.get::<String,_>("provider_type"),"alias":row.get::<String,_>("alias"),"upstream_model":row.get::<String,_>("upstream_model"),"request_path":row.get::<String,_>("request_path"),"api_config":serde_json::from_str::<Value>(row.get("api_config")).unwrap_or(json!({})),"billing_config":serde_json::from_str::<Value>(row.get("billing_config")).unwrap_or(json!({})),"capabilities":serde_json::from_str::<Value>(row.get("capabilities")).unwrap_or(json!(["chat"])),"enabled":row.get::<i64,_>("enabled"),"create_time":row.get::<i64,_>("created_at"),"update_time":row.get::<i64,_>("updated_at")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

pub(crate) async fn provision_sub2api_provider(
    state: &AppState,
    uid: i64,
    provider_name: &str,
    api_key: &str,
) -> AppResult<Value> {
    let encrypted = secrets::encrypt(&state.config.encryption_key, api_key)?;
    let timestamp = store::now();
    let base_name = provider_name.trim();
    let duplicate_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_providers WHERE user_id=? AND name LIKE ?")
            .bind(uid)
            .bind(format!("{base_name}%"))
            .fetch_one(&state.pool)
            .await?;
    let name = if duplicate_count == 0 {
        base_name.to_owned()
    } else {
        format!("{base_name} {}", duplicate_count + 1)
    };
    let provider = sqlx::query("INSERT INTO user_providers(user_id,name,provider_type,base_url,encrypted_api_key,enabled,created_at,updated_at) VALUES(?,?, 'sub2api',?,?,1,?,?)")
        .bind(uid)
        .bind(name)
        .bind(crate::store::MANAGED_PROVIDER_BASE_URL)
        .bind(encrypted)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&state.pool)
        .await
        .map_err(map_unique("供应商名称已存在"))?;
    let provider_id = provider.last_insert_rowid();
    Ok(json!({"provider_id":provider_id}))
}

async fn import_models(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::BadRequest("provider_id 不能为空".into()))?;
    load_provider(state, uid, provider_id).await?;
    let requested = payload
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("请选择要导入的模型".into()))?;
    if requested.is_empty() || requested.len() > 200 {
        return Err(AppError::BadRequest("每次可导入 1 到 200 个模型".into()));
    }
    let mut existing =
        sqlx::query_scalar::<_, String>("SELECT alias FROM user_models WHERE user_id=?")
            .bind(uid)
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
    let timestamp = store::now();
    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut transaction = state.pool.begin().await?;
    for value in requested {
        let model = value.as_str().unwrap_or_default().trim();
        if model.is_empty() || model.len() > 200 {
            return Err(AppError::BadRequest("模型名称无效".into()));
        }
        if !existing.insert(model.to_owned()) {
            skipped.push(model.to_owned());
            continue;
        }
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?)")
            .bind(uid)
            .bind(provider_id)
            .bind(model)
            .bind(model)
            .bind("[\"chat\"]")
            .bind(timestamp)
            .bind(timestamp)
            .execute(&mut *transaction)
            .await?;
        imported.push(model.to_owned());
    }
    transaction.commit().await?;
    Ok(
        json!({"imported":imported,"skipped":skipped,"imported_count":imported.len(),"skipped_count":skipped.len()}),
    )
}

fn model_price(payload: &Value, field: &str, previous: Option<f64>) -> AppResult<Option<f64>> {
    match payload.get(field) {
        None => Ok(previous),
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|v| crate::gateway_billing::price_is_supported(*v))
            .map(Some)
            .ok_or_else(|| {
                AppError::BadRequest(
                    "模型价格须为 0–1000000000 之间的数字，精度最多 28 位小数，单位为美元／百万 Token".into(),
                )
            }),
    }
}

/// 拒绝无法识别的顶层字段。
/// 没有这一步时，把 `api_type` 误放在顶层会被静默忽略并落成 `auto`
/// —— 用户以为限制了协议，实际把路由口子开得更宽。
fn reject_unknown_fields(payload: &Value, allowed: &[&str]) -> AppResult<()> {
    let Some(object) = payload.as_object() else {
        return Ok(());
    };
    if let Some(unknown) = object
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(AppError::BadRequest(format!(
            "请求包含无法识别的字段 {unknown}；接口类型、鉴权和返回方式请放在 api_config 内"
        )));
    }
    Ok(())
}

const MODEL_CREATE_FIELDS: &[&str] = &[
    "provider_id", "alias", "upstream_model", "capabilities", "request_path",
    "api_config", "billing_config", "input_price_per_million",
    "output_price_per_million", "request_limits",
];
const MODEL_UPDATE_FIELDS: &[&str] = &[
    "id", "provider_id", "alias", "upstream_model", "capabilities", "enabled",
    "is_enabled", "request_path", "api_config", "billing_config",
    "input_price_per_million", "output_price_per_million", "request_limits",
];

async fn create_model(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    reject_unknown_fields(&payload, MODEL_CREATE_FIELDS)?;
    let limits = request_limits(&payload)?;
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let provider = load_provider(state, uid, provider_id).await?;
    let request_path =
        crate::model_request_path::for_provider(&provider.provider_type, &payload, "")?;
    let api_config = crate::model_api::ApiConfig::for_provider(
        &provider.provider_type,
        &payload,
        "{}",
        &request_path,
    )?;
    let billing_config = crate::gateway_billing::Config::from_payload(&payload, "{}")?;
    billing_config.validate_api(&api_config.api_type)?;
    let alias = string(&payload, "alias");
    let upstream = string(&payload, "upstream_model");
    if alias.is_empty() || upstream.is_empty() {
        return Err(AppError::BadRequest("模型别名和上游模型不能为空".into()));
    }
    let capabilities = payload
        .get("capabilities")
        .cloned()
        .unwrap_or(json!(["chat"]));
    let input_price = model_price(&payload, "input_price_per_million", None)?;
    let output_price = if api_config.api_type == crate::model_api::ApiType::Embeddings {
        None
    } else {
        model_price(&payload, "output_price_per_million", None)?
    };
    let timestamp = store::now();
    let mut transaction = state.pool.begin().await?;
    let result=sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at,input_price_per_million,output_price_per_million,request_path,api_config,billing_config) VALUES(?,?,?,?,?,1,?,?,?,?,?,?,?)").bind(uid).bind(provider_id).bind(&alias).bind(&upstream).bind(capabilities.to_string()).bind(timestamp).bind(timestamp).bind(input_price).bind(output_price).bind(&request_path).bind(serde_json::to_string(&api_config)?).bind(serde_json::to_string(&billing_config)?).execute(&mut *transaction).await.map_err(map_unique("模型别名已存在"))?;
    save_request_limits(
        &mut transaction,
        "model",
        result.last_insert_rowid(),
        limits,
    )
    .await?;
    transaction.commit().await?;
    Ok(
        json!({"id":result.last_insert_rowid(),"provider_id":provider_id,"alias":alias,"upstream_model":upstream,"request_path":request_path,"api_config":api_config,"billing_config":billing_config,"capabilities":capabilities,"input_price_per_million":input_price,"output_price_per_million":output_price,"enabled":1,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_model(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    reject_unknown_fields(&payload, MODEL_UPDATE_FIELDS)?;
    let limits = request_limits(&payload)?;
    let model_id = id(&payload)?;
    let row=sqlx::query("SELECT provider_id,alias,upstream_model,request_path,api_config,billing_config,capabilities,enabled,created_at,input_price_per_million,output_price_per_million FROM user_models WHERE id=? AND user_id=?").bind(model_id).bind(uid).fetch_optional(&state.pool).await?.ok_or_else(||AppError::NotFound("模型不存在".into()))?;
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("provider_id"));
    let provider = load_provider(state, uid, provider_id).await?;
    let request_path = crate::model_request_path::for_provider(
        &provider.provider_type,
        &payload,
        row.get("request_path"),
    )?;
    let api_config = crate::model_api::ApiConfig::for_provider(
        &provider.provider_type,
        &payload,
        row.get("api_config"),
        &request_path,
    )?;
    let billing_config =
        crate::gateway_billing::Config::from_payload(&payload, row.get("billing_config"))?;
    billing_config.validate_api(&api_config.api_type)?;
    let alias = nonempty_or(&payload, "alias", row.get("alias"));
    let upstream = nonempty_or(&payload, "upstream_model", row.get("upstream_model"));
    let capabilities = payload.get("capabilities").cloned().unwrap_or_else(|| {
        serde_json::from_str(row.get("capabilities")).unwrap_or(json!(["chat"]))
    });
    let enabled = payload
        .get("enabled")
        .or_else(|| payload.get("is_enabled"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("enabled"));
    let input_price = model_price(
        &payload,
        "input_price_per_million",
        row.get("input_price_per_million"),
    )?;
    let output_price = if api_config.api_type == crate::model_api::ApiType::Embeddings {
        None
    } else {
        model_price(
            &payload,
            "output_price_per_million",
            row.get("output_price_per_million"),
        )?
    };
    let timestamp = store::now();
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE user_models SET provider_id=?,alias=?,upstream_model=?,capabilities=?,enabled=?,updated_at=?,input_price_per_million=?,output_price_per_million=?,request_path=?,api_config=?,billing_config=? WHERE id=? AND user_id=?").bind(provider_id).bind(&alias).bind(&upstream).bind(capabilities.to_string()).bind(enabled).bind(timestamp).bind(input_price).bind(output_price).bind(&request_path).bind(serde_json::to_string(&api_config)?).bind(serde_json::to_string(&billing_config)?).bind(model_id).bind(uid).execute(&mut *transaction).await.map_err(map_unique("模型别名已存在"))?;
    save_request_limits(&mut transaction, "model", model_id, limits).await?;
    transaction.commit().await?;
    Ok(
        json!({"id":model_id,"provider_id":provider_id,"alias":alias,"upstream_model":upstream,"request_path":request_path,"api_config":api_config,"billing_config":billing_config,"capabilities":capabilities,"input_price_per_million":input_price,"output_price_per_million":output_price,"enabled":enabled,"create_time":row.get::<i64,_>("created_at"),"update_time":timestamp}),
    )
}

async fn delete_model(state: &AppState, uid: i64, model_id: i64) -> AppResult<Value> {
    let result = sqlx::query("DELETE FROM user_models WHERE id=? AND user_id=?")
        .bind(model_id)
        .bind(uid)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("模型不存在".into()));
    }
    Ok(json!({}))
}

#[derive(Clone)]
pub struct UserModelRoute {
    pub id: i64,
    pub owner_id: i64,
    pub alias: String,
    pub upstream_model: String,
    pub request_path: String,
    pub request_id: String,
    pub api_config: crate::model_api::ApiConfig,
    pub provider_id: i64,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub billing_config: crate::gateway_billing::Config,
}
pub async fn resolve_model(state: &AppState, uid: i64, alias: &str) -> AppResult<UserModelRoute> {
    let row = accessible_models(state, uid, Some(alias))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound(format!("模型 {alias} 不存在")))?;
    Ok(UserModelRoute {
        id: row.get("id"),
        owner_id: row.get("user_id"),
        alias: row.get("alias"),
        upstream_model: row.get("upstream_model"),
        request_path: row.get("request_path"),
        request_id: Uuid::new_v4().to_string(),
        api_config: crate::model_api::ApiConfig::from_stored(row.get("api_config"))?,
        provider_id: row.get("provider_id"),
        input_price_per_million: row.get("input_price_per_million"),
        output_price_per_million: row.get("output_price_per_million"),
        billing_config: crate::gateway_billing::Config::stored(row.get("billing_config"))?,
    })
}
pub async fn gateway_models(state: &AppState, uid: i64) -> AppResult<Vec<String>> {
    Ok(accessible_models(state, uid, None)
        .await?
        .into_iter()
        .map(|row| row.get("alias"))
        .collect())
}

// One catalog drives the member UI, /v1/models and route resolution. Existing
// personal aliases take precedence; shared aliases use the earliest active route.
async fn accessible_models(
    state: &AppState,
    uid: i64,
    alias: Option<&str>,
) -> AppResult<Vec<sqlx::sqlite::SqliteRow>> {
    let admin = key_admin(state, uid).await?;
    Ok(sqlx::query("WITH ranked AS (SELECT m.*,p.provider_type AS icon_provider_type,p.avatar_url AS icon_avatar_url,COALESCE((SELECT platform FROM sub2_managed_pools WHERE provider_id=p.id AND user_id=p.user_id),'') AS icon_platform,ROW_NUMBER() OVER (PARTITION BY m.alias ORDER BY (m.user_id=?) DESC,m.id) AS route_rank FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id JOIN users u ON u.id=m.user_id WHERE m.enabled=1 AND p.enabled=1 AND u.status=1 AND (m.user_id=? OR (?=0 AND u.role='admin')) AND (? IS NULL OR m.alias=?)) SELECT * FROM ranked WHERE route_rank=1 ORDER BY alias")
        .bind(uid).bind(uid).bind(admin).bind(alias).bind(alias).fetch_all(&state.pool).await?)
}

pub(crate) async fn available_models(state: &AppState, uid: i64) -> AppResult<Value> {
    let mut list = Vec::new();
    for row in accessible_models(state, uid, None).await? {
        let config = crate::model_api::ApiConfig::from_stored(row.get("api_config"))?;
        let billing = crate::gateway_billing::Config::stored(row.get("billing_config"))?;
        let mut formats = vec!["application/json"];
        if row.get::<String, _>("icon_provider_type") != "sub2api"
            && matches!(
                config.api_type,
                crate::model_api::ApiType::ImagesEdits
                    | crate::model_api::ApiType::AudioTranscriptions
                    | crate::model_api::ApiType::AudioTranslations
            )
        {
            formats.push("multipart/form-data");
        }
        list.push(json!({"icon":{"provider_type":row.get::<String,_>("icon_provider_type"),"avatar_url":row.get::<String,_>("icon_avatar_url"),"platform":row.get::<String,_>("icon_platform")},"alias":row.get::<String,_>("alias"),"api_type":config.api_type,"endpoint":config.api_type.endpoint().unwrap_or("/v1/chat/completions"),"response_mode":config.response_mode,"request_formats":formats,"billing_config":{"unit":billing.unit,"price_per_unit":billing.price_per_unit},"capabilities":serde_json::from_str::<Value>(row.get("capabilities")).unwrap_or(json!(["chat"])),"input_price_per_million":row.get::<Option<f64>,_>("input_price_per_million"),"output_price_per_million":row.get::<Option<f64>,_>("output_price_per_million")}));
    }
    Ok(json!({"total":list.len(),"list":list}))
}

async fn key_admin(state: &AppState, uid: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE id=? AND role='admin' AND status=1",
    )
    .bind(uid)
    .fetch_one(&state.pool)
    .await?
        == 1)
}

async fn key_owner_for_actor(state: &AppState, uid: i64, key_id: i64) -> AppResult<i64> {
    let admin = key_admin(state, uid).await?;
    sqlx::query_scalar("SELECT user_id FROM virtual_keys WHERE id=? AND (user_id=? OR ?)")
        .bind(key_id)
        .bind(uid)
        .bind(admin)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("访问密钥不存在".into()))
}

async fn list_keys(state: &AppState, uid: i64) -> AppResult<Value> {
    let admin = key_admin(state, uid).await?;
    list_keys_scoped(state, uid, admin).await
}

async fn list_own_keys(state: &AppState, uid: i64) -> AppResult<Value> {
    list_keys_scoped(state, uid, false).await
}

async fn list_keys_scoped(state: &AppState, uid: i64, admin: bool) -> AppResult<Value> {
    let rows=sqlx::query("SELECT k.*, k.encrypted_key IS NOT NULL AS can_reveal,u.display_name AS owner_name,u.email AS owner_email FROM virtual_keys k JOIN users u ON u.id=k.user_id WHERE k.user_id=? OR ? ORDER BY k.id DESC").bind(uid).bind(admin).fetch_all(&state.pool).await?;
    let list=rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"owner_id":r.get::<i64,_>("user_id"),"owner_name":r.get::<String,_>("owner_name"),"owner_email":r.get::<String,_>("owner_email"),"name":r.get::<String,_>("name"),"can_reveal":r.get::<i64,_>("can_reveal") == 1,"cost_usd":r.get::<f64,_>("cost_usd"),"unpriced_requests":r.get::<i64,_>("unpriced_requests"),"key_prefix":r.get::<String,_>("key_prefix"),"key_masked":format!("{}••••••••",r.get::<String,_>("key_prefix")),"allowed_models":serde_json::from_str::<Value>(r.get("allowed_models")).unwrap_or(json!([])),"status":r.get::<i64,_>("status"),"expires_at":r.get::<i64,_>("expires_at"),"create_time":r.get::<i64,_>("created_at"),"update_time":r.get::<i64,_>("updated_at"),"last_used_time":r.get::<i64,_>("last_used_at"),"total_requests":r.get::<i64,_>("total_requests"),"input_tokens":r.get::<i64,_>("input_tokens"),"output_tokens":r.get::<i64,_>("output_tokens")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len(),"scope":if admin {"all"} else {"own"}}))
}

async fn reveal_key(state: &AppState, uid: i64, key_id: i64) -> AppResult<Value> {
    let uid = key_owner_for_actor(state, uid, key_id).await?;
    let row = sqlx::query("SELECT encrypted_key FROM virtual_keys WHERE id=? AND user_id=?")
        .bind(key_id)
        .bind(uid)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("访问密钥不存在".into()))?;
    let encrypted: Option<String> = row.get("encrypted_key");
    let encrypted = encrypted.ok_or_else(|| {
        AppError::BadRequest("旧密钥尚未保存完整值，使用该密钥成功调用网关后可再次查看。".into())
    })?;
    Ok(json!({"key":secrets::decrypt(&state.config.encryption_key, &encrypted)?}))
}

async fn create_key(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let limits = request_limits(&payload)?;
    let name = string(&payload, "name");
    if name.is_empty() {
        return Err(AppError::BadRequest("Key 名称不能为空".into()));
    }
    let raw = format!("sk-juno-{}", Uuid::new_v4().simple());
    let prefix = raw.chars().take(16).collect::<String>();
    let allowed = validate_key_models(payload.get("allowed_models").unwrap_or(&json!([])))?;
    let expires = validate_key_expiry(payload.get("expires_at"), 0)?;
    let timestamp = store::now();
    let mut transaction = state.pool.begin().await?;
    let result=sqlx::query("INSERT INTO virtual_keys(user_id,name,key_prefix,key_hash,allowed_models,status,expires_at,created_at,updated_at,encrypted_key) VALUES(?,?,?,?,?,1,?,?,?,?)").bind(uid).bind(&name).bind(&prefix).bind(store::hash_key(&raw)).bind(allowed.to_string()).bind(expires).bind(timestamp).bind(timestamp).bind(secrets::encrypt(&state.config.encryption_key, &raw)?).execute(&mut *transaction).await?;
    save_request_limits(&mut transaction, "key", result.last_insert_rowid(), limits).await?;
    transaction.commit().await?;
    Ok(
        json!({"id":result.last_insert_rowid(),"name":name,"key":raw,"key_prefix":prefix,"key_masked":format!("{}••••••••",prefix),"allowed_models":allowed,"status":1,"expires_at":expires,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_key(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let limits = request_limits(&payload)?;
    let key_id = id(&payload)?;
    let uid = key_owner_for_actor(state, uid, key_id).await?;
    let row = sqlx::query(
        "SELECT name,allowed_models,status,expires_at FROM virtual_keys WHERE id=? AND user_id=?",
    )
    .bind(key_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("API Key 不存在".into()))?;
    let name = nonempty_or(&payload, "name", row.get("name"));
    let allowed = payload
        .get("allowed_models")
        .cloned()
        .unwrap_or_else(|| serde_json::from_str(row.get("allowed_models")).unwrap_or(Value::Null));
    let allowed = validate_key_models(&allowed)?;
    let status = payload
        .get("status")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("status"));
    if !matches!(status, 0 | 1) {
        return Err(AppError::BadRequest("密钥状态只能是启用或停用".into()));
    }
    let expires = validate_key_expiry(payload.get("expires_at"), row.get("expires_at"))?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE virtual_keys SET name=?,allowed_models=?,status=?,expires_at=?,updated_at=? WHERE id=? AND user_id=?").bind(&name).bind(allowed.to_string()).bind(status).bind(expires).bind(store::now()).bind(key_id).bind(uid).execute(&mut *transaction).await?;
    save_request_limits(&mut transaction, "key", key_id, limits).await?;
    transaction.commit().await?;
    Ok(
        json!({"id":key_id,"name":name,"allowed_models":allowed,"status":status,"expires_at":expires}),
    )
}
async fn delete_key(state: &AppState, uid: i64, key_id: i64) -> AppResult<Value> {
    let uid = key_owner_for_actor(state, uid, key_id).await?;
    let result = sqlx::query("DELETE FROM virtual_keys WHERE id=? AND user_id=?")
        .bind(key_id)
        .bind(uid)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("API Key 不存在".into()));
    }
    Ok(json!({}))
}

#[derive(Clone)]
pub struct GatewayKey {
    pub id: i64,
    pub user_id: i64,
    pub allowed_models: Vec<String>,
    pub expires_at: i64,
}

pub(crate) fn parse_key_models(stored: &str) -> AppResult<Vec<String>> {
    let value: Value = serde_json::from_str(stored)
        .map_err(|_| AppError::BadRequest("访问密钥模型范围数据无效".into()))?;
    Ok(serde_json::from_value(validate_key_models(&value)?)?)
}

fn validate_key_models(value: &Value) -> AppResult<Value> {
    let models = value
        .as_array()
        .filter(|models| models.len() <= 1000)
        .ok_or_else(|| AppError::BadRequest("密钥模型范围须为模型名称数组，最多 1000 项".into()))?;
    let mut normalized = Vec::new();
    for model in models {
        let name = model
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.len() <= 200)
            .ok_or_else(|| {
                AppError::BadRequest("密钥模型范围中每一项须为 1–200 字符的模型名称".into())
            })?;
        if !normalized.iter().any(|existing| existing == name) {
            normalized.push(name.to_owned());
        }
    }
    Ok(json!(normalized))
}

fn validate_key_expiry(value: Option<&Value>, previous: i64) -> AppResult<i64> {
    value
        .map(|value| value.as_i64())
        .unwrap_or(Some(previous))
        .filter(|expiry| *expiry >= 0)
        .ok_or_else(|| AppError::BadRequest("密钥到期时间须为非负整数时间戳，0 表示不过期".into()))
}

pub async fn gateway_key(state: &AppState, raw: &str) -> AppResult<Option<GatewayKey>> {
    let row = sqlx::query(
        "SELECT k.id,k.user_id,k.allowed_models,k.expires_at,k.encrypted_key IS NULL AS needs_key \
         FROM virtual_keys k JOIN users u ON u.id=k.user_id \
         WHERE k.key_hash=? AND k.status=1 AND u.status=1",
    )
    .bind(store::hash_key(raw))
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let expires: i64 = row.get("expires_at");
    if expires < 0 || (expires > 0 && expires <= store::now()) {
        return Ok(None);
    }
    // Invalid persisted restrictions must never become the unrestricted [] scope.
    let allowed: Value = match serde_json::from_str(row.get("allowed_models")) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let allowed = match validate_key_models(&allowed) {
        Ok(value) => serde_json::from_value(value)?,
        Err(_) => return Ok(None),
    };
    if row.get::<i64, _>("needs_key") == 1 {
        let encrypted = secrets::encrypt(&state.config.encryption_key, raw)?;
        sqlx::query("UPDATE virtual_keys SET encrypted_key=? WHERE id=? AND encrypted_key IS NULL")
            .bind(encrypted)
            .bind(row.get::<i64, _>("id"))
            .execute(&state.pool)
            .await?;
    }
    Ok(Some(GatewayKey {
        id: row.get("id"),
        user_id: row.get("user_id"),
        allowed_models: allowed,
        expires_at: expires,
    }))
}

async fn list_logs(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let page = payload
        .get("page")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, 1_000_000);
    let size = payload
        .get("page_size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 100);
    let admin = key_admin(state, uid).await?;
    let filters = LogFilters {
        actor_uid: uid,
        admin,
        query: payload
            .get("q")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split_whitespace()
            .take(8)
            .map(str::to_owned)
            .collect(),
        user_id: payload
            .get("user_id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0),
        user_search: payload
            .get("user_search")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        request_id: payload
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        provider: payload
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        status: payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        from: payload
            .get("from")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0),
        to: payload
            .get("to")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0),
    };

    let mut rows_query = QueryBuilder::<Sqlite>::new("SELECT l.id,l.request_id,l.virtual_key_id,COALESCE(NULLIF(l.key_name,''),NULLIF(k.name,''),'已删除密钥') AS key_name,l.model_alias,l.upstream_model,l.provider_type,l.provider_id,COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type) AS provider_name,l.status_code,l.duration_ms,l.error_message,l.input_tokens,l.output_tokens,l.total_tokens,l.cost_usd,l.billing_details,l.created_at,u.id AS owner_id,u.display_name AS owner_name,u.email AS owner_email FROM user_request_logs l JOIN users u ON u.id=l.user_id LEFT JOIN virtual_keys k ON k.id=l.virtual_key_id AND k.user_id=l.user_id LEFT JOIN user_providers p ON p.id=l.provider_id");
    push_log_filters(&mut rows_query, &filters);
    rows_query
        .push(" ORDER BY l.id DESC LIMIT ")
        .push_bind(size)
        .push(" OFFSET ")
        .push_bind((page - 1) * size);
    let rows = rows_query.build().fetch_all(&state.pool).await?;

    let mut count_query = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM user_request_logs l JOIN users u ON u.id=l.user_id LEFT JOIN virtual_keys k ON k.id=l.virtual_key_id AND k.user_id=l.user_id LEFT JOIN user_providers p ON p.id=l.provider_id");
    push_log_filters(&mut count_query, &filters);
    let total: i64 = count_query
        .build_query_scalar()
        .fetch_one(&state.pool)
        .await?;

    let mut list=rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"api_key_id":r.get::<i64,_>("virtual_key_id"),"key_name":r.get::<String,_>("key_name"),"owner_id":r.get::<i64,_>("owner_id"),"owner_name":r.get::<String,_>("owner_name"),"owner_email":r.get::<String,_>("owner_email"),"model_alias":r.get::<String,_>("model_alias"),"upstream_model":r.get::<String,_>("upstream_model"),"provider_id":r.get::<i64,_>("provider_id"),"provider_name":r.get::<String,_>("provider_name"),"provider_type":r.get::<String,_>("provider_type"),"status_code":r.get::<i64,_>("status_code"),"duration_ms":r.get::<i64,_>("duration_ms"),"error_message":r.get::<String,_>("error_message"),"input_tokens":r.get::<i64,_>("input_tokens"),"output_tokens":r.get::<i64,_>("output_tokens"),"total_tokens":r.get::<i64,_>("total_tokens"),"cost_usd":r.get::<Option<f64>,_>("cost_usd"),"request_id":r.get::<String,_>("request_id"),"billing_details":serde_json::from_str::<Value>(r.get("billing_details")).unwrap_or(json!({})),"create_time":r.get::<i64,_>("created_at")})).collect::<Vec<_>>();
    if !admin {
        for log in &mut list {
            if let Some(log) = log.as_object_mut() {
                log.remove("provider_id");
            }
        }
    }

    let users = if admin {
        sqlx::query("SELECT DISTINCT u.id,u.display_name,u.email FROM user_request_logs l JOIN users u ON u.id=l.user_id ORDER BY u.display_name,u.email")
            .fetch_all(&state.pool).await?
            .into_iter().map(|r| json!({"id":r.get::<i64,_>("id"),"name":r.get::<String,_>("display_name"),"email":r.get::<String,_>("email")})).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let models = if admin {
        sqlx::query_scalar::<_, String>("SELECT DISTINCT model_alias FROM user_request_logs WHERE model_alias<>'' ORDER BY model_alias").fetch_all(&state.pool).await?
    } else {
        sqlx::query_scalar::<_, String>("SELECT DISTINCT model_alias FROM user_request_logs WHERE user_id=? AND model_alias<>'' ORDER BY model_alias").bind(uid).fetch_all(&state.pool).await?
    };
    let providers = if admin {
        sqlx::query_scalar::<_, String>("SELECT DISTINCT COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type) FROM user_request_logs l LEFT JOIN user_providers p ON p.id=l.provider_id WHERE COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type)<>'' ORDER BY 1").fetch_all(&state.pool).await?
    } else {
        sqlx::query_scalar::<_, String>("SELECT DISTINCT COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type) FROM user_request_logs l LEFT JOIN user_providers p ON p.id=l.provider_id WHERE l.user_id=? AND COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type)<>'' ORDER BY 1").bind(uid).fetch_all(&state.pool).await?
    };
    Ok(
        json!({"list":list,"total":total,"page":page,"page_size":size,"scope":if admin {"all"} else {"own"},"filters":{"users":users,"models":models,"providers":providers}}),
    )
}

struct LogFilters {
    actor_uid: i64,
    admin: bool,
    query: Vec<String>,
    user_id: Option<i64>,
    user_search: String,
    request_id: String,
    model: String,
    provider: String,
    status: String,
    from: Option<i64>,
    to: Option<i64>,
}

fn push_log_filters<'a>(query: &mut QueryBuilder<'a, Sqlite>, filters: &'a LogFilters) {
    query.push(" WHERE 1=1");
    if !filters.admin || filters.user_id.is_some() {
        query.push(" AND l.user_id=");
        query.push_bind(if filters.admin {
            filters.user_id.unwrap_or(filters.actor_uid)
        } else {
            filters.actor_uid
        });
    }
    if !filters.request_id.is_empty() {
        query
            .push(" AND l.request_id=")
            .push_bind(&filters.request_id);
    }
    if !filters.user_search.is_empty() {
        let pattern = log_like_pattern(&filters.user_search);
        query
            .push(" AND (LOWER(u.display_name) LIKE LOWER(")
            .push_bind(pattern.clone())
            .push(") ESCAPE '\\' OR LOWER(u.email) LIKE LOWER(")
            .push_bind(pattern)
            .push(") ESCAPE '\\')");
    }
    for term in &filters.query {
        let pattern = format!("%{term}%");
        query.push(" AND (LOWER(u.display_name) LIKE LOWER(").push_bind(pattern.clone())
            .push(") OR LOWER(u.email) LIKE LOWER(").push_bind(pattern.clone())
            .push(") OR LOWER(COALESCE(NULLIF(l.key_name,''),NULLIF(k.name,''),'')) LIKE LOWER(").push_bind(pattern.clone())
            .push(") OR LOWER(l.model_alias) LIKE LOWER(").push_bind(pattern.clone())
            .push(") OR LOWER(l.upstream_model) LIKE LOWER(").push_bind(pattern.clone())
            .push(") OR LOWER(COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type)) LIKE LOWER(").push_bind(pattern)
            .push("))");
    }
    if !filters.model.is_empty() {
        query
            .push(" AND LOWER(l.model_alias) LIKE LOWER(")
            .push_bind(log_like_pattern(&filters.model))
            .push(") ESCAPE '\\'");
    }
    if !filters.provider.is_empty() {
        query
            .push(" AND LOWER(COALESCE(NULLIF(l.provider_name,''),NULLIF(p.name,''),l.provider_type)) LIKE LOWER(")
            .push_bind(log_like_pattern(&filters.provider))
            .push(") ESCAPE '\\'");
    }
    match filters.status.as_str() {
        "success" => {
            query.push(" AND l.status_code>=200 AND l.status_code<400");
        }
        "4xx" => {
            query.push(" AND l.status_code>=400 AND l.status_code<500");
        }
        "5xx" => {
            query.push(" AND l.status_code>=500 AND l.status_code<600");
        }
        "other" => {
            query.push(" AND (l.status_code<200 OR l.status_code>=600)");
        }
        _ => {}
    }
    if let Some(from) = filters.from {
        query.push(" AND l.created_at>=").push_bind(from);
    }
    if let Some(to) = filters.to {
        query.push(" AND l.created_at<").push_bind(to);
    }
}

fn log_like_pattern(value: &str) -> String {
    format!(
        "%{}%",
        value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
async fn stats(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    if !key_admin(state, uid).await? {
        return personal_stats(state, uid, payload).await;
    }
    let providers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_providers WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let models: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_models WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM virtual_keys WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let fallback = store::now() - store::now().rem_euclid(86_400);
    let since = payload
        .get("since")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0 && *value <= store::now())
        .unwrap_or(fallback);
    let (requests, input_tokens, output_tokens): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0) FROM user_request_logs WHERE user_id=? AND created_at>=?",
    )
    .bind(uid)
    .bind(since)
    .fetch_one(&state.pool)
    .await?;
    Ok(
        json!({"providers":providers,"models":models,"keys":keys,"requests":requests,"input_tokens":input_tokens,"output_tokens":output_tokens,"total_tokens":input_tokens.saturating_add(output_tokens)}),
    )
}

async fn personal_stats(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM virtual_keys WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let since = payload["since"]
        .as_i64()
        .filter(|value| *value > 0 && *value <= store::now())
        .unwrap_or(store::now() - store::now().rem_euclid(86_400));
    let today = sqlx::query("SELECT COUNT(*) AS requests,COALESCE(SUM(input_tokens),0) AS input,COALESCE(SUM(output_tokens),0) AS output FROM user_request_logs WHERE user_id=? AND created_at>=?").bind(uid).bind(since).fetch_one(&state.pool).await?;
    let usage = sqlx::query("SELECT COUNT(*) AS requests,COALESCE(SUM(cost_usd),0.0) AS cost,SUM(CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END) AS unpriced FROM user_request_logs WHERE user_id=?").bind(uid).fetch_one(&state.pool).await?;
    let input: i64 = today.get("input");
    let output: i64 = today.get("output");
    Ok(
        json!({"models":gateway_models(state,uid).await?.len(),"keys":keys,"requests":today.get::<i64,_>("requests"),"input_tokens":input,"output_tokens":output,"total_tokens":input.saturating_add(output),"total_requests":usage.get::<i64,_>("requests"),"cost_usd":usage.get::<f64,_>("cost"),"unpriced_requests":usage.get::<Option<i64>,_>("unpriced").unwrap_or(0),"scope":"own"}),
    )
}

pub async fn admin_users(state: &AppState, payload: &Value) -> AppResult<Value> {
    let page = payload
        .get("page")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, 1_000_000);
    let size = payload
        .get("page_size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 100);
    let rows=sqlx::query("SELECT id,email,display_name,role,status,created_at,updated_at FROM users ORDER BY id DESC LIMIT ? OFFSET ?").bind(size).bind((page-1)*size).fetch_all(&state.pool).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    let list=rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"email":r.get::<String,_>("email"),"display_name":r.get::<String,_>("display_name"),"role":r.get::<String,_>("role"),"status":r.get::<i64,_>("status"),"create_time":r.get::<i64,_>("created_at"),"update_time":r.get::<i64,_>("updated_at")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":total,"page":page,"page_size":size}))
}
pub async fn admin_user_detail(state: &AppState, payload: &Value) -> AppResult<Value> {
    let uid = id(payload)?;
    let user = sqlx::query(
        "SELECT id,email,display_name,role,status,created_at,updated_at FROM users WHERE id=?",
    )
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
    let usage = sqlx::query("SELECT (SELECT COUNT(*) FROM virtual_keys WHERE user_id=?) AS keys_count,COUNT(*) AS requests,COALESCE(SUM(input_tokens),0) AS input,COALESCE(SUM(output_tokens),0) AS output,COALESCE(SUM(cost_usd),0.0) AS cost,COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END),0) AS unpriced,COALESCE(MAX(created_at),0) AS last_used FROM user_request_logs WHERE user_id=?")
        .bind(uid)
        .bind(uid).fetch_one(&state.pool).await?;
    let rows = sqlx::query("SELECT id,name,key_prefix,status,expires_at,total_requests,input_tokens,output_tokens,cost_usd FROM virtual_keys WHERE user_id=? ORDER BY created_at DESC,id DESC LIMIT 50")
        .bind(uid).fetch_all(&state.pool).await?;
    let keys: Vec<Value> = rows.into_iter().map(|r| json!({"id":r.get::<i64,_>("id"),"name":r.get::<String,_>("name"),"prefix":r.get::<String,_>("key_prefix"),"status":r.get::<i64,_>("status"),"expires_at":r.get::<i64,_>("expires_at"),"requests":r.get::<i64,_>("total_requests"),"input_tokens":r.get::<i64,_>("input_tokens"),"output_tokens":r.get::<i64,_>("output_tokens"),"cost_usd":r.get::<f64,_>("cost_usd")})).collect();
    let models: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_models WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let providers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_providers WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    Ok(
        json!({"user":{"id":uid,"email":user.get::<String,_>("email"),"display_name":user.get::<String,_>("display_name"),"role":user.get::<String,_>("role"),"status":user.get::<i64,_>("status"),"created_at":user.get::<i64,_>("created_at"),"updated_at":user.get::<i64,_>("updated_at")},"usage":{"keys_count":usage.get::<i64,_>("keys_count"),"requests":usage.get::<i64,_>("requests"),"input_tokens":usage.get::<i64,_>("input"),"output_tokens":usage.get::<i64,_>("output"),"cost_usd":usage.get::<f64,_>("cost"),"unpriced_requests":usage.get::<i64,_>("unpriced"),"last_used_at":usage.get::<i64,_>("last_used"),"models_count":models,"providers_count":providers},"keys":keys}),
    )
}

pub async fn admin_update_user(state: &AppState, payload: &Value) -> AppResult<Value> {
    let limits = request_limits(payload)?;
    let user_id = id(payload)?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = sqlx::query("SELECT role,status FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
    let status = payload
        .get("status")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| current.get("status"));
    if !matches!(status, 0 | 1) {
        return Err(AppError::BadRequest("status 只能是 0 或 1".into()));
    }
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_else(|| current.get("role"));
    if !matches!(role, "user" | "admin") {
        return Err(AppError::BadRequest("role 只能是 user 或 admin".into()));
    }
    if current.get::<String, _>("role") == "admin"
        && current.get::<i64, _>("status") == 1
        && (role != "admin" || status != 1)
    {
        let others: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role='admin' AND status=1 AND id<>?",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if others == 0 {
            return Err(AppError::BadRequest(
                "不能停用或降级最后一位有效管理员".into(),
            ));
        }
    }
    sqlx::query("UPDATE users SET role=?,status=?,session_version=session_version+CASE WHEN role<>? OR status<>? THEN 1 ELSE 0 END,updated_at=? WHERE id=?")
        .bind(role)
        .bind(status)
        .bind(role)
        .bind(status)
        .bind(store::now())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    save_request_limits(&mut tx, "user", user_id, limits).await?;
    tx.commit().await?;
    Ok(json!({"id":user_id,"role":role,"status":status}))
}

#[allow(clippy::too_many_arguments)]
pub async fn record_request(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    provider_type: &str,
    status: u16,
    duration: i64,
    error: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Option<i64> {
    record_request_with_usage(
        state,
        key,
        model,
        provider_type,
        status,
        duration,
        error,
        input_tokens,
        output_tokens,
        input_tokens.saturating_add(output_tokens) > 0,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_request_with_usage(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    provider_type: &str,
    status: u16,
    duration: i64,
    error: &str,
    input_tokens: i64,
    output_tokens: i64,
    usage_complete: bool,
) -> Option<i64> {
    record_request_with_billing(
        state,
        key,
        model,
        provider_type,
        status,
        duration,
        error,
        input_tokens,
        output_tokens,
        usage_complete,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_request_with_billing(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    provider_type: &str,
    status: u16,
    duration: i64,
    error: &str,
    input_tokens: i64,
    output_tokens: i64,
    usage_complete: bool,
    response: Option<&Value>,
) -> Option<i64> {
    let result = async {
    let input_tokens = input_tokens.max(0);
    let output_tokens = if model.api_config.api_type == crate::model_api::ApiType::Embeddings { 0 }
        else { output_tokens.max(0) };
    let total_tokens = input_tokens.saturating_add(output_tokens);
    let prices = sqlx::query("SELECT p.name AS provider_name,k.name AS key_name FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id JOIN virtual_keys k ON k.id=? AND k.user_id=? WHERE m.id=? AND m.user_id=?")
        .bind(key.id).bind(key.user_id).bind(model.id).bind(model.owner_id).fetch_optional(&state.pool).await.ok().flatten();
    let provider_name = prices
        .as_ref()
        .map(|row| row.get::<String, _>("provider_name"))
        .unwrap_or_default();
    let key_name = prices
        .as_ref()
        .map(|row| row.get::<String, _>("key_name"))
        .unwrap_or_default();
    // A request uses the prices resolved before dispatch, even if the route is edited
    // while the upstream is generating or streaming its response.
    let bill=crate::gateway_billing::calculate(&model.billing_config,model.input_price_per_million,
        model.output_price_per_million,input_tokens,output_tokens,usage_complete,response,
        (200..300).contains(&status) && error.is_empty() && !(model.api_config.is_task() && status==202));
    let cost=bill.cost;
    let Ok(mut transaction) = state.pool.begin().await else {
        return None;
    };
    let logged = sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,key_name,model_alias,upstream_model,provider_type,provider_id,provider_name,status_code,duration_ms,error_message,input_tokens,output_tokens,total_tokens,created_at,cost_usd,input_price_per_million,output_price_per_million,billing_config,billing_details,request_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(key.user_id).bind(key.id).bind(key_name).bind(&model.alias).bind(&model.upstream_model).bind(provider_type).bind(model.provider_id).bind(provider_name).bind(status as i64).bind(duration).bind(error).bind(input_tokens).bind(output_tokens).bind(total_tokens).bind(store::now()).bind(cost).bind(model.input_price_per_million).bind(model.output_price_per_million).bind(serde_json::to_string(&model.billing_config).ok()?).bind(bill.details.to_string()).bind(&model.request_id).execute(&mut *transaction).await;
    let Ok(logged) = logged else { return None };
    let log_id = logged.last_insert_rowid();
    let updated = sqlx::query("UPDATE virtual_keys SET total_requests=total_requests+1,input_tokens=input_tokens+?,output_tokens=output_tokens+?,last_used_at=?,cost_usd=cost_usd+?,unpriced_requests=unpriced_requests+? WHERE id=? AND user_id=?")
        .bind(input_tokens).bind(output_tokens).bind(store::now()).bind(cost.unwrap_or(0.0)).bind(i64::from(cost.is_none())).bind(key.id).bind(key.user_id).execute(&mut *transaction).await;
    if updated.is_ok() && transaction.commit().await.is_ok() {
        return Some(log_id);
    }
    None
    }.await;
    if result.is_none() {
        crate::observability::log_failure();
        tracing::error!(request_id=%model.request_id,"gateway request log write failed");
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::image_storage::{
        tests::{mock_oss, PNG_AVATAR},
        validate_avatar,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    async fn usage_fixture() -> (AppState, GatewayKey, UserModelRoute) {
        let state = state().await;
        let provider = create_provider(&state, 1, json!({
            "name":"usage fixture", "provider_type":"openai", "base_url":"https://example.test/v1"
        })).await.unwrap();
        create_model(
            &state,
            1,
            json!({
                "provider_id":provider["id"], "alias":"usage-fixture", "upstream_model":"upstream",
                "input_price_per_million":1, "output_price_per_million":2
            }),
        )
        .await
        .unwrap();
        let created = create_key(&state, 1, json!({"name":"usage fixture"}))
            .await
            .unwrap();
        let key = gateway_key(&state, created["key"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        let model = resolve_model(&state, 1, "usage-fixture").await.unwrap();
        (state, key, model)
    }

    #[tokio::test]
    async fn continued_fix_media_billing_crud_and_actual_results() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, old) = usage_fixture().await;
        update_model(
            &state,
            1,
            json!({"id":old.id,"billing_config":{"unit":"image","price_per_unit":0.04}}),
        )
        .await
        .unwrap();
        let model = resolve_model(&state, 1, &old.alias).await.unwrap();
        assert_eq!(
            list_models(&state, 1).await.unwrap()["list"][0]["billing_config"]["unit"],
            "image"
        );
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let public = available_models(&state, 2).await.unwrap();
        assert_eq!(public["list"][0]["billing_config"]["price_per_unit"], 0.04);
        assert!(public["list"][0]["billing_config"]
            .get("quantity_pointer")
            .is_none());
        update_model(
            &state,
            1,
            json!({"id":model.id,"billing_config":{"unit":"image","price_per_unit":100}}),
        )
        .await
        .unwrap();
        for (status, body) in [
            (200, json!({"n":20,"data":[{"url":"a"},{"url":"b"}]})),
            (200, json!({"n":20})),
            (500, json!({"data":[{"url":"a"}]})),
        ] {
            let response = Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                response,
                0,
                None,
            )
            .await
            .unwrap();
            to_bytes(response.into_body(), 10000).await.unwrap();
        }
        let costs: Vec<Option<f64>> =
            sqlx::query_scalar("SELECT cost_usd FROM user_request_logs ORDER BY id")
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(costs, vec![Some(0.08), None, None]);
        let logs = list_logs(&state, 1, &json!({})).await.unwrap();
        assert_eq!(logs["list"][2]["billing_details"]["quantity"], "2");
        assert_eq!(logs["list"][2]["billing_details"]["amount_usd"], "0.08");
        record_request(
            &state,
            &key,
            &old,
            "fixture",
            502,
            1,
            "dispatch failed",
            0,
            0,
        )
        .await
        .unwrap();
        let cost: Option<f64> =
            sqlx::query_scalar("SELECT cost_usd FROM user_request_logs ORDER BY id DESC LIMIT 1")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(cost.is_none());
        assert!(update_model(
            &state,
            1,
            json!({"id":model.id,"billing_config":{"unit":"second"}})
        )
        .await
        .is_err());
        update_model(&state, 1, json!({"id":model.id,"alias":"renamed"}))
            .await
            .unwrap();
        assert_eq!(
            resolve_model(&state, 1, "renamed")
                .await
                .unwrap()
                .billing_config
                .price_per_unit,
            Some(100.0)
        );
        assert!(update_model(
            &state,
            1,
            json!({"id":model.id,"api_config":{"api_type":"embeddings"}})
        )
        .await
        .is_err());
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn continued_fix_background_tasks_settle_without_clients_and_keep_snapshots() {
        use axum::{
            body::to_bytes,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
            Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (mut state, key, old) = usage_fixture().await;
        state.client = Client::builder().no_proxy().build().unwrap();
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        let polls = Arc::new(AtomicUsize::new(0));
        let seen = polls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new()
            .route("/submit",post(||async {Json(json!({"task_id":"vendor-job","status":"queued"}))}))
            .route("/tasks/{id}",get(move |headers:HeaderMap| {let seen=seen.clone();async move {
                assert_eq!(headers["authorization"],"Bearer snapshot-secret");
                if seen.fetch_add(1,Ordering::SeqCst)==0 {(StatusCode::TOO_MANY_REQUESTS,Json(json!({"error":"later"})))}
                else {tokio::time::sleep(Duration::from_millis(50)).await;(StatusCode::OK,Json(json!({"status":"succeeded","result":{"images":[{"url":"a"},{"url":"b"}]}})))}
            }}))).await.unwrap();
        });
        sqlx::query("UPDATE user_providers SET base_url=? WHERE id=?")
            .bind(format!("http://{address}"))
            .bind(old.provider_id)
            .execute(&state.pool)
            .await
            .unwrap();
        let encrypted = secrets::encrypt(&state.config.encryption_key, "snapshot-secret").unwrap();
        sqlx::query("UPDATE provider_keys SET encrypted_api_key=? WHERE provider_id=?")
            .bind(encrypted)
            .bind(old.provider_id)
            .execute(&state.pool)
            .await
            .unwrap();
        update_model(&state,1,json!({"id":old.id,"request_path":"/submit","api_config":{"api_type":"images_generations","response_mode":"task","task_path":"/tasks/{task_id}"},"billing_config":{"unit":"image","price_per_unit":0.04}})).await.unwrap();
        let model = resolve_model(&state, 1, &old.alias).await.unwrap();
        let account = crate::direct_provider::keys(&state, model.provider_id)
            .await
            .unwrap()
            .remove(0);
        let response = crate::gateway_tasks::submit(
            &state,
            &key,
            &model,
            &account,
            state
                .client
                .post(format!("http://{address}/submit"))
                .json(&json!({})),
        )
        .await
        .unwrap();
        let response = crate::proxy::record_response(
            &state,
            &key,
            &model,
            "fixture",
            std::time::Instant::now(),
            response,
            0,
            None,
        )
        .await
        .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        let id = value["task_id"].as_str().unwrap();
        crate::gateway_tasks::run_background_once(&state)
            .await
            .unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        let retry: (i64, i64) =
            sqlx::query_as("SELECT poll_after,poll_failures FROM gateway_tasks WHERE id=?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(retry.0 > store::now());
        assert_eq!(retry.1, 1);
        crate::gateway_tasks::run_background_once(&state)
            .await
            .unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        update_model(
            &state,
            1,
            json!({"id":model.id,"billing_config":{"unit":"image","price_per_unit":100}}),
        )
        .await
        .unwrap();
        delete_provider(&state, 1, model.provider_id).await.unwrap();
        sqlx::query("UPDATE gateway_tasks SET poll_after=0,poll_owner='dead-worker',poll_lease_until=? WHERE id=?").bind(store::now()+60).bind(id).execute(&state.pool).await.unwrap();
        crate::gateway_tasks::run_background_once(&state)
            .await
            .unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        sqlx::query("UPDATE gateway_tasks SET poll_lease_until=0 WHERE id=?")
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            crate::gateway_tasks::run_background_once(&state),
            crate::gateway_tasks::run_background_once(&state)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        for _ in 0..2 {
            crate::gateway_tasks::run_background_once(&state)
                .await
                .unwrap();
        }
        let totals: (i64, f64, i64) = sqlx::query_as(
            "SELECT total_requests,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?",
        )
        .bind(key.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(totals, (1, 0.08, 0));
        let row: (String, i64, String) =
            sqlx::query_as("SELECT status,settled,encrypted_api_key FROM gateway_tasks WHERE id=?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(row, ("succeeded".into(), 1, String::new()));
        let raw = reveal_key(&state, 1, key.id).await.unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {raw}").parse().unwrap());
        let response = crate::gateway_tasks::query(State(state.clone()), Path(id.into()), headers)
            .await
            .unwrap();
        let result: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(result["status"], "succeeded");
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn continued_fix_expired_jobs_are_unpriced_and_never_resubmitted() {
        let (state, key, model) = usage_fixture().await;
        let log =
            record_request_with_usage(&state, &key, &model, "fixture", 202, 1, "", 0, 0, false)
                .await
                .unwrap();
        sqlx::query("INSERT INTO gateway_tasks(id,user_id,virtual_key_id,model_id,model_alias,provider_id,provider_key_id,base_url,encrypted_api_key,auth_type,task_path,created_at,expires_at,log_id,api_type) VALUES('expired',?,?,?,?,?,0,'https://unreachable.example.test','fixture','\"bearer\"','/tasks/{task_id}',?,?,?,'images_generations')")
            .bind(key.user_id).bind(key.id).bind(model.id).bind(&model.alias).bind(model.provider_id).bind(store::now()-4000).bind(store::now()-1).bind(log).execute(&state.pool).await.unwrap();
        for _ in 0..2 {
            crate::gateway_tasks::run_background_once(&state)
                .await
                .unwrap();
        }
        let row: (i64, Option<f64>, String) = sqlx::query_as(
            "SELECT status_code,cost_usd,error_message FROM user_request_logs WHERE id=?",
        )
        .bind(log)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(row.0, 504);
        assert!(row.1.is_none());
        assert!(row.2.contains("未获确认"));
        let state_row: (i64, String) = sqlx::query_as(
            "SELECT settled,encrypted_api_key FROM gateway_tasks WHERE id='expired'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(state_row, (1, String::new()));
        let totals: (i64, i64) =
            sqlx::query_as("SELECT total_requests,unpriced_requests FROM virtual_keys WHERE id=?")
                .bind(key.id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(totals, (1, 1));
    }

    #[tokio::test]
    async fn third_review_extreme_pagination_cannot_overflow_or_return_first_page() {
        let (state, key, model) = usage_fixture().await;
        record_request_with_usage(&state, &key, &model, "openai", 200, 1, "", 1, 1, true)
            .await
            .unwrap();
        let payload = json!({"page":i64::MAX,"page_size":100});
        for result in [
            admin_users(&state, &payload).await.unwrap(),
            list_logs(&state, key.user_id, &payload).await.unwrap(),
        ] {
            assert!(result["list"].as_array().unwrap().is_empty());
            assert_eq!(result["page"], 1_000_000);
        }
    }

    #[tokio::test]
    async fn third_review_history_and_inflight_prices_survive_deletion() {
        let (state, key, model) = usage_fixture().await;
        record_request_with_usage(&state, &key, &model, "openai", 200, 1, "", 100, 200, true)
            .await
            .unwrap();
        let before = admin_user_detail(&state, &json!({"id":key.user_id}))
            .await
            .unwrap();
        delete_key(&state, key.user_id, key.id).await.unwrap();
        let after = admin_user_detail(&state, &json!({"id":key.user_id}))
            .await
            .unwrap();
        assert_eq!(after["usage"]["requests"], before["usage"]["requests"]);
        assert_eq!(after["usage"]["cost_usd"], before["usage"]["cost_usd"]);
        assert_eq!(after["usage"]["keys_count"], 0);
        delete_model(&state, model.owner_id, model.id)
            .await
            .unwrap();
        let log =
            record_request_with_usage(&state, &key, &model, "openai", 200, 1, "", 100, 200, true)
                .await
                .unwrap();
        let cost: Option<f64> =
            sqlx::query_scalar("SELECT cost_usd FROM user_request_logs WHERE id=?")
                .bind(log)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(cost, Some(0.0005));
        let detail = admin_user_detail(&state, &json!({"id":key.user_id}))
            .await
            .unwrap();
        assert_eq!(detail["usage"]["requests"], 2);
        assert_eq!(detail["usage"]["input_tokens"], 200);
        assert_eq!(detail["usage"]["output_tokens"], 400);
        assert_eq!(detail["usage"]["unpriced_requests"], 0);
        assert_eq!(detail["usage"]["cost_usd"], 0.001);
    }

    #[tokio::test]
    async fn functional_recheck_key_scopes_and_expiry_fail_closed() {
        let state = state().await;
        for allowed in [
            json!("chat"),
            json!({"models":["chat"]}),
            json!([1]),
            json!([""]),
            Value::Null,
        ] {
            assert!(create_key(
                &state,
                1,
                json!({"name":"invalid","allowed_models":allowed})
            )
            .await
            .is_err());
        }
        for expires in [json!(-1), json!("never"), Value::Null, json!(0.5)] {
            assert!(create_key(
                &state,
                1,
                json!({"name":"invalid expiry","expires_at":expires})
            )
            .await
            .is_err());
        }
        let created = create_key(
            &state,
            1,
            json!({"name":"scoped","allowed_models":[" chat-* ","chat-*"],"expires_at":0}),
        )
        .await
        .unwrap();
        let raw = created["key"].as_str().unwrap();
        let id = created["id"].as_i64().unwrap();
        let key = gateway_key(&state, raw).await.unwrap().unwrap();
        assert_eq!(key.allowed_models, vec!["chat-*"]);
        assert!(crate::proxy::allows_model(&key, "chat-small"));
        assert!(!crate::proxy::allows_model(&key, "image"));
        assert!(update_key(&state, 1, json!({"id":id,"allowed_models":"*"}))
            .await
            .is_err());
        assert_eq!(
            gateway_key(&state, raw)
                .await
                .unwrap()
                .unwrap()
                .allowed_models,
            vec!["chat-*"]
        );
        for corrupted in ["not-json", "[123]", "null", "[\"\"]"] {
            sqlx::query("UPDATE virtual_keys SET allowed_models=? WHERE id=?")
                .bind(corrupted)
                .bind(id)
                .execute(&state.pool)
                .await
                .unwrap();
            assert!(gateway_key(&state, raw).await.unwrap().is_none());
        }
        sqlx::query("UPDATE virtual_keys SET allowed_models='[]',expires_at=? WHERE id=?")
            .bind(store::now())
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(gateway_key(&state, raw).await.unwrap().is_none());
        sqlx::query("UPDATE virtual_keys SET expires_at=0 WHERE id=?")
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(gateway_key(&state, raw).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn functional_recheck_gemini_unsupported_methods_never_generate() {
        use axum::{body::Body, http::Request};
        let state = state().await;
        for (method, path) in [
            ("POST", "model:countTokens"),
            ("POST", "model:unknown"),
            ("GET", "model:generateContent"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri("/v1beta/models/model:countTokens")
                .body(Body::from("{}"))
                .unwrap();
            let result = crate::client_protocol::gemini_generate(
                State(state.clone()),
                Path(path.into()),
                request,
            )
            .await;
            // Method validation occurs before auth/dispatch. No credential or
            // schedulable upstream exists in this fixture.
            assert!(matches!(result, Err(AppError::BadRequest(_))));
        }
    }

    #[tokio::test]
    async fn stream_completion_stops_delivery_and_accounting_at_terminal_event() {
        use axum::{
            body::{to_bytes, Body, Bytes},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        let terminal =
            b"data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n";
        for combined in [false, true] {
            let tail = vec![b'x'; 32 * 1024 * 1024 + 1];
            let chunks = if combined {
                let mut chunk = terminal.to_vec();
                chunk.extend(tail);
                vec![Ok::<Bytes, std::io::Error>(Bytes::from(chunk))]
            } else {
                vec![Ok(Bytes::copy_from_slice(terminal)), Ok(Bytes::from(tail))]
            };
            let upstream = Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                0,
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap()
                    .as_ref(),
                terminal
            );
        }
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT status_code,input_tokens,output_tokens FROM user_request_logs ORDER BY id",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![(200, 12, 3); 2]);
        let totals: (i64, i64, i64) = sqlx::query_as(
            "SELECT total_requests,input_tokens,output_tokens FROM virtual_keys WHERE id=?",
        )
        .bind(key.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(totals, (2, 24, 6));
    }

    #[tokio::test]
    async fn mislabeled_sse_requires_valid_terminal_event_before_billing() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        let usage = "data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n\n";
        let cases = [
            (usage.to_owned(), false),
            (format!("{usage}data: [DONE]\n\n"), true),
            (format!("{usage}data: [DONE]"), false),
            (format!("{usage}data: broken\n\ndata: [DONE]\n\n"), false),
        ];
        for content_type in [Some("application/json"), None] {
            for (body, completed) in &cases {
                let mut upstream = Response::builder();
                if let Some(content_type) = content_type {
                    upstream = upstream.header("content-type", content_type);
                }
                let upstream = upstream.body(Body::from(body.clone())).unwrap();
                let response = crate::proxy::record_response(
                    &state,
                    &key,
                    &model,
                    "fixture",
                    std::time::Instant::now(),
                    upstream,
                    0,
                    None,
                )
                .await
                .unwrap();
                assert_eq!(
                    response.status().as_u16(),
                    if *completed { 200 } else { 502 }
                );
                assert_eq!(
                    to_bytes(response.into_body(), 10000)
                        .await
                        .unwrap()
                        .as_ref(),
                    body.as_bytes()
                );
            }
        }
        let rows: Vec<(i64, Option<f64>)> =
            sqlx::query_as("SELECT status_code,cost_usd FROM user_request_logs ORDER BY id")
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 8);
        for (index, (status, cost)) in rows.iter().enumerate() {
            if index % 4 == 1 {
                assert_eq!(*status, 200);
                assert!((cost.unwrap() - 0.000018).abs() < 1e-12);
            } else {
                assert_eq!((*status, *cost), (502, None));
            }
        }
    }

    #[tokio::test]
    async fn native_messages_cache_usage_is_consistent_for_json_and_streams() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, mut model) = usage_fixture().await;
        model.api_config.api_type = crate::model_api::ApiType::Messages;
        let json = json!({"type":"message","usage":{
            "input_tokens":10,"cache_read_input_tokens":100,
            "cache_creation_input_tokens":20,"output_tokens":5
        }})
        .to_string();
        let stream = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":20}}}\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        for (kind, body) in [
            ("application/json", json.as_str()),
            ("text/event-stream", stream),
        ] {
            let upstream = Response::builder()
                .header("content-type", kind)
                .body(Body::from(body.to_owned()))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                0,
                None,
            )
            .await
            .unwrap();
            to_bytes(response.into_body(), 10000).await.unwrap();
        }
        let rows: Vec<(i64, i64, f64)> = sqlx::query_as(
            "SELECT input_tokens,output_tokens,cost_usd FROM user_request_logs ORDER BY id",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for (input, output, cost) in rows {
            assert_eq!((input, output), (130, 5));
            assert!((cost - 0.00014).abs() < 1e-12);
        }
    }

    #[tokio::test]
    async fn stream_accounting_decodes_multiline_events_and_rejects_unfinished_delivery() {
        use axum::{
            body::{to_bytes, Body, Bytes},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        let valid = concat!(
            "data: {\"type\":\"response.completed\",\r\n",
            "data: \"response\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":3}}}\r\n\r\n"
        );
        let partial = "data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n\n";
        let failed = "data: {\"type\":\"response.failed\",\ndata: \"response\":{\"error\":{\"message\":\"multiline failure\"}}}\n\n";
        for (body, delivered) in [
            (valid, true),
            (partial, false),
            ("data: broken\n\n", false),
            (failed, true),
        ] {
            let chunks = body
                .as_bytes()
                .chunks(7)
                .map(|part| Ok::<_, std::io::Error>(Bytes::copy_from_slice(part)))
                .collect::<Vec<_>>();
            let upstream = Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                0,
                None,
            )
            .await
            .unwrap();
            let bytes = to_bytes(response.into_body(), 10000).await;
            let bytes = bytes.unwrap();
            if delivered {
                assert_eq!(bytes.as_ref(), body.as_bytes());
            } else {
                let text = std::str::from_utf8(&bytes).unwrap();
                assert!(text.contains("upstream_error"));
                assert!(!text.contains("[DONE]"));
                assert!(!text.contains("data: broken"));
            }
        }
        let rows: Vec<(i64, i64, i64, Option<f64>, String)> = sqlx::query_as(
            "SELECT status_code,input_tokens,output_tokens,cost_usd,error_message FROM user_request_logs ORDER BY id"
        ).fetch_all(&state.pool).await.unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!((rows[0].0, rows[0].1, rows[0].2), (200, 12, 3));
        assert!((rows[0].3.unwrap() - 0.000018).abs() < 1e-12);
        assert_eq!((rows[1].1, rows[1].2), (12, 3));
        for row in &rows[1..] {
            assert_eq!(row.0, 502);
            assert!(row.3.is_none());
        }
        assert!(rows[1].4.contains("完成事件前结束"));
        assert!(rows[2].4.contains("格式无效"));
        assert_eq!(rows[3].4, "multiline failure");
    }

    #[tokio::test]
    async fn premature_stream_end_is_readable_in_each_client_protocol() {
        use axum::{body::{to_bytes, Body}, response::Response};
        use crate::model_api::ApiType;
        for api in [ApiType::Messages, ApiType::Responses, ApiType::ChatCompletions] {
            let (state, key, mut model) = usage_fixture().await;
            model.api_config.api_type = api.clone();
            // A partial upstream frame must not corrupt the generated error event.
            let upstream = Response::builder().header("content-type", "text/event-stream")
                .body(Body::from("data: {\"incomplete\":" )).unwrap();
            let response = crate::proxy::record_response(&state, &key, &model, "fixture", std::time::Instant::now(), upstream, 0, None).await.unwrap();
            let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
            let (end, _) = crate::sub2_sse::frame_end(&bytes).unwrap();
            let Some(crate::sub2_sse::Frame::Event(value)) = crate::sub2_sse::decode(&bytes[..end]).unwrap() else { panic!("missing error") };
            let message = match api {
                ApiType::Responses => { assert_eq!(value["type"], "error"); value["message"].as_str().unwrap() },
                _ => value["error"]["message"].as_str().unwrap(),
            };
            assert!(message.contains("完成事件前结束"));
            assert!(!bytes.windows(6).any(|v| v == b"[DONE]"));
            let (status, cost): (i64, Option<f64>) = sqlx::query_as("SELECT status_code,cost_usd FROM user_request_logs").fetch_one(&state.pool).await.unwrap();
            assert_eq!(status, 502);
            assert!(cost.is_none());
        }
    }

    #[tokio::test]
    async fn functional_recheck_errors_and_body_failures_are_logged() {
        use axum::{
            body::{to_bytes, Body, Bytes},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        for (content_type,body) in [
            ("application/json","{\"error\":{\"message\":\"fixture upstream failure\"}}"),
            ("text/event-stream","data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"fixture upstream failure\"}}}\n\n"),
            ("text/event-stream","event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"fixture upstream failure\"}}\n\n"),
        ] {
            let upstream=Response::builder().header("content-type",content_type).body(Body::from(body)).unwrap();
            let response=crate::proxy::record_response(&state,&key,&model,"fixture",std::time::Instant::now(),upstream,0,None).await.unwrap();
            assert_eq!(response.status(),if content_type=="application/json" {502}else{200});
            assert_eq!(to_bytes(response.into_body(),10000).await.unwrap().as_ref(),body.as_bytes());
        }
        let failed_body = || {
            Body::from_stream(futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::other("fixture broken body"))
            }))
        };
        let response = Response::builder()
            .header("content-type", "application/json")
            .body(failed_body())
            .unwrap();
        assert!(matches!(
            crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                response,
                0,
                None
            )
            .await,
            Err(AppError::BadGateway(_))
        ));
        let response = Response::builder()
            .header("content-type", "text/event-stream")
            .body(failed_body())
            .unwrap();
        let response = crate::proxy::record_response(
            &state,
            &key,
            &model,
            "fixture",
            std::time::Instant::now(),
            response,
            0,
            None,
        )
        .await
        .unwrap();
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("上游流式响应读取失败"));
        let rows: Vec<(i64, String, Option<f64>)> = sqlx::query_as(
            "SELECT status_code,error_message,cost_usd FROM user_request_logs ORDER BY id",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 5);
        for (index, (status, error, cost)) in rows.into_iter().enumerate() {
            assert_eq!(status, 502);
            assert!(!error.is_empty());
            assert!(cost.is_none());
            if index < 3 {
                assert_eq!(error, "fixture upstream failure");
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT total_requests FROM virtual_keys WHERE id=?")
            .bind(key.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn functional_recheck_stream_error_is_saved_before_client_disconnect() {
        use axum::{body::Body, response::Response};
        use futures_util::StreamExt;
        let (state, key, model) = usage_fixture().await;
        let frame = "data: {\"error\":{\"message\":\"fixture stream failure\"}}\n\n";
        let upstream = Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(async_stream::stream! {
                yield Ok::<_,std::io::Error>(frame);
                std::future::pending::<()>().await;
            }))
            .unwrap();
        let response = crate::proxy::record_response(
            &state,
            &key,
            &model,
            "fixture",
            std::time::Instant::now(),
            upstream,
            0,
            None,
        )
        .await
        .unwrap();
        let mut stream = response.into_body().into_data_stream();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .as_ref(),
            frame.as_bytes()
        );
        drop(stream);
        let row: (i64, String) =
            sqlx::query_as("SELECT status_code,error_message FROM user_request_logs")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(row, (502, "fixture stream failure".into()));
    }

    #[tokio::test]
    async fn functional_recheck_cancelled_streams_are_logged_once_without_guessing_cost() {
        use axum::{body::Body, response::Response};
        use futures_util::StreamExt;
        let (state, key, model) = usage_fixture().await;
        for consume_first in [false, true] {
            let upstream = Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(async_stream::stream! {
                    yield Ok::<_,std::io::Error>("data: {\"usage\":{\"prompt_tokens\":9}}\n\n");
                    std::future::pending::<()>().await;
                }))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                0,
                None,
            )
            .await
            .unwrap();
            if consume_first {
                let mut stream = response.into_body().into_data_stream();
                tokio::time::timeout(Duration::from_secs(1), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                drop(stream);
            } else {
                drop(response);
            }
            let expected = if consume_first { 2 } else { 1 };
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs")
                        .fetch_one(&state.pool)
                        .await
                        .unwrap();
                    if count == expected {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        let rows:Vec<(i64,i64,i64,Option<f64>)>=sqlx::query_as("SELECT status_code,input_tokens,output_tokens,cost_usd FROM user_request_logs ORDER BY id").fetch_all(&state.pool).await.unwrap();
        assert_eq!(rows, vec![(499, 0, 0, None), (499, 9, 0, None)]);
        let count: i64 = sqlx::query_scalar("SELECT total_requests FROM virtual_keys WHERE id=?")
            .bind(key.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn functional_recheck_final_stream_log_survives_delivery_cancellation() {
        use axum::{body::Body, response::Response};
        use futures_util::StreamExt;
        let (state, key, model) = usage_fixture().await;
        let blocked_database = state.pool.acquire().await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let upstream=Response::builder().header("content-type","text/event-stream")
            .body(Body::from_stream(async_stream::stream! {
                signal.notify_one();
                yield Ok::<_,std::io::Error>("data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n");
                std::future::pending::<()>().await;
            })).unwrap();
        let response = crate::proxy::record_response(
            &state,
            &key,
            &model,
            "fixture",
            std::time::Instant::now(),
            upstream,
            0,
            None,
        )
        .await
        .unwrap();
        let reader =
            tokio::spawn(async move { response.into_body().into_data_stream().next().await });
        entered.notified().await;
        reader.abort();
        let _ = reader.await;
        drop(blocked_database);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs")
                    .fetch_one(&state.pool)
                    .await
                    .unwrap();
                if count == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let row: (i64, i64, i64, f64) = sqlx::query_as(
            "SELECT status_code,input_tokens,output_tokens,cost_usd FROM user_request_logs",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!((row.0, row.1, row.2), (200, 12, 3));
        assert!((row.3 - 0.000018).abs() < 1e-12);
        let count: i64 = sqlx::query_scalar("SELECT total_requests FROM virtual_keys WHERE id=?")
            .bind(key.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn functional_recheck_concurrent_task_polls_return_the_persisted_terminal_result() {
        use axum::{
            body::to_bytes,
            http::HeaderMap,
            routing::{get, post},
            Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (winner, late) in [("failed", "succeeded"), ("succeeded", "processing")] {
            let (mut state, _, previous) = usage_fixture().await;
            state.client = Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            state.sub2_redis = Some(
                redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                    .unwrap()
                    .get_connection_manager()
                    .await
                    .unwrap(),
            );
            let release = Arc::new(tokio::sync::Notify::new());
            let gate = release.clone();
            let both_started = Arc::new(tokio::sync::Barrier::new(2));
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener,Router::new()
                    .route("/submit",post(||async {Json(json!({"task_id":"vendor-task","status":"queued"}))}))
                    .route("/tasks/{id}",get(move || {let count=count.clone();let gate=gate.clone();let both_started=both_started.clone();async move {
                        let first=count.fetch_add(1,Ordering::SeqCst)==0;
                        both_started.wait().await;
                        if !first {gate.notified().await;}
                        Json(json!({"status":if first {winner} else {late},"result":{"usage":{"total_tokens":if first {10} else {20}}}}))
                    }}))).await.unwrap();
            });
            sqlx::query("UPDATE user_providers SET base_url=? WHERE id=?")
                .bind(format!("http://{address}"))
                .bind(previous.provider_id)
                .execute(&state.pool)
                .await
                .unwrap();
            update_model(&state,1,json!({"id":previous.id,"request_path":"/submit","api_config":{"api_type":"embeddings","response_mode":"task","task_path":"/tasks/{task_id}"}})).await.unwrap();
            let model = resolve_model(&state, 1, &previous.alias).await.unwrap();
            let account = crate::direct_provider::keys(&state, model.provider_id)
                .await
                .unwrap()
                .remove(0);
            let created = create_key(&state, 1, json!({"name":"concurrent task"}))
                .await
                .unwrap();
            let raw = created["key"].as_str().unwrap();
            let caller = gateway_key(&state, raw).await.unwrap().unwrap();
            let submitted = crate::gateway_tasks::submit(
                &state,
                &caller,
                &model,
                &account,
                state
                    .client
                    .post(format!("http://{address}/submit"))
                    .json(&json!({"input":"hello"})),
            )
            .await
            .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &caller,
                &model,
                "fixture",
                std::time::Instant::now(),
                submitted,
                0,
                None,
            )
            .await
            .unwrap();
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap())
                    .unwrap();
            let id = value["task_id"].as_str().unwrap().to_owned();
            let mut headers = HeaderMap::new();
            headers.insert("authorization", format!("Bearer {raw}").parse().unwrap());
            let poll = || {
                let state = state.clone();
                let headers = headers.clone();
                let id = id.clone();
                let release = release.clone();
                async move {
                    let response = crate::gateway_tasks::query(State(state), Path(id), headers)
                        .await
                        .unwrap();
                    release.notify_one();
                    serde_json::from_slice::<Value>(
                        &to_bytes(response.into_body(), 10000).await.unwrap(),
                    )
                    .unwrap()
                }
            };
            let (one, two) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(poll(), poll())
            })
            .await
            .unwrap();
            assert_eq!(one["status"], winner);
            assert_eq!(two["status"], winner);
            assert_eq!(one, two);
            let totals: (i64, i64, f64) = sqlx::query_as(
                "SELECT total_requests,input_tokens,cost_usd FROM virtual_keys WHERE id=?",
            )
            .bind(caller.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
            assert_eq!((totals.0, totals.1), (1, 10));
            assert!((totals.2 - 0.00001).abs() < 1e-12);
            let status: i64 = sqlx::query_scalar("SELECT status_code FROM user_request_logs")
                .fetch_one(&state.pool)
                .await
                .unwrap();
            assert_eq!(status, if winner == "failed" { 502 } else { 200 });
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            server.abort();
        }
    }

    #[tokio::test]
    async fn gateway_audit_missing_usage_is_never_replaced_with_estimated_tokens() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        for (status, content_type, body) in [
            (
                200,
                "application/json",
                "{\"choices\":[{\"message\":{\"content\":\"OK\"}}]}",
            ),
            (
                429,
                "application/json",
                "{\"error\":{\"message\":\"limited\"}}",
            ),
            (
                200,
                "text/event-stream",
                "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n",
            ),
        ] {
            let upstream = Response::builder()
                .status(status)
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                9999,
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap()
                    .as_ref(),
                body.as_bytes()
            );
        }
        let logged: Vec<(i64, i64, Option<f64>)> = sqlx::query_as(
            "SELECT input_tokens,output_tokens,cost_usd FROM user_request_logs ORDER BY id",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap();
        assert_eq!(logged, vec![(0, 0, None); 3]);
        let totals: (i64,i64,i64,f64,i64) = sqlx::query_as(
            "SELECT total_requests,input_tokens,output_tokens,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?")
            .bind(key.id).fetch_one(&state.pool).await.unwrap();
        assert_eq!(totals, (3, 0, 0, 0.0, 3));
    }

    #[tokio::test]
    async fn gateway_audit_price_is_frozen_before_response_not_after_route_edit() {
        let (state, key, model) = usage_fixture().await;
        update_model(
            &state,
            1,
            json!({"id":model.id,"input_price_per_million":100,"output_price_per_million":200}),
        )
        .await
        .unwrap();
        record_request(&state, &key, &model, "fixture", 200, 1, "", 1000, 500)
            .await
            .unwrap();
        let logged: (f64,f64,f64) = sqlx::query_as(
            "SELECT cost_usd,input_price_per_million,output_price_per_million FROM user_request_logs")
            .fetch_one(&state.pool).await.unwrap();
        assert!((logged.0 - 0.002).abs() < 1e-12);
        assert_eq!((logged.1, logged.2), (1.0, 2.0));
        let next = resolve_model(&state, 1, &model.alias).await.unwrap();
        record_request(&state, &key, &next, "fixture", 200, 1, "", 1000, 500)
            .await
            .unwrap();
        let next_cost: f64 =
            sqlx::query_scalar("SELECT cost_usd FROM user_request_logs ORDER BY id DESC LIMIT 1")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!((next_cost - 0.2).abs() < 1e-12);
    }

    #[tokio::test]
    async fn gateway_audit_partial_usage_does_not_become_a_complete_bill() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        for (content_type, body) in [
            ("application/json", "{\"usage\":{\"prompt_tokens\":1000}}"),
            (
                "text/event-stream",
                "data: {\"usage\":{\"prompt_tokens\":1000}}\n\ndata: [DONE]\n\n",
            ),
            (
                "application/json",
                "{\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":0}}",
            ),
        ] {
            let upstream = Response::builder()
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                999,
                None,
            )
            .await
            .unwrap();
            to_bytes(response.into_body(), 10000).await.unwrap();
        }
        let logs: Vec<(i64, Option<f64>)> =
            sqlx::query_as("SELECT input_tokens,cost_usd FROM user_request_logs ORDER BY id")
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(logs, vec![(1000, None), (1000, None), (1000, Some(0.001))]);
        let totals:(i64,i64,f64,i64)=sqlx::query_as("SELECT total_requests,input_tokens,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?").bind(key.id).fetch_one(&state.pool).await.unwrap();
        assert_eq!(totals, (3, 3000, 0.001, 2));
    }

    #[tokio::test]
    async fn gateway_audit_completed_submission_is_cached_and_settled_without_polling() {
        use axum::{body::to_bytes, http::HeaderMap, routing::post, Router};
        let (mut state, _, previous) = usage_fixture().await;
        state.client = Client::builder().no_proxy().build().unwrap();
        update_model(&state,1,json!({"id":previous.id,"request_path":"/submit","api_config":{"api_type":"embeddings","response_mode":"task","task_path":"/tasks/{task_id}"}})).await.unwrap();
        let model = resolve_model(&state, 1, &previous.alias).await.unwrap();
        let created = create_key(&state, 1, json!({"name":"completed submission"}))
            .await
            .unwrap();
        let raw = created["key"].as_str().unwrap();
        let caller = gateway_key(&state, raw).await.unwrap().unwrap();
        let account = crate::direct_provider::keys(&state, model.provider_id)
            .await
            .unwrap()
            .remove(0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new().route("/submit",post(||async {
                Json(json!({"task_id":"vendor-task","status":"succeeded","result":{"data":[{"object":"embedding","embedding":[0.1]}],"usage":{"total_tokens":1200}}}))
            }))).await.unwrap();
        });
        // Change the stored route before submission: the already resolved request
        // still uses its original price snapshot.
        update_model(
            &state,
            1,
            json!({"id":model.id,"input_price_per_million":100}),
        )
        .await
        .unwrap();
        let submitted = crate::gateway_tasks::submit(
            &state,
            &caller,
            &model,
            &account,
            state
                .client
                .post(format!("http://{address}/submit"))
                .json(&json!({"input":"hello"})),
        )
        .await
        .unwrap();
        let response = crate::proxy::record_response(
            &state,
            &caller,
            &model,
            "fixture",
            std::time::Instant::now(),
            submitted,
            999,
            None,
        )
        .await
        .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        let task_id = value["task_id"].as_str().unwrap();
        let stored: (i64, String) =
            sqlx::query_as("SELECT settled,encrypted_api_key FROM gateway_tasks WHERE id=?")
                .bind(task_id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(stored, (1, String::new()));
        for _ in 0..2 {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", format!("Bearer {raw}").parse().unwrap());
            let response =
                crate::gateway_tasks::query(State(state.clone()), Path(task_id.into()), headers)
                    .await
                    .unwrap();
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap())
                    .unwrap();
            assert_eq!(body["task_id"], task_id);
            assert_eq!(body["status"], "succeeded");
        }
        let totals:(i64,i64,f64,i64)=sqlx::query_as("SELECT total_requests,input_tokens,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?")
            .bind(caller.id).fetch_one(&state.pool).await.unwrap();
        assert_eq!((totals.0, totals.1, totals.3), (1, 1200, 0));
        assert!((totals.2 - 0.0012).abs() < 1e-12);
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn gateway_audit_media_requests_are_not_replayed_after_ambiguous_failures() {
        use axum::{
            body::Body,
            http::{HeaderMap, Method, StatusCode},
            routing::post,
            Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        state.client = Client::builder()
            .no_proxy()
            .read_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/generate",
                    post(move |Json(body): Json<Value>| {
                        let count = count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            if body["model"] == "timeout" {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                            (
                                StatusCode::GATEWAY_TIMEOUT,
                                Body::from("unknown generation status"),
                            )
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        for (index, kind, path, fault) in [
            (
                0,
                crate::model_api::ApiType::ImagesGenerations,
                "/v1/images/generations",
                "http",
            ),
            (
                1,
                crate::model_api::ApiType::ImagesEdits,
                "/v1/images/edits",
                "http",
            ),
            (
                2,
                crate::model_api::ApiType::VideosGenerations,
                "/v1/videos/generations",
                "timeout",
            ),
            (
                3,
                crate::model_api::ApiType::AudioSpeech,
                "/v1/audio/speech",
                "http",
            ),
            (
                4,
                crate::model_api::ApiType::AudioTranscriptions,
                "/v1/audio/transcriptions",
                "timeout",
            ),
            (
                5,
                crate::model_api::ApiType::Auto,
                "/v1/images/generations",
                "http",
            ),
        ] {
            let pid=create_provider(&state,1,json!({"name":format!("media-{index}"),"provider_type":"openai","base_url":format!("http://{address}"),"api_key":"first-fixture"})).await.unwrap()["id"].as_i64().unwrap();
            crate::direct_provider::create(
                &state,
                pid,
                &json!({"name":"backup","api_key":"backup-fixture"}),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE provider_keys SET assigned=1,priority=1 WHERE provider_id=?")
                .bind(pid)
                .execute(&state.pool)
                .await
                .unwrap();
            let config = crate::model_api::ApiConfig {
                api_type: kind,
                ..Default::default()
            };
            let before = hits.load(Ordering::SeqCst);
            let headers = HeaderMap::new();
            let payload = json!({"model":fault});
            let result = crate::direct_provider::forward_route(
                &state,
                crate::direct_provider::ForwardRequest {
                    provider: pid,
                    method: Method::POST,
                    path,
                    headers: &headers,
                    payload: &payload,
                    route: crate::direct_provider::ForwardRoute {
                        request_path: "/generate",
                        config: &config,
                        task_owner: None,
                    },
                    upload: None,
                },
            )
            .await;
            if fault == "timeout" {
                assert!(matches!(result, Err(AppError::GatewayTimeout(_))));
            } else {
                assert_eq!(result.unwrap().status(), 504);
            }
            assert_eq!(
                hits.load(Ordering::SeqCst) - before,
                1,
                "media generation must be submitted once"
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn supplier_cookies_auth_and_connection_headers_never_reach_clients() {
        use axum::{body::Body, response::Response};
        let (state, key, model) = usage_fixture().await;
        let upstream = Response::builder()
            .header("set-cookie", "supplier-session=private")
            .header("authorization", "Bearer supplier-secret")
            .header("x-api-key", "supplier-secret")
            .header("connection", "x-private")
            .header("x-private", "private-value")
            .header("x-request-id", "safe-request-id")
            .body(Body::from("{}"))
            .unwrap();
        let response = crate::proxy::record_response(
            &state,
            &key,
            &model,
            "fixture",
            std::time::Instant::now(),
            upstream,
            0,
            None,
        )
        .await
        .unwrap();
        for header in [
            "set-cookie",
            "authorization",
            "x-api-key",
            "connection",
            "x-private",
        ] {
            assert!(response.headers().get(header).is_none());
        }
        assert_eq!(response.headers()["x-request-id"], "safe-request-id");
    }

    #[tokio::test]
    async fn embedding_accounting_is_input_only_and_missing_usage_is_unpriced() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        let configured = update_model(&state, 1, json!({"id":model.id,"api_config":{"api_type":"embeddings"},"input_price_per_million":2.5,"output_price_per_million":999})).await.unwrap();
        assert!(configured["output_price_per_million"].is_null());
        let model = resolve_model(&state, 1, &model.alias).await.unwrap();
        let bodies = [
            json!({"data":[{"object":"embedding","embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":1000,"total_tokens":1000,"completion_tokens":99999}}),
            json!({"data":[{"object":"embedding","embedding":[0.1,0.2,0.3]}],"usage":{"total_tokens":2000}}),
            json!({"data":[{"object":"embedding","embedding":[0.1,0.2,0.3]}]}),
        ];
        for body in bodies {
            let raw = body.to_string();
            let upstream = Response::new(Body::from(raw.clone()));
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                999,
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap()
                    .as_ref(),
                raw.as_bytes()
            );
        }
        let logged: Vec<(i64, i64, i64, Option<f64>)> = sqlx::query_as("SELECT input_tokens,output_tokens,total_tokens,cost_usd FROM user_request_logs ORDER BY id")
            .fetch_all(&state.pool).await.unwrap();
        assert_eq!((logged[0].0, logged[0].1, logged[0].2), (1000, 0, 1000));
        assert!((logged[0].3.unwrap() - 0.0025).abs() < 1e-12);
        assert_eq!((logged[1].0, logged[1].1, logged[1].2), (2000, 0, 2000));
        assert!((logged[1].3.unwrap() - 0.005).abs() < 1e-12);
        assert_eq!(logged[2], (0, 0, 0, None));
        // Even direct accounting callers cannot charge vector output as tokens.
        record_request(&state, &key, &model, "fixture", 200, 1, "", 1000, 99999)
            .await
            .unwrap();
        let totals: (i64,i64,i64,f64,i64) = sqlx::query_as("SELECT total_requests,input_tokens,output_tokens,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?")
            .bind(key.id).fetch_one(&state.pool).await.unwrap();
        assert_eq!((totals.0, totals.1, totals.2, totals.4), (4, 4000, 0, 1));
        assert!((totals.3 - 0.01).abs() < 1e-12);
        let partial = update_model(
            &state,
            1,
            json!({"id":model.id,"alias":"embedding-renamed","output_price_per_million":777}),
        )
        .await
        .unwrap();
        assert!(partial["output_price_per_million"].is_null());
    }

    #[tokio::test]
    async fn embedding_tasks_use_submission_price_and_settle_once_after_route_edit() {
        use axum::{body::to_bytes, http::HeaderMap, routing::post, Router};
        let (state, _, previous) = usage_fixture().await;
        update_model(&state,1,json!({"id":previous.id,"request_path":"/submit","api_config":{"api_type":"embeddings","response_mode":"task","task_path":"/tasks/{task_id}"},"input_price_per_million":2.5})).await.unwrap();
        let model = resolve_model(&state, 1, &previous.alias).await.unwrap();
        let created = create_key(&state, 1, json!({"name":"embedding task"}))
            .await
            .unwrap();
        let secret = created["key"].as_str().unwrap();
        let key = gateway_key(&state, secret).await.unwrap().unwrap();
        let account = crate::direct_provider::keys(&state, model.provider_id)
            .await
            .unwrap()
            .remove(0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/submit",
                    post(|| async {
                        Json(json!({"task_id":"upstream-embedding","status":"queued"}))
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let response = crate::gateway_tasks::submit(
            &state,
            &key,
            &model,
            &account,
            state
                .client
                .post(format!("http://{address}/submit"))
                .json(&json!({"input":["first","second"]})),
        )
        .await
        .unwrap();
        let task: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        let task_id = task["task_id"].as_str().unwrap();
        let log_id = record_request(&state, &key, &model, "fixture", 202, 1, "", 0, 0)
            .await
            .unwrap();
        crate::gateway_tasks::attach_log(&state, task_id, log_id)
            .await
            .unwrap();
        let cached = json!({"task_id":task_id,"status":"succeeded","result":{"data":[{"object":"embedding","embedding":[0.1,0.2]}],"usage":{"total_tokens":1200,"output_tokens":999}}});
        sqlx::query("UPDATE gateway_tasks SET status='succeeded',result_json=? WHERE id=?")
            .bind(cached.to_string())
            .bind(task_id)
            .execute(&state.pool)
            .await
            .unwrap();
        let snapshot: String = sqlx::query_scalar("SELECT api_type FROM gateway_tasks WHERE id=?")
            .bind(task_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(snapshot, "embeddings");
        update_model(&state,1,json!({"id":model.id,"input_price_per_million":100,"request_path":"/chat","api_config":{"api_type":"chat_completions"}})).await.unwrap();
        for _ in 0..2 {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", format!("Bearer {secret}").parse().unwrap());
            let response =
                crate::gateway_tasks::query(State(state.clone()), Path(task_id.into()), headers)
                    .await
                    .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap()
                    .as_ref(),
                cached.to_string().as_bytes()
            );
        }
        let totals: (i64,i64,i64,f64,i64) = sqlx::query_as("SELECT total_requests,input_tokens,output_tokens,cost_usd,unpriced_requests FROM virtual_keys WHERE id=?")
            .bind(key.id).fetch_one(&state.pool).await.unwrap();
        assert_eq!((totals.0, totals.1, totals.2, totals.4), (1, 1200, 0, 0));
        assert!((totals.3 - 0.003).abs() < 1e-12);
        server.abort();
    }

    #[tokio::test]
    async fn usage_accounting_reads_sse_without_correct_content_type() {
        use axum::{
            body::{to_bytes, Body},
            response::Response,
        };
        let (state, key, model) = usage_fixture().await;
        let frame = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":18,\"output_tokens\":5}}}\n\n";
        for content_type in [None, Some("application/json")] {
            let mut upstream = Response::new(Body::from(frame));
            if let Some(content_type) = content_type {
                upstream
                    .headers_mut()
                    .insert("content-type", content_type.parse().unwrap());
            }
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                999,
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap()
                    .as_ref(),
                frame.as_bytes()
            );
        }
        let totals: (i64, i64, i64) = sqlx::query_as(
            "SELECT total_requests,input_tokens,output_tokens FROM virtual_keys WHERE id=?",
        )
        .bind(key.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(totals, (2, 36, 10));
        let usage: (i64, i64, i64, f64) = sqlx::query_as(
            "SELECT input_tokens,output_tokens,total_tokens,cost_usd FROM user_request_logs ORDER BY id DESC LIMIT 1"
        ).fetch_one(&state.pool).await.unwrap();
        assert_eq!((usage.0, usage.1, usage.2), (18, 5, 23));
        assert!((usage.3 - 0.000028).abs() < 1e-12);
    }

    #[tokio::test]
    async fn usage_accounting_persists_before_stream_client_disconnects() {
        use axum::{
            body::{Body, Bytes},
            response::Response,
        };
        use futures_util::StreamExt;
        let (state, key, model) = usage_fixture().await;
        let frames = [
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":18,\"output_tokens\":5}}}\n\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":18}}}\n\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":18,\"completion_tokens\":5}}\r\n\r\ndata: [DONE]\r\n\r\n",
        ];
        for frame in frames {
            let first = Bytes::copy_from_slice(&frame.as_bytes()[..37]);
            let last = Bytes::copy_from_slice(&frame.as_bytes()[37..]);
            let upstream = Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(async_stream::stream! {
                    yield Ok::<_, std::io::Error>(first);
                    yield Ok::<_, std::io::Error>(last);
                    std::future::pending::<()>().await;
                }))
                .unwrap();
            let response = crate::proxy::record_response(
                &state,
                &key,
                &model,
                "fixture",
                std::time::Instant::now(),
                upstream,
                999,
                None,
            )
            .await
            .unwrap();
            let mut stream = response.into_body().into_data_stream();
            let mut received = Vec::new();
            while received.len() < frame.len() {
                let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                received.extend_from_slice(&chunk);
            }
            assert_eq!(received, frame.as_bytes());
            drop(stream); // A client stops at the terminal event without waiting for EOF.
            let usage: (i64, i64, i64) = sqlx::query_as(
                "SELECT input_tokens,output_tokens,total_tokens FROM user_request_logs ORDER BY id DESC LIMIT 1"
            ).fetch_one(&state.pool).await.unwrap();
            assert_eq!(usage, (18, 5, 23));
        }
        let totals: (i64, i64, i64) = sqlx::query_as(
            "SELECT total_requests,input_tokens,output_tokens FROM virtual_keys WHERE id=?",
        )
        .bind(key.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(totals, (3, 54, 15));
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn model_request_path_forwards_to_configured_endpoint() {
        use axum::{
            body::{to_bytes, Body},
            http::HeaderMap,
            routing::post,
            Json, Router,
        };
        let mut state = state().await;
        let redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/custom/chat", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-provider-key");
            assert_eq!(body["model"], "real-upstream-model");
            assert_eq!(body["messages"][0]["content"], "hello");
            Json(json!({"choices":[{"message":{"role":"assistant","content":"configured-path-ok"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}))
        }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state.client = Client::builder().no_proxy().build().unwrap();
        let provider = create_provider(&state,1,json!({"name":"path test","provider_type":"openai","base_url":format!("http://{address}/v1"),"api_key":"fixture-provider-key"})).await.unwrap()["id"].as_i64().unwrap();
        create_model(&state,1,json!({"provider_id":provider,"alias":"alias-to-call","upstream_model":"real-upstream-model","request_path":"/custom/chat"})).await.unwrap();
        let key = create_key(&state, 1, json!({"name":"path test key"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::from(
                json!({"model":"alias-to-call","messages":[{"role":"user","content":"hello"}]})
                    .to_string(),
            ))
            .unwrap();
        let response = crate::proxy::passthrough(State(state.clone()), request)
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "configured-path-ok"
        );
        task.abort();
    }
    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn relay_file_requests_preserve_bytes_scope_path_billing_and_no_replay() {
        use axum::{
            body::{to_bytes, Body},
            http::{Request, StatusCode},
            routing::post,
            Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        let redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        state.client = Client::builder().no_proxy().build().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let count = calls.clone();
        let app=Router::new().route("/vendor/{operation}",post(move |request:Request<Body>| {
            let count=count.clone();async move {
                count.fetch_add(1,Ordering::SeqCst);
                let operation=request.uri().path().to_owned();
                let headers=request.headers();
                assert_eq!(headers["authorization"],"Bearer fixture-provider-key");
                for name in ["cookie","x-api-key","proxy-authorization","x-private-hop"] { assert!(headers.get(name).is_none(),"{name} leaked"); }
                let boundary=multer::parse_boundary(headers["content-type"].to_str().unwrap()).unwrap();
                assert_ne!(boundary,"fixture-boundary");
                let bytes=to_bytes(request.into_body(),crate::gateway_upload::MAX_BODY).await.unwrap();
                let mut parser=multer::Multipart::new(futures_util::stream::once(async move {Ok::<_,std::convert::Infallible>(bytes)}),boundary);
                let mut parts=vec![];
                while let Some(field)=parser.next_field().await.unwrap() {
                    let name=field.name().unwrap().to_owned();let filename=field.file_name().map(str::to_owned);
                    let mime=field.content_type().map(ToString::to_string);
                    let bytes=field.bytes().await.unwrap();parts.push((name,filename,mime,bytes));
                }
                assert_eq!(parts[0].3.as_ref(),b"real-media-model");
                assert_eq!(parts[1].1.as_deref(),Some("original.bin"));
                assert_eq!(parts[1].2.as_deref(),Some("application/octet-stream"));
                assert_eq!(parts[1].3.as_ref(),b"\0\xffFILE_BYTES_ONLY");
                assert_eq!(parts[2].0,parts[1].0);assert_eq!(parts[2].3.as_ref(),b"second-file");
                assert_eq!(parts[3].0,"custom_parameter");assert_eq!(parts[3].3.as_ref(),b"keep this value");
                if operation.ends_with("fail") { (StatusCode::BAD_GATEWAY,axum::Json(json!({"error":{"message":"fixture failure"}}))) }
                else { (StatusCode::OK,axum::Json(json!({"data":[{"url":"https://example.test/result.png"}],"usage":{"seconds":2}}))) }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider=create_provider(&state,1,json!({"name":"file fixture","provider_type":"openai","base_url":format!("http://{address}/v1"),"api_key":"fixture-provider-key"})).await.unwrap()["id"].as_i64().unwrap();
        crate::direct_provider::create(
            &state,
            provider,
            &json!({"name":"second","api_key":"fixture-provider-key"}),
        )
        .await
        .unwrap();
        for (alias, kind, path, billing) in [
            (
                "image-edit",
                "images_edits",
                "/vendor/edit",
                json!({"unit":"image","price_per_unit":0.04}),
            ),
            (
                "audio-transcribe",
                "audio_transcriptions",
                "/vendor/transcribe",
                json!({"unit":"second","price_per_unit":0.01,"quantity_pointer":"/usage/seconds"}),
            ),
            (
                "image-fail",
                "images_edits",
                "/vendor/fail",
                json!({"unit":"image","price_per_unit":0.04}),
            ),
        ] {
            create_model(&state,1,json!({"provider_id":provider,"alias":alias,"upstream_model":"real-media-model","request_path":path,"api_config":{"api_type":kind},"billing_config":billing})).await.unwrap();
        }
        let key=create_key(&state,1,json!({"name":"file key","allowed_models":["image-edit","audio-transcribe","image-fail"]})).await.unwrap()["key"].as_str().unwrap().to_owned();
        let request = |alias: &str, path: &str| {
            let (headers, body) = crate::gateway_upload::tests::body(&[
                ("model", None, alias.as_bytes()),
                ("file", Some("original.bin"), b"\0\xffFILE_BYTES_ONLY"),
                ("file", Some("second.bin"), b"second-file"),
                ("custom_parameter", None, b"keep this value"),
            ]);
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", &headers["content-type"])
                .header("cookie", "private-cookie")
                .header("x-api-key", "virtual-not-upstream")
                .header("proxy-authorization", "private-proxy")
                .header("connection", "x-private-hop")
                .header("x-private-hop", "private-hop")
                .body(Body::from(body))
                .unwrap()
        };
        for (alias, path) in [
            ("image-edit", "/v1/images/edits"),
            ("audio-transcribe", "/v1/audio/transcriptions"),
        ] {
            let response = crate::proxy::passthrough(State(state.clone()), request(alias, path))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            to_bytes(response.into_body(), 10000).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Invalid format, wrong endpoint, unavailable alias and oversized bodies never hit the supplier.
        assert!(crate::proxy::passthrough(
            State(state.clone()),
            request("blocked-alias", "/v1/images/edits")
        )
        .await
        .is_err());
        assert!(crate::proxy::passthrough(
            State(state.clone()),
            request("image-edit", "/v1/chat/completions")
        )
        .await
        .is_err());
        assert!(crate::proxy::passthrough(
            State(state.clone()),
            request("image-edit", "/v1/audio/transcriptions")
        )
        .await
        .is_err());
        let too_large = Request::builder()
            .method("POST")
            .uri("/v1/images/edits")
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from(vec![0; crate::gateway_upload::MAX_BODY + 1]))
            .unwrap();
        assert!(matches!(
            crate::proxy::passthrough(State(state.clone()), too_large).await,
            Err(AppError::PayloadTooLarge(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let response = crate::proxy::passthrough(
            State(state.clone()),
            request("image-fail", "/v1/images/edits"),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 502);
        to_bytes(response.into_body(), 10000).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "media failure must not replay onto the second key"
        );
        let logs = list_logs(&state, 1, &json!({})).await.unwrap();
        assert_eq!(logs["total"], 3);
        assert!(!logs.to_string().contains("FILE_BYTES_ONLY"));
        assert!(!logs.to_string().contains("original.bin"));
        let (requests, cost): (i64, f64) = sqlx::query_as(
            "SELECT total_requests,cost_usd FROM virtual_keys WHERE name='file key'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(requests, 3);
        assert!(
            (cost - 0.06).abs() < 1e-10,
            "actual image and second charges: {cost}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn gateway_catalog_metadata_is_scoped_safe_and_has_detail() {
        use axum::extract::Path;
        let state = state().await;
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let pid=create_provider(&state,1,json!({"name":"private supplier","provider_type":"openai","base_url":"https://private.test/v1","api_key":"private-secret"})).await.unwrap()["id"].as_i64().unwrap();
        for alias in ["image/model", "not-allowed"] {
            create_model(&state,1,json!({"provider_id":pid,"alias":alias,"upstream_model":"private-upstream","request_path":"/private/edit","api_config":{"api_type":"images_edits"},"billing_config":{"unit":"image","price_per_unit":0.05}})).await.unwrap();
        }
        let key = create_key(
            &state,
            2,
            json!({"name":"scoped catalog","allowed_models":["image/model"]}),
        )
        .await
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", key["key"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        let list = crate::proxy::models(State(state.clone()), headers.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(list["data"].as_array().unwrap().len(), 1);
        let model = &list["data"][0];
        assert_eq!(model["id"], "image/model");
        assert_eq!(model["object"], "model");
        assert_eq!(model["junohub"]["endpoint"], "/v1/images/edits");
        assert_eq!(
            model["junohub"]["request_formats"],
            json!(["application/json", "multipart/form-data"])
        );
        assert_eq!(model["junohub"]["billing_config"]["price_per_unit"], 0.05);
        assert!(!list.to_string().contains("private"));
        assert!(model["junohub"].get("provider_id").is_none());
        let detail = crate::proxy::model(
            State(state.clone()),
            Path("image/model".into()),
            headers.clone(),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(&detail, model);
        // 越权返回 403，客户端才不会把“没授权这个模型”当成“密钥无效”。
        // 越权请求一律 403，也不透露该模型是否存在。
        for alias in ["not-allowed", "missing"] {
            assert!(matches!(
                crate::proxy::model(
                    State(state.clone()),
                    Path(alias.into()),
                    headers.clone()
                )
                .await,
                Err(AppError::Forbidden(_))
            ));
        }
        // 在授权范围内但确实不存在的模型，仍然是 404。
        let wide = create_key(
            &state,
            2,
            json!({"name":"wide catalog","allowed_models":["*"]}),
        )
        .await
        .unwrap();
        let mut wide_headers = HeaderMap::new();
        wide_headers.insert(
            "authorization",
            format!("Bearer {}", wide["key"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            crate::proxy::model(
                State(state.clone()),
                Path("missing".into()),
                wide_headers
            )
            .await,
            Err(AppError::NotFound(_))
        ));
        sqlx::query("UPDATE user_providers SET enabled=0 WHERE id=?")
            .bind(pid)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(crate::proxy::models(State(state.clone()), headers.clone())
            .await
            .unwrap()
            .0["data"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            crate::proxy::model(State(state), Path("image/model".into()), headers)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn only_admin_can_set_user_limits_and_demotion_revokes_access() {
        let state = state().await;
        assert!(matches!(
            resource_limits(
                &state,
                1,
                "user",
                1,
                true,
                &json!({"max_concurrency":2,"rpm":10})
            )
            .await,
            Err(AppError::Forbidden(_))
        ));
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            resource_limits(
                &state,
                1,
                "user",
                2,
                true,
                &json!({"max_concurrency":2,"rpm":10})
            )
            .await
            .unwrap()["rpm"],
            10
        );
        assert!(matches!(
            resource_limits(&state, 2, "user", 2, false, &json!({})).await,
            Err(AppError::Forbidden(_))
        ));
        assert!(resource_limits(
            &state,
            1,
            "user",
            99999,
            true,
            &json!({"max_concurrency":2,"rpm":10})
        )
        .await
        .is_err());
        sqlx::query("UPDATE users SET role='user' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(matches!(
            resource_limits(&state, 1, "user", 2, false, &json!({})).await,
            Err(AppError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn monitor_key_limits_and_request_id_search_preserve_ownership() {
        let (state, key, model) = usage_fixture().await;
        assert_eq!(
            resource_limits(&state, 1, "key", key.id, false, &json!({}))
                .await
                .unwrap()["rpm"],
            0
        );
        assert!(resource_limits(
            &state,
            2,
            "key",
            key.id,
            true,
            &json!({"rpm":1,"max_concurrency":1})
        )
        .await
        .is_err());
        let limits = resource_limits(
            &state,
            1,
            "key",
            key.id,
            true,
            &json!({"rpm":5,"max_concurrency":2}),
        )
        .await
        .unwrap();
        assert_eq!(limits["rpm"], 5);
        assert!(resource_limits(
            &state,
            1,
            "key",
            key.id,
            true,
            &json!({"rpm":-1,"max_concurrency":2})
        )
        .await
        .is_err());
        record_request(&state, &key, &model, "openai", 200, 10, "", 1, 1)
            .await
            .unwrap();
        assert_eq!(
            list_logs(&state, 1, &json!({"request_id":model.request_id}))
                .await
                .unwrap()["total"],
            1
        );
        assert_eq!(
            list_logs(
                &state,
                2,
                &json!({"request_id":model.request_id,"user_id":1})
            )
            .await
            .unwrap()["total"],
            0
        );
        assert_eq!(
            list_logs(&state, 1, &json!({"request_id":"not-found"}))
                .await
                .unwrap()["total"],
            0
        );
        sqlx::query("UPDATE users SET role='admin' WHERE id=2")
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            resource_limits(&state, 2, "key", key.id, false, &json!({}))
                .await
                .unwrap()["max_concurrency"],
            2
        );
        delete_key(&state, 2, key.id).await.unwrap();
        assert_eq!(
            crate::gateway_limits::read(&state, "key", key.id)
                .await
                .unwrap()
                .rpm,
            0
        );
    }

    #[tokio::test]
    async fn model_request_path_crud_preserves_settings_and_ownership() {
        let state = state().await;
        let p = create_provider(&state, 1, json!({"name":"path fixture","provider_type":"openai","base_url":"https://example.test/v1","api_key":"fixture"})).await.unwrap();
        let provider_id = p["id"].as_i64().unwrap();
        let model = create_model(&state,1,json!({"provider_id":provider_id,"alias":"custom-path","upstream_model":"real-model","request_path":"/api/chat"})).await.unwrap();
        let id = model["id"].as_i64().unwrap();
        assert_eq!(
            resolve_model(&state, 1, "custom-path")
                .await
                .unwrap()
                .request_path,
            "/api/chat"
        );
        assert_eq!(
            list_models(&state, 1).await.unwrap()["list"][0]["request_path"],
            "/api/chat"
        );
        update_model(&state, 1, json!({"id":id,"alias":"renamed-path"}))
            .await
            .unwrap();
        assert_eq!(
            resolve_model(&state, 1, "renamed-path")
                .await
                .unwrap()
                .request_path,
            "/api/chat"
        );
        assert!(
            update_model(&state, 2, json!({"id":id,"request_path":"/other"}))
                .await
                .is_err()
        );
        assert!(update_model(
            &state,
            1,
            json!({"id":id,"request_path":"https://other.test/chat"})
        )
        .await
        .is_err());
        update_model(&state, 1, json!({"id":id,"request_path":""}))
            .await
            .unwrap();
        assert_eq!(
            resolve_model(&state, 1, "renamed-path")
                .await
                .unwrap()
                .request_path,
            ""
        );
    }
    #[tokio::test]
    async fn model_api_config_crud_preserves_and_validates() {
        let state = state().await;
        let pid=create_provider(&state,1,json!({"name":"api fixture","provider_type":"openai","base_url":"https://example.test/v1"})).await.unwrap()["id"].as_i64().unwrap();
        let config = json!({"api_type":"images_generations","auth_type":"x_api_key","response_mode":"task","task_path":"/tasks/{task_id}"});
        let model=create_model(&state,1,json!({"provider_id":pid,"alias":"images","upstream_model":"image-model","request_path":"/images","api_config":config})).await.unwrap();
        let id = model["id"].as_i64().unwrap();
        assert_eq!(model["api_config"], config);
        assert_eq!(
            list_models(&state, 1).await.unwrap()["list"][0]["api_config"],
            config
        );
        update_model(&state, 1, json!({"id":id,"alias":"images-renamed"}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(
                resolve_model(&state, 1, "images-renamed")
                    .await
                    .unwrap()
                    .api_config
            )
            .unwrap(),
            config
        );
        assert!(update_model(&state, 2, json!({"id":id,"api_config":{}}))
            .await
            .is_err());
        assert!(update_model(&state, 1, json!({"id":id,"request_path":""}))
            .await
            .is_err());
        assert!(update_model(&state,1,json!({"id":id,"api_config":{"response_mode":"task","task_path":"https://other.test/{task_id}"}})).await.is_err());
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn direct_fallback_exhaustion_preserves_last_failure_without_leaking_keys() {
        use axum::{body::to_bytes, http::{HeaderMap, Method, StatusCode}, routing::post, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        state.client = Client::builder().no_proxy().build().unwrap();
        state.sub2_redis = Some(redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap().get_connection_manager().await.unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/v1/chat/completions", post(move |headers: HeaderMap| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let secret = headers["authorization"].to_str().unwrap().strip_prefix("Bearer ").unwrap();
                (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "23")], Json(json!({"error":{"message":format!("Service Unavailable {secret}")}})))
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pid = create_provider(&state, 1, json!({"name":"fallback fixture","provider_type":"custom","base_url":format!("http://{address}"),"api_key":"fixture-secret-one"})).await.unwrap()["id"].as_i64().unwrap();
        crate::direct_provider::create(&state, pid, &json!({"name":"second","api_key":"fixture-secret-two"})).await.unwrap();
        let config = crate::model_api::ApiConfig { api_type: crate::model_api::ApiType::ChatCompletions, ..Default::default() };
        let headers = HeaderMap::new();
        let payload = json!({"model":"fixture-model"});
        let response = crate::direct_provider::forward_route(&state, crate::direct_provider::ForwardRequest {
            provider: pid, method: Method::POST, path: "/v1/chat/completions", headers: &headers, payload: &payload,
            route: crate::direct_provider::ForwardRoute { request_path: "", config: &config, task_owner: None }, upload: None,
        }).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()["retry-after"], "23");
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("Service Unavailable"));
        assert!(!text.contains("fixture-secret"));
        let cooling: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_keys WHERE provider_id=? AND cooldown_until>?").bind(pid).bind(crate::store::now()).fetch_one(&state.pool).await.unwrap();
        assert_eq!(cooling, 2);
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn context_marker_saved_route_forwards_and_preserves_client_headers() {
        use axum::{
            body::to_bytes,
            http::{HeaderMap, HeaderValue, Method},
            routing::post,
            Router,
        };
        let mut state = state().await;
        state.client = Client::builder().no_proxy().build().unwrap();
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/v1/messages",
                    post(|headers: HeaderMap| async move {
                        assert_eq!(headers["x-api-key"], "fixture-secret");
                        assert_eq!(headers.get_all("anthropic-beta").iter().count(), 1);
                        assert_eq!(
                            headers["anthropic-beta"],
                            "interleaved-thinking-2025-05-14,context-1m-2025-08-07"
                        );
                        Json(json!({"type":"message","content":[{"type":"text","text":"OK"}]}))
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let pid = create_provider(&state, 1, json!({"name":"context fixture","provider_type":"custom","base_url":format!("http://{address}"),"api_key":"fixture-secret"})).await.unwrap()["id"].as_i64().unwrap();
        let model = create_model(&state, 1, json!({"provider_id":pid,"alias":"context-fixture","upstream_model":"fixture-model","api_config":{"api_type":"messages","auth_type":"x_api_key","context_1m":true}})).await.unwrap();
        assert_eq!(model["api_config"]["context_1m"], true);
        update_model(&state, 1, json!({"id":model["id"],"alias":"context-saved"}))
            .await
            .unwrap();
        let route = resolve_model(&state, 1, "context-saved").await.unwrap();
        assert!(route.api_config.context_1m);
        for client_beta in [
            "interleaved-thinking-2025-05-14",
            "interleaved-thinking-2025-05-14,context-1m-2025-08-07",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_str(client_beta).unwrap(),
            );
            let payload = json!({"model":"fixture-model"});
            let response = crate::direct_provider::forward_route(
                &state,
                crate::direct_provider::ForwardRequest {
                    provider: pid,
                    method: Method::POST,
                    path: "/v1/messages",
                    headers: &headers,
                    payload: &payload,
                    route: crate::direct_provider::ForwardRoute {
                        request_path: "",
                        config: &route.api_config,
                        task_owner: None,
                    },
                    upload: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(response.status(), 200);
            assert!(!to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty());
        }
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn model_api_gateway_tasks_and_native_messages() {
        use axum::{
            body::{to_bytes, Body},
            extract::Path,
            http::HeaderMap,
            routing::{get, post},
            Json, Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        let redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        state.client = Client::builder().no_proxy().build().unwrap();
        let submitted = Arc::new(AtomicUsize::new(0));
        let queried = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(AtomicUsize::new(0));
        let submit_count = submitted.clone();
        let query_count = queried.clone();
        let failure_count = failures.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=Router::new()
            .route("/native/image/submit",post(move |headers:HeaderMap,Json(body):Json<Value>| {let count=submit_count.clone();async move {
                count.fetch_add(1,Ordering::SeqCst);
                assert_eq!(headers["x-api-key"],"original-secret");assert!(headers.get("authorization").is_none());assert!(headers.get("cookie").is_none());
                assert_eq!(body["model"],"real-image-model");assert_eq!(body["size"],"2K");
                Json(json!({"task_id":"upstream_img_123","status":"queued"}))
            }}))
            .route("/native/image/tasks/{id}",get(move |Path(id):Path<String>,headers:HeaderMap| {let count=query_count.clone();async move {
                count.fetch_add(1,Ordering::SeqCst);assert_eq!(id,"upstream_img_123");assert_eq!(headers["x-api-key"],"original-secret");assert!(headers.get("authorization").is_none());
                Json(json!({"task_id":id,"status":"succeeded","model":"internal-vendor-model","result":{"images":[{"url":"https://example.test/image.png"}],"usage":{"input_tokens":12,"output_tokens":100}}}))
            }}))
            .route("/native/messages",post(|headers:HeaderMap,Json(body):Json<Value>| async move {
                assert_eq!(headers["x-api-key"],"rotated-secret");assert_eq!(headers["anthropic-version"],"2023-06-01");
                assert_eq!(body["system"],"native system");assert_eq!(body["max_tokens"],32);assert_eq!(body["model"],"claude-upstream");
                Json(json!({"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"native-ok"}],"usage":{"input_tokens":3,"output_tokens":4}}))
            }))
            .route("/native/failure",post(move || {let count=failure_count.clone();async move {count.fetch_add(1,Ordering::SeqCst);(axum::http::StatusCode::GATEWAY_TIMEOUT,Json(json!({"error":"private-provider-information"})))}}));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pid=create_provider(&state,1,json!({"name":"task fixture","provider_type":"openai","base_url":format!("http://{address}/v1"),"api_key":"original-secret"})).await.unwrap()["id"].as_i64().unwrap();
        let model=create_model(&state,1,json!({"provider_id":pid,"alias":"my-image","upstream_model":"real-image-model","request_path":"/native/image/submit","input_price_per_million":1,"output_price_per_million":2,"api_config":{"api_type":"images_generations","auth_type":"x_api_key","response_mode":"task","task_path":"/native/image/tasks/{task_id}"}})).await.unwrap();
        let mid = model["id"].as_i64().unwrap();
        let key = create_key(&state, 1, json!({"name":"task key"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let headers = || {
            let mut h = HeaderMap::new();
            h.insert("authorization", format!("Bearer {key}").parse().unwrap());
            h
        };
        let request = |path: &str, body: Value| {
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("cookie", "private-browser-cookie")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        assert!(crate::proxy::passthrough(
            State(state.clone()),
            request(
                "/v1/chat/completions",
                json!({"model":"my-image","prompt":"hi"})
            )
        )
        .await
        .is_err());
        assert_eq!(submitted.load(Ordering::SeqCst), 0);
        let response = crate::proxy::passthrough(
            State(state.clone()),
            request(
                "/v1/images/generations",
                json!({"model":"my-image","prompt":"hi","size":"2K"}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 202);
        assert!(!response.headers().contains_key("x-junohub-task-id"));
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        let task_id = body["task_id"].as_str().unwrap().to_owned();
        assert!(task_id.starts_with("task_"));
        assert_ne!(task_id, "upstream_img_123");
        assert_eq!(submitted.load(Ordering::SeqCst), 1);
        let other = create_key(&state, 1, json!({"name":"other key"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut other_headers = HeaderMap::new();
        other_headers.insert("authorization", format!("Bearer {other}").parse().unwrap());
        assert!(crate::gateway_tasks::query(
            State(state.clone()),
            Path(task_id.clone()),
            other_headers
        )
        .await
        .is_err());
        let foreign = create_key(&state, 2, json!({"name":"foreign key"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut foreign_headers = HeaderMap::new();
        foreign_headers.insert(
            "authorization",
            format!("Bearer {foreign}").parse().unwrap(),
        );
        assert!(crate::gateway_tasks::query(
            State(state.clone()),
            Path(task_id.clone()),
            foreign_headers
        )
        .await
        .is_err());
        assert_eq!(queried.load(Ordering::SeqCst), 0);
        let aid = crate::direct_provider::keys(&state, pid).await.unwrap()[0].id;
        crate::direct_provider::update(
            &state,
            pid,
            &json!({"account_id":aid,"api_key":"rotated-secret"}),
        )
        .await
        .unwrap();
        update_model(
            &state,
            1,
            json!({"id":mid,"input_price_per_million":100,"output_price_per_million":100}),
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let response =
                crate::gateway_tasks::query(State(state.clone()), Path(task_id.clone()), headers())
                    .await
                    .unwrap();
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap())
                    .unwrap();
            assert_eq!(value["task_id"], task_id);
            assert_eq!(value["model"], "my-image");
            assert_eq!(value["status"], "succeeded");
        }
        assert_eq!(queried.load(Ordering::SeqCst), 1);
        let usage=sqlx::query("SELECT total_requests,input_tokens,output_tokens,cost_usd FROM virtual_keys WHERE key_hash=?").bind(store::hash_key(&key)).fetch_one(&state.pool).await.unwrap();
        assert_eq!(usage.get::<i64, _>("total_requests"), 1);
        assert_eq!(usage.get::<i64, _>("input_tokens"), 12);
        assert_eq!(usage.get::<i64, _>("output_tokens"), 100);
        assert!((usage.get::<f64, _>("cost_usd") - 0.000212).abs() < 1e-9);
        create_model(&state,1,json!({"provider_id":pid,"alias":"my-claude","upstream_model":"claude-upstream","request_path":"/native/messages","api_config":{"api_type":"messages","auth_type":"x_api_key"}})).await.unwrap();
        let response=crate::client_protocol::anthropic_messages(State(state.clone()),request("/v1/messages",json!({"model":"my-claude","system":"native system","max_tokens":32,"messages":[{"role":"user","content":"hi"}]}))).await.unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value["content"][0]["text"], "native-ok");
        update_model(
            &state,
            1,
            json!({"id":mid,"request_path":"/native/failure"}),
        )
        .await
        .unwrap();
        let response = crate::proxy::passthrough(
            State(state.clone()),
            request(
                "/v1/images/generations",
                json!({"model":"my-image","prompt":"hi"}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 504);
        assert_eq!(failures.load(Ordering::SeqCst), 1);
        let text = String::from_utf8(
            to_bytes(response.into_body(), 10000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("private-provider-information"));
        sqlx::query("UPDATE gateway_tasks SET expires_at=0 WHERE id=?")
            .bind(&task_id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            crate::gateway_tasks::query(State(state.clone()), Path(task_id), headers())
                .await
                .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn model_route_independent_protocols_convert_request_and_response() {
        use axum::{
            body::{to_bytes, Body},
            http::HeaderMap,
            routing::post,
            Json, Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        let redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        state.client = Client::builder().no_proxy().build().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=Router::new().route("/vendor/chat",post(move |headers:HeaderMap,Json(body):Json<Value>| {let count=count.clone();async move {
            count.fetch_add(1,Ordering::SeqCst);
            assert_eq!(headers["authorization"],"Bearer upstream-secret");assert!(headers.get("x-api-key").is_none());
            assert_eq!(body["model"],"vendor-model");assert_eq!(body["messages"][0]["role"],"system");assert_eq!(body["messages"][0]["content"],"system text");
            assert_eq!(body["messages"][1]["content"],"hello");assert_eq!(body["max_tokens"],32);assert_eq!(body["top_p"],0.9);assert_eq!(body["stream"],false);
            assert_eq!(body["stop"][0],"END");assert_eq!(body["tool_choice"],"auto");assert!(body.get("system").is_none());
            Json(json!({"id":"chat_fixture","choices":[{"message":{"role":"assistant","content":"converted-ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4}}))
        }}));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pid=create_provider(&state,1,json!({"name":"two protocol fixture","provider_type":"openai","base_url":format!("http://{address}/v1"),"api_key":"upstream-secret"})).await.unwrap()["id"].as_i64().unwrap();
        let model=create_model(&state,1,json!({"provider_id":pid,"alias":"claude-entry","upstream_model":"vendor-model","request_path":"/vendor/chat","api_config":{"api_type":"messages","upstream_api_type":"chat_completions"}})).await.unwrap();
        assert_eq!(model["api_config"]["api_type"], "messages");
        assert_eq!(model["api_config"]["upstream_api_type"], "chat_completions");
        update_model(
            &state,
            1,
            json!({"id":model["id"],"output_price_per_million":1}),
        )
        .await
        .unwrap();
        let resolved = resolve_model(&state, 1, "claude-entry").await.unwrap();
        assert_eq!(
            resolved.api_config.api_type.endpoint(),
            Some("/v1/messages")
        );
        assert_eq!(
            resolved.api_config.upstream_path(""),
            "/v1/chat/completions"
        );
        let key = create_key(&state, 1, json!({"name":"caller fixture"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = |body: Value| {
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", &key)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let source = json!({"model":"claude-entry","system":"system text","max_tokens":32,"top_p":0.9,"stop_sequences":["END"],"tool_choice":{"type":"auto"},"messages":[{"role":"user","content":"hello"}]});
        let response = crate::client_protocol::anthropic_messages(
            State(state.clone()),
            request(source.clone()),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value["type"], "message");
        assert_eq!(value["content"][0]["text"], "converted-ok");
        assert_eq!(value["model"], "claude-entry");
        assert_eq!(value["usage"]["input_tokens"], 3);
        let mut streaming = source.clone();
        streaming["stream"] = json!(true);
        let response =
            crate::client_protocol::anthropic_messages(State(state.clone()), request(streaming))
                .await
                .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let text = String::from_utf8(
            to_bytes(response.into_body(), 10000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(text.contains("event: message_start"));
        assert!(text.contains("converted-ok"));
        let mut unsupported = source;
        unsupported["thinking"] = json!({"type":"enabled","budget_tokens":1000});
        assert!(crate::client_protocol::anthropic_messages(
            State(state.clone()),
            request(unsupported)
        )
        .await
        .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let logged:i64=sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs WHERE model_alias='claude-entry' AND status_code=200").fetch_one(&state.pool).await.unwrap();
        assert_eq!(logged, 2);
        server.abort();
    }

    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use reqwest::Client;
    use serde_json::json;
    use sqlx::{postgres::PgPoolOptions, sqlite::SqlitePoolOptions};

    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn managed_provider_address_is_a_marker_not_deployment_topology() {
        let state = state().await;
        // 客户端以前会传部署拓扑地址（容器名 + 端口），现在无论传什么都不采用。
        let created = create_provider(
            &state,
            1,
            json!({
                "name": "managed fixture",
                "provider_type": "sub2api",
                "base_url": "http://junohub-server:9003/v1",
                "api_key": "private-managed-key",
            }),
        )
        .await
        .unwrap();
        let stored: String = sqlx::query_scalar("SELECT base_url FROM user_providers WHERE id=?")
            .bind(created["id"].as_i64().unwrap())
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(stored, crate::store::MANAGED_PROVIDER_BASE_URL);
        assert!(!stored.contains("junohub-server"));
        // 标记值不是 http(s) 地址，不会被误当成真实上游端点。
        assert!(validate_base_url(&stored).is_err());

        // 非托管供应商仍然要求并保存客户端提供的真实地址。
        let direct = create_provider(
            &state,
            1,
            json!({"name":"direct fixture","provider_type":"openai","base_url":"https://supplier.test/v1"}),
        )
        .await
        .unwrap();
        let direct_base: String = sqlx::query_scalar("SELECT base_url FROM user_providers WHERE id=?")
            .bind(direct["id"].as_i64().unwrap())
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(direct_base, "https://supplier.test/v1");

        // 真正挂进账户池的托管供应商，其地址不允许被改回一个真实地址。
        sqlx::query("INSERT INTO sub2_managed_pools(provider_id,user_id,platform,group_id,api_key_id,created_at,updated_at) VALUES(?,1,'openai',1,1,0,0)")
            .bind(created["id"].as_i64().unwrap())
            .execute(&state.pool)
            .await
            .unwrap();
        let rejected = update_provider(
            &state,
            1,
            json!({"id": created["id"], "base_url": "https://elsewhere.test/v1"}),
        )
        .await;
        assert!(matches!(rejected, Err(crate::error::AppError::BadRequest(_))));
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn tokenhub_images_gateway_alias_auth_usage() {
        use axum::{
            body::{to_bytes, Body},
            http::HeaderMap,
            routing::{get, post},
            Json, Router,
        };
        let mut state = state().await;
        let redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap()).unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        // 图片适配的作用范围来自配置，代码里没有内置的供应商地址。
        Arc::make_mut(&mut state.config).tokenhub_image_hosts =
            vec!["images.example.test".into()];
        Arc::make_mut(&mut state.config).tokenhub_image_models =
            vec!["gpt-image-2".into(), "gpt-image-2.5-sunburst".into(), "gpt-image-2.5-flare".into()];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app=Router::new().route("/v2/extend/image/async/gpt_image/generations",post(|headers:HeaderMap,Json(body):Json<Value>|async move {
            assert_eq!(headers["x-api-key"],"private-tokenhub-key");
            assert!(headers.get("authorization").is_none());
            assert_eq!(body["model"],"gpt-image-2.5-flare");
            assert_eq!(body["prompt"],"A blue circle");
            assert!(body.get("response_format").is_none());
            Json(json!({"task_id":"task-test","status":"queued"}))
        })).route("/v2/extend/image/gpt_image/tasks/task-test",get(|headers:HeaderMap|async move {
            assert_eq!(headers["x-api-key"],"private-tokenhub-key");
            Json(json!({"status":"succeeded","result":{"images":[{"url":"https://example.test/generated.png"}],"usage":{"input_tokens":12,"output_tokens":100}}}))
        }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state.client = Client::builder()
            .no_proxy()
            .resolve("images.example.test", addr)
            .build()
            .unwrap();
        let provider=create_provider(&state,1,json!({"name":"TokenHub fixture","provider_type":"openai","base_url":format!("http://images.example.test:{}/v1",addr.port()),"api_key":"private-tokenhub-key"})).await.unwrap()["id"].as_i64().unwrap();
        create_model(&state,1,json!({"provider_id":provider,"alias":"my-image","upstream_model":"gpt-image-2.5-flare"})).await.unwrap();
        let key = create_key(
            &state,
            1,
            json!({"name":"image-client","allowed_models":["my-image"]}),
        )
        .await
        .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let req = |model: &str, format: &str| {
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/images/generations")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::from(
                    json!({"model":model,"prompt":"A blue circle","response_format":format})
                        .to_string(),
                ))
                .unwrap()
        };
        assert!(matches!(
            crate::proxy::passthrough(State(state.clone()), req("forbidden", "url")).await,
            Err(AppError::Unauthorized)
        ));
        assert!(matches!(
            crate::proxy::passthrough(State(state.clone()), req("my-image", "b64_json")).await,
            Err(AppError::BadRequest(_))
        ));
        let response = crate::proxy::passthrough(State(state.clone()), req("my-image", "url"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["data"][0]["url"], "https://example.test/generated.png");
        assert_eq!(body["usage"]["output_tokens"], 100);
        assert!(!String::from_utf8_lossy(&bytes).contains("private-tokenhub-key"));
        let logged=sqlx::query("SELECT model_alias,input_tokens,output_tokens FROM user_request_logs WHERE status_code=200 ORDER BY id DESC LIMIT 1").fetch_one(&state.pool).await.unwrap();
        assert_eq!(logged.get::<String, _>("model_alias"), "my-image");
        assert_eq!(logged.get::<i64, _>("input_tokens"), 12);
        assert_eq!(logged.get::<i64, _>("output_tokens"), 100);
        task.abort();
    }

    #[tokio::test]
    async fn admin_detail_scopes_usage_and_excludes_secrets() {
        let state = state().await;
        let key = create_key(&state, 1, json!({"name":"owner-key"}))
            .await
            .unwrap();
        create_key(&state, 2, json!({"name":"other-key"}))
            .await
            .unwrap();
        sqlx::query("UPDATE virtual_keys SET total_requests=3,input_tokens=42,cost_usd=1.25 WHERE user_id=1").execute(&state.pool).await.unwrap();
        // User totals come from durable request history, including deleted keys.
        // Keep both sides of this fixture consistent with real request recording.
        for (input, cost) in [(42, 1.25), (0, 0.0), (0, 0.0)] {
            sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,input_tokens,cost_usd,created_at) VALUES(1,?,'test',?,?,?)")
                .bind(key["id"].as_i64().unwrap()).bind(input).bind(cost).bind(store::now()).execute(&state.pool).await.unwrap();
        }
        let detail = admin_user_detail(&state, &json!({"id":1})).await.unwrap();
        assert_eq!(detail["usage"]["requests"], 3);
        assert_eq!(detail["usage"]["cost_usd"], 1.25);
        assert_eq!(detail["keys"].as_array().unwrap().len(), 1);
        assert_eq!(detail["keys"][0]["name"], "owner-key");
        assert!(!detail.to_string().contains(key["key"].as_str().unwrap()));
        assert!(!detail.to_string().contains("encrypted_key"));
        assert!(!detail.to_string().contains("other-key"));
        assert!(matches!(
            admin_user_detail(&state, &json!({"id":999})).await,
            Err(AppError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn route_test_draft_is_owned_and_does_not_save_route() {
        use axum::{routing::post, Json, Router};
        let state = state().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/custom/draft",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer secret-fixture");
                assert_eq!(body["model"], "draft-model");
                Json(json!({"choices":[{"message":{"content":"OK"}}]}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = create_provider(&state, 1, json!({"name":"draft test","provider_type":"openai","base_url":base,"api_key":"secret-fixture"})).await.unwrap()["id"].as_i64().unwrap();
        let draft = json!({"provider_id":provider,"upstream_model":"draft-model","request_path":"/custom/draft","api_config":{"api_type":"chat_completions","upstream_api_type":"chat_completions"}});
        assert!(matches!(
            test_model_draft(&state, 2, draft.clone()).await,
            Err(AppError::NotFound(_))
        ));
        let preview = preview_model_draft(&state, 1, draft.clone()).await.unwrap();
        assert_eq!(
            preview["upstream_url"],
            format!("{}/custom/draft", base.trim_end_matches("/v1"))
        );
        assert_eq!(preview["available_accounts"], 1);
        assert!(preview.get("api_key").is_none());
        assert!(matches!(
            preview_model_draft(&state, 2, draft.clone()).await,
            Err(AppError::NotFound(_))
        ));
        let report = test_model_draft(&state, 1, draft.clone()).await.unwrap();
        assert_eq!(report["ok"], true);
        assert!(!report.to_string().contains("secret-fixture"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_models")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        sqlx::query("UPDATE provider_keys SET assigned=0 WHERE provider_id=?")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(test_model_draft(&state, 1, draft.clone()).await.is_err());
        sqlx::query("UPDATE provider_keys SET assigned=1,allowed_models='[\"another-model\"]' WHERE provider_id=?").bind(provider).execute(&state.pool).await.unwrap();
        assert!(test_model_draft(&state, 1, draft.clone()).await.is_err());
        assert_eq!(
            preview_model_draft(&state, 1, draft.clone()).await.unwrap()["available_accounts"],
            0
        );
        sqlx::query("UPDATE provider_keys SET allowed_models='[\"*\"]' WHERE provider_id=?")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            test_model_draft(&state, 1, draft.clone()).await.unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        sqlx::query("UPDATE provider_keys SET cooldown_until=? WHERE provider_id=?")
            .bind(store::now() + 60)
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(test_model_draft(&state, 1, draft).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn member_catalog_uses_shared_routes_without_exposing_upstream_details() {
        let state = state().await;
        let provider = create_provider(&state, 1, json!({"name":"private supplier","provider_type":"openai","base_url":"https://private.example/v1","api_key":"private-secret"})).await.unwrap()["id"].as_i64().unwrap();
        create_model(&state, 1, json!({"provider_id":provider,"alias":"shared-chat","upstream_model":"private-upstream","request_path":"/private/chat","api_config":{"api_type":"chat_completions","upstream_api_type":"chat_completions"},"input_price_per_million":2,"output_price_per_million":4})).await.unwrap();
        assert!(gateway_models(&state, 2).await.unwrap().is_empty());
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let catalog = available_models(&state, 2).await.unwrap();
        assert_eq!(catalog["total"], 1);
        assert_eq!(catalog["list"][0]["icon"]["provider_type"], "openai");
        assert_eq!(catalog["list"][0]["alias"], "shared-chat");
        assert_eq!(catalog["list"][0]["endpoint"], "/v1/chat/completions");
        assert!(!catalog.to_string().contains("private"));
        let avatar = "data:image/png;base64,aW1hZ2U=";
        sqlx::query("UPDATE user_providers SET avatar_url=? WHERE id=?")
            .bind(avatar)
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        let with_avatar = available_models(&state, 2).await.unwrap();
        assert_eq!(with_avatar["list"][0]["icon"]["avatar_url"], avatar);
        assert!(with_avatar["list"][0].get("provider_id").is_none());
        assert!(with_avatar["list"][0].get("request_path").is_none());
        let route = resolve_model(&state, 2, "shared-chat").await.unwrap();
        assert_eq!(route.owner_id, 1);
        assert_eq!(route.request_path, "/private/chat");
        assert!(load_provider(&state, route.owner_id, route.provider_id)
            .await
            .is_ok());
        assert!(load_provider(&state, 2, route.provider_id).await.is_err());
        let created = create_key(&state, 2, json!({"name":"member key"}))
            .await
            .unwrap();
        let key = gateway_key(&state, created["key"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", created["key"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        let gateway_list = crate::proxy::models(State(state.clone()), headers.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(gateway_list["data"][0]["id"], catalog["list"][0]["alias"]);
        record_request(&state, &key, &route, "openai", 200, 50, "", 1000, 500)
            .await
            .unwrap();
        // An attacker-supplied user_id never expands a member's log scope.
        let logs = list_logs(&state, 2, &json!({"user_id":1})).await.unwrap();
        assert_eq!(logs["total"], 1);
        assert_eq!(logs["list"][0]["owner_id"], 2);
        assert_eq!(logs["list"][0]["key_name"], "member key");
        assert_eq!(logs["list"][0]["provider_name"], "private supplier");
        assert_eq!(logs["list"][0]["upstream_model"], "private-upstream");
        assert!(logs["list"][0].get("provider_id").is_none());
        let stats = stats(&state, 2, &json!({})).await.unwrap();
        assert_eq!(stats["total_requests"], 1);
        assert_eq!(stats["total_tokens"], 1500);
        assert!((stats["cost_usd"].as_f64().unwrap() - 0.004).abs() < 1e-9);
        assert!(stats.get("providers").is_none());
        assert!(list_own_keys(&state, 1).await.unwrap()["list"]
            .as_array()
            .unwrap()
            .is_empty());
        // Shared task polling still belongs to the caller and its original key.
        let account = crate::direct_provider::keys(&state, provider)
            .await
            .unwrap()[0]
            .id;
        sqlx::query("INSERT INTO gateway_tasks(id,user_id,virtual_key_id,model_id,model_alias,provider_id,provider_key_id,base_url,encrypted_api_key,auth_type,task_path,upstream_task_id,result_json,settled,created_at,expires_at) VALUES('task_member',2,?,?,'shared-chat',?,?,'https://private.example','','\"bearer\"','/tasks/{task_id}','vendor-task',?,1,?,?)")
            .bind(key.id).bind(route.id).bind(provider).bind(account).bind(json!({"task_id":"task_member","status":"succeeded","result":{"ok":true}}).to_string()).bind(store::now()).bind(store::now()+3600).execute(&state.pool).await.unwrap();
        let response =
            crate::gateway_tasks::query(State(state.clone()), Path("task_member".into()), headers)
                .await
                .unwrap();
        assert_eq!(response.status(), 200);
        let other_key = create_key(&state, 1, json!({"name":"other caller"}))
            .await
            .unwrap();
        let mut other_headers = HeaderMap::new();
        other_headers.insert(
            "authorization",
            format!("Bearer {}", other_key["key"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            crate::gateway_tasks::query(
                State(state.clone()),
                Path("task_member".into()),
                other_headers
            )
            .await,
            Err(AppError::NotFound(_))
        ));
        sqlx::query("UPDATE user_providers SET enabled=0 WHERE id=?")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(gateway_models(&state, 2).await.unwrap().is_empty());
        assert!(resolve_model(&state, 2, "shared-chat").await.is_err());
        sqlx::query("UPDATE user_providers SET enabled=1 WHERE id=?")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(gateway_models(&state, 2).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn password_and_identity_changes_revoke_existing_sessions_on_all_protected_routes() {
        use axum::{middleware, routing::post, Router};
        let state = state().await;
        let hash = secrets::hash_password("fixture-old-password").unwrap();
        sqlx::query("UPDATE users SET role='admin',password_hash=? WHERE id=1")
            .bind(hash)
            .execute(&state.pool)
            .await
            .unwrap();
        let old =
            crate::auth::issue_user_token(&state, 1, "one@example.com".into(), "admin".into())
                .unwrap();
        let other =
            crate::auth::issue_user_token(&state, 2, "two@example.com".into(), "user".into())
                .unwrap();
        let app = Router::new()
            .route(
                "/account",
                post(|| async { "ok" }).route_layer(middleware::from_fn_with_state(
                    state.clone(),
                    crate::auth::require_account,
                )),
            )
            .route(
                "/console",
                post(|| async { "ok" }).route_layer(middleware::from_fn_with_state(
                    state.clone(),
                    crate::auth::require_user,
                )),
            )
            .route(
                "/admin",
                post(|| async { "ok" }).route_layer(middleware::from_fn_with_state(
                    state.clone(),
                    crate::auth::require_admin,
                )),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::builder().no_proxy().build().unwrap();
        for path in ["account", "console", "admin"] {
            assert_eq!(
                client
                    .post(format!("{base}/{path}"))
                    .bearer_auth(&old)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
        update_account_password(&state,1,&json!({"current_password":"fixture-old-password","new_password":"fixture-new-password"})).await.unwrap();
        for path in ["account", "console", "admin"] {
            assert_eq!(
                client
                    .post(format!("{base}/{path}"))
                    .bearer_auth(&old)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
        }
        let (_, Json(response)) = login(
            State(state.clone()),
            Json(Credentials {
                email: "one@example.com".into(),
                password: "fixture-new-password".into(),
            }),
        )
        .await
        .unwrap();
        let fresh = response.data["token"].as_str().unwrap().to_owned();
        assert_eq!(
            client
                .post(format!("{base}/admin"))
                .bearer_auth(fresh)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        admin_update_user(&state, &json!({"id":2,"role":"user","status":0}))
            .await
            .unwrap();
        admin_update_user(&state, &json!({"id":2,"role":"user","status":1}))
            .await
            .unwrap();
        assert_eq!(
            client
                .post(format!("{base}/account"))
                .bearer_auth(&other)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let version: i64 = sqlx::query_scalar("SELECT session_version FROM users WHERE id=2")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(version, 2);
        server.abort();
    }

    #[tokio::test]
    async fn bootstrap_does_not_reactivate_or_promote_existing_accounts_and_last_admin_is_preserved(
    ) {
        let mut state = state().await;
        let config = Arc::make_mut(&mut state.config);
        config.bootstrap_admin_email = "one@example.com".into();
        config.bootstrap_admin_password = "fixture-bootstrap-password".into();
        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        ensure_bootstrap_admin(&state).await.unwrap();
        let record: (String, i64) = sqlx::query_as("SELECT role,status FROM users WHERE id=1")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(record, ("user".into(), 0));
        sqlx::query("UPDATE users SET status=1,role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(admin_update_user(&state, &json!({"id":1,"role":"user"}))
            .await
            .is_err());
        assert!(admin_update_user(&state, &json!({"id":1,"status":0}))
            .await
            .is_err());
        admin_update_user(&state, &json!({"id":2,"role":"admin"}))
            .await
            .unwrap();
        admin_update_user(&state, &json!({"id":1,"status":0}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn member_api_permissions_use_current_database_role() {
        use axum::{middleware, routing::post, Router};
        let state = state().await;
        let token =
            crate::auth::issue_user_token(&state, 2, "two@example.com".into(), "admin".into())
                .unwrap();
        let app = Router::new()
            .nest(
                "/console",
                Router::new().route("/{*path}", post(dispatch)).route_layer(
                    middleware::from_fn_with_state(state.clone(), crate::auth::require_user),
                ),
            )
            .nest(
                "/admin",
                Router::new()
                    .route("/{*path}", post(crate::admin::dispatch))
                    .route_layer(middleware::from_fn_with_state(
                        state.clone(),
                        crate::auth::require_admin,
                    )),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for path in [
            "provider/list",
            "provider/create",
            "model/list",
            "model/create",
            "model/test",
            "model/limits/update",
            "sub2api/oauth/openai/start",
        ] {
            let response = state
                .client
                .post(format!("{base}/console/{path}"))
                .bearer_auth(&token)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{path}");
        }
        let save = state
            .client
            .post(format!("{base}/admin/guide/update"))
            .bearer_auth(&token)
            .json(&json!({"content":"forbidden","revision":0}))
            .send()
            .await
            .unwrap();
        assert_eq!(save.status(), 403);
        for path in [
            "model/available",
            "guide/get",
            "key/own-list",
            "logs",
            "stats",
        ] {
            let response = state
                .client
                .post(format!("{base}/console/{path}"))
                .bearer_auth(&token)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
        }
        // Promotion takes effect with the existing token, even if its old claim differs.
        sqlx::query("UPDATE users SET role='admin' WHERE id=2")
            .execute(&state.pool)
            .await
            .unwrap();
        let response = state
            .client
            .post(format!("{base}/console/provider/list"))
            .bearer_auth(&token)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        server.abort();
    }

    #[tokio::test]
    async fn member_stats_and_log_filters_never_include_other_users() {
        let state = state().await;
        for (uid, tokens, cost) in [(1, 9000, 9.0), (2, 25, 0.25)] {
            sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,input_tokens,total_tokens,cost_usd,created_at) VALUES(?,0,'test',?,?,?,?)")
                .bind(uid).bind(tokens).bind(tokens).bind(cost).bind(store::now()).execute(&state.pool).await.unwrap();
        }
        let own = stats(&state, 2, &json!({"user_id":1})).await.unwrap();
        assert_eq!(own["requests"], 1);
        assert_eq!(own["total_tokens"], 25);
        assert_eq!(own["cost_usd"], 0.25);
        let logs = list_logs(&state, 2, &json!({"user_id":1,"page_size":100}))
            .await
            .unwrap();
        assert_eq!(logs["total"], 1);
        assert_eq!(logs["list"][0]["owner_id"], 2);
        assert_eq!(logs["filters"]["users"], json!([]));
        assert_eq!(logs["filters"]["providers"], json!([]));
        let own_search = list_logs(&state, 2, &json!({"user_search":"two@example.com"}))
            .await
            .unwrap();
        assert_eq!(own_search["total"], 1);
        let other_search = list_logs(&state, 2, &json!({"user_search":"one@example.com"}))
            .await
            .unwrap();
        assert_eq!(other_search["total"], 0);
    }

    #[tokio::test]
    async fn login_failures_report_the_actual_cause_and_field() {
        use axum::response::IntoResponse;
        let state = state().await;
        let hash = secrets::hash_password("fixture-password").unwrap();
        sqlx::query("UPDATE users SET password_hash=? WHERE id=1")
            .bind(hash)
            .execute(&state.pool)
            .await
            .unwrap();
        for (email, password, reason, field, status) in [
            (
                "missing@example.com",
                "fixture-password",
                "email_not_registered",
                Some("email"),
                401,
            ),
            (
                "one@example.com",
                "wrong-password",
                "password_incorrect",
                Some("password"),
                401,
            ),
        ] {
            let failure = login(
                State(state.clone()),
                Json(Credentials {
                    email: email.into(),
                    password: password.into(),
                }),
            )
            .await
            .unwrap_err()
            .into_response();
            assert_eq!(failure.status(), status);
            let body = axum::body::to_bytes(failure.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["reason"], reason);
            assert_eq!(body["field"].as_str(), field);
            assert!(body["message"]
                .as_str()
                .unwrap()
                .contains(if field == Some("email") {
                    "尚未注册"
                } else {
                    "密码不正确"
                }));
        }
        let credentials = || {
            Json(Credentials {
                email: "one@example.com".into(),
                password: "fixture-password".into(),
            })
        };
        let success = login(State(state.clone()), credentials()).await.unwrap();
        assert_eq!(success.1 .0.data["uid"], 1);
        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let failure = login(State(state.clone()), credentials())
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(failure.status(), 403);
        let body = axum::body::to_bytes(failure.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["reason"], "account_disabled");
        assert_eq!(body["message"], "账户已停用，请联系管理员。");
        assert!(body.get("field").is_none());
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn registration_requires_verification_and_creates_only_one_user() {
        use axum::response::IntoResponse;
        let state = state().await;
        let input = |code: &str| {
            Json(RegistrationInput {
                email: " VERIFIED@Example.Test ".into(),
                password: "fixture-password".into(),
                display_name: "测试账户".into(),
                verification_code: code.into(),
            })
        };
        let missing = register(State(state.clone()), input(""))
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(missing.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(missing.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["field"],
            "verification_code"
        );
        crate::email_verification::seed_test_code(
            &state.pool,
            &state.config.encryption_key,
            "verified@example.test",
            "123456",
            store::now(),
        )
        .await;
        assert!(matches!(
            register(State(state.clone()), input("654321")).await,
            Err(AppError::FormFailed {
                reason: "code_incorrect",
                ..
            })
        ));
        let attempts: i64 = sqlx::query_scalar(
            "SELECT attempts FROM registration_codes WHERE email='verified@example.test'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        let (first, second) = tokio::join!(
            register(State(state.clone()), input("123456")),
            register(State(state.clone()), input("123456"))
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let (_, Json(result)) = first.or(second).unwrap();
        assert_eq!(result.data["role"], "user");
        assert_eq!(result.data["email"], "verified@example.test");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email='verified@example.test'")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert!(matches!(
            register(State(state.clone()), input("123456")).await,
            Err(AppError::FormFailed {
                reason: "code_missing",
                ..
            })
        ));
        assert!(login(
            State(state.clone()),
            Json(Credentials {
                email: "verified@example.test".into(),
                password: "fixture-password".into()
            })
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn registration_code_reports_missing_configuration_and_existing_email() {
        let state = state().await;
        let existing = crate::email_verification::send_code(
            State(state.clone()),
            Json(crate::email_verification::SendCodeInput {
                email: "one@example.com".into(),
            }),
        )
        .await;
        assert!(matches!(
            existing,
            Err(AppError::FormFailed {
                reason: "email_registered",
                field: "email",
                ..
            })
        ));
        let missing = crate::email_verification::send_code(
            State(state.clone()),
            Json(crate::email_verification::SendCodeInput {
                email: "new@example.test".into(),
            }),
        )
        .await;
        assert!(matches!(missing, Err(AppError::ServiceUnavailable(_))));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_codes")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn smtp_registration_failure_never_enables_a_code_or_falls_back_to_resend() {
        let mut state = state().await;
        let config = Arc::make_mut(&mut state.config);
        config.email.provider = crate::config::EmailProvider::Smtp;
        config.email.api_key = "not-a-real-resend-key".into();
        let input = || {
            Json(crate::email_verification::SendCodeInput {
                email: "member@example.test".into(),
            })
        };
        let missing = crate::email_verification::send_code(State(state.clone()), input())
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("SMTP_HOST"));
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_email_sends")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(attempts, 0);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        Arc::make_mut(&mut state.config).email.smtp = crate::config::SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            username: "fixture@example.test".into(),
            password: "fixture-secret".into(),
            from: "JunoHub <fixture@example.test>".into(),
            ..Default::default()
        };
        let failed = crate::email_verification::send_code(State(state.clone()), input())
            .await
            .unwrap_err();
        assert!(failed.to_string().contains("SMTP 连接失败"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_codes")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_email_sends")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(attempts, 1);
        let retry = crate::email_verification::send_code(State(state.clone()), input())
            .await
            .unwrap_err();
        assert!(matches!(retry, AppError::TooManyRequests(_)));
    }

    #[tokio::test]
    async fn created_provider_discovers_models_using_saved_credentials_and_protocol() {
        use axum::{http::HeaderMap, routing::get, Json, Router};
        let app = Router::new()
            .route(
                "/compat/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["authorization"], "Bearer discovery-fixture");
                    Json(json!({"data":[{"id":"model-b"},{"id":"model-a"},{"id":"model-a"}]}))
                }),
            )
            .route(
                "/anthropic/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-api-key"], "discovery-fixture");
                    assert_eq!(headers["anthropic-version"], "2023-06-01");
                    assert!(headers.get("authorization").is_none());
                    Json(json!({"data":[{"id":"claude-fixture"}]}))
                }),
            )
            .route(
                "/gemini/v1beta/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-goog-api-key"], "discovery-fixture");
                    assert!(headers.get("authorization").is_none());
                    Json(json!({"models":[{"name":"models/gemini-fixture"}]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = state().await;
        for (kind, path, expected) in [
            (
                "openai-compatible",
                "/compat/v1",
                json!(["model-a", "model-b"]),
            ),
            ("anthropic", "/anthropic", json!(["claude-fixture"])),
            ("gemini", "/gemini", json!(["models/gemini-fixture"])),
        ] {
            let created = create_provider(&state, 1, json!({"name":kind,"provider_type":kind,"base_url":format!("{base}{path}"),"api_key":"discovery-fixture","request_limits":{"max_concurrency":0,"rpm":0}})).await.unwrap();
            let id = created["id"].as_i64().unwrap();
            let catalog = provider_models(&state, 1, id).await.unwrap();
            assert_eq!(catalog["models"], expected);
            assert!(matches!(
                provider_models(&state, 2, id).await,
                Err(AppError::NotFound(_))
            ));
            assert!(!catalog.to_string().contains("discovery-fixture"));
        }
        server.abort();
    }

    pub(crate) async fn state() -> AppState {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect test database");
        sqlx::migrate!().run(&pool).await.expect("run migrations");
        let timestamp = store::now();
        for email in ["one@example.com", "two@example.com"] {
            sqlx::query("INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) VALUES(?,?,'test','user',1,?,?)")
                .bind(email)
                .bind("unused")
                .bind(timestamp)
                .bind(timestamp)
                .execute(&pool)
                .await
                .expect("insert test user");
        }
        AppState {
            pool,
            sub2_pool: PgPoolOptions::new()
                .connect_lazy("postgres://sub2api:sub2api@127.0.0.1/sub2api")
                .unwrap(),
            sub2_redis: None,
            oauth_sessions: crate::oauth::OAuthSessionStore::default(),
            config: Arc::new(Config {
                bind: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                database_url: "sqlite::memory:".into(),
                jwt_secret: "test-jwt-secret".into(),
                bootstrap_admin_email: String::new(),
                bootstrap_admin_password: String::new(),
                encryption_key: "test-encryption-key".into(),
                email: Default::default(),
                oss: Default::default(),
                sub2api_admin_email: String::new(),
                sub2api_database_url: "postgres://sub2api:sub2api@127.0.0.1/sub2api".into(),
                sub2api_redis_url: "redis://127.0.0.1:6379/0".into(),
                sub2api_slot_ttl_seconds: 1800,
                sub2api_wait_ttl_seconds: 120,
                sub2api_sticky_ttl_seconds: 3600,
                sub2api_response_sticky_ttl_seconds: 3600,
                sub2api_session_hash_read_old_fallback: true,
                sub2api_session_hash_dual_write_old: true,
                scheduler_worker_enabled: false,
                account_engine_gateway_enabled: false,
                allowed_origins: Vec::new(),
                tokenhub_image_hosts: Vec::new(),
                tokenhub_image_models: Vec::new(),
                log_retention_days: 0,
            }),
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn editing_resources_saves_limits_atomically_and_preserves_omitted_limits() {
        let state = state().await;
        let pid=create_provider(&state,1,json!({"name":"limits-provider","provider_type":"openai","base_url":"https://supplier.test/v1","api_key":"fixture","request_limits":{"max_concurrency":5,"rpm":50}})).await.unwrap()["id"].as_i64().unwrap();
        let mid=create_model(&state,1,json!({"provider_id":pid,"alias":"limits-model","upstream_model":"fixture","request_limits":{"max_concurrency":3,"rpm":30}})).await.unwrap()["id"].as_i64().unwrap();
        let kid = create_key(
            &state,
            1,
            json!({"name":"limits-key","request_limits":{"max_concurrency":2,"rpm":20}}),
        )
        .await
        .unwrap()["id"]
            .as_i64()
            .unwrap();
        for (kind, id, concurrency, rpm) in [
            ("provider", pid, 5, 50),
            ("model", mid, 3, 30),
            ("key", kid, 2, 20),
        ] {
            let saved = crate::gateway_limits::read(&state, kind, id).await.unwrap();
            assert_eq!((saved.max_concurrency, saved.rpm), (concurrency, rpm));
        }
        update_provider(&state,1,json!({"id":pid,"name":"renamed-provider","request_limits":{"max_concurrency":4,"rpm":40}})).await.unwrap();
        update_key(
            &state,
            1,
            json!({"id":kid,"name":"renamed-key","request_limits":{"max_concurrency":0,"rpm":0}}),
        )
        .await
        .unwrap();
        update_model(&state, 1, json!({"id":mid,"alias":"renamed-model"}))
            .await
            .unwrap();
        assert_eq!(
            crate::gateway_limits::read(&state, "model", mid)
                .await
                .unwrap()
                .rpm,
            30
        );
        assert_eq!(
            crate::gateway_limits::read(&state, "key", kid)
                .await
                .unwrap()
                .rpm,
            0
        );
        assert_eq!(
            crate::gateway_limits::read(&state, "provider", pid)
                .await
                .unwrap()
                .rpm,
            40
        );
        assert!(update_key(&state,1,json!({"id":kid,"name":"must-not-save","request_limits":{"max_concurrency":-1,"rpm":20}})).await.is_err());
        let name: String = sqlx::query_scalar("SELECT name FROM virtual_keys WHERE id=?")
            .bind(kid)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(name, "renamed-key");
        assert!(create_key(
            &state,
            1,
            json!({"name":"bad-new","request_limits":{"max_concurrency":1,"rpm":1.5}})
        )
        .await
        .is_err());
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM virtual_keys WHERE name='bad-new'")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        sqlx::query("CREATE TRIGGER reject_model_limit BEFORE UPDATE ON model_limits BEGIN SELECT RAISE(ABORT,'fixture limit failure'); END").execute(&state.pool).await.unwrap();
        assert!(update_model(&state,1,json!({"id":mid,"alias":"must-rollback","request_limits":{"max_concurrency":9,"rpm":90}})).await.is_err());
        let alias: String = sqlx::query_scalar("SELECT alias FROM user_models WHERE id=?")
            .bind(mid)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(alias, "renamed-model");
        admin_update_user(
            &state,
            &json!({"id":2,"request_limits":{"max_concurrency":6,"rpm":60}}),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TRIGGER reject_user_limit BEFORE UPDATE ON user_limits BEGIN SELECT RAISE(ABORT,'fixture limit failure'); END").execute(&state.pool).await.unwrap();
        assert!(admin_update_user(
            &state,
            &json!({"id":2,"role":"admin","request_limits":{"max_concurrency":7,"rpm":70}})
        )
        .await
        .is_err());
        let user: (String, i64) =
            sqlx::query_as("SELECT role,session_version FROM users WHERE id=2")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(user, ("user".into(), 0));
        assert_eq!(
            crate::gateway_limits::read(&state, "user", 2)
                .await
                .unwrap()
                .rpm,
            60
        );
        let other = create_key(&state, 2, json!({"name":"other-owner"}))
            .await
            .unwrap()["id"]
            .as_i64()
            .unwrap();
        assert!(update_key(
            &state,
            1,
            json!({"id":other,"request_limits":{"max_concurrency":1,"rpm":1}})
        )
        .await
        .is_err());
    }

    #[test]
    fn damaged_upstream_permissions_never_expand_to_unrestricted_access() {
        for stored in ["broken", "null", "{}", "[3]", "[\"\"]"] {
            assert!(parse_key_models(stored).is_err(), "{stored}");
        }
        assert_eq!(parse_key_models("[]").unwrap(), Vec::<String>::new());
        assert_eq!(parse_key_models("[\"*\"]").unwrap(), vec!["*"]);
        assert!(validate_base_url("https://:embedded-secret@supplier.test/v1").is_err());
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn public_response_continuations_and_corrupt_permissions_are_isolated_before_dispatch() {
        use axum::{
            body::{to_bytes, Body},
            http::{HeaderMap, Method, Request},
            routing::post,
            Router,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut state = state().await;
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        state.client = Client::builder().no_proxy().build().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new().route("/v1/responses",post(move |Json(body):Json<Value>| {let count=count.clone();async move {
            let n=count.fetch_add(1,Ordering::SeqCst);
            if body["stream"]==true {
                axum::response::Response::builder().header("content-type","text/event-stream").body(Body::from(format!("data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_stream_{n}\"}}}}\n\ndata: [DONE]\n\n"))).unwrap()
            } else {axum::response::IntoResponse::into_response(Json(json!({"id":format!("resp_buffer_{n}"),"output":[],"usage":{"input_tokens":1,"output_tokens":2}})))}
        }}))).await.unwrap()
        });
        let pid=create_provider(&state,1,json!({"name":"resource fixture","provider_type":"openai","base_url":format!("http://{address}/v1"),"api_key":"supplier-fixture-key"})).await.unwrap()["id"].as_i64().unwrap();
        create_model(&state,1,json!({"provider_id":pid,"alias":"shared-response","upstream_model":"fixture","api_config":{"api_type":"responses"}})).await.unwrap();
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let first = create_key(&state, 1, json!({"name":"owner"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let other = create_key(&state, 2, json!({"name":"other"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let send = |key: &str, path: &str, payload: Value| {
            let state = state.clone();
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .body(Body::from(payload.to_string()))
                .unwrap();
            async move { crate::proxy::passthrough(State(state), request).await }
        };
        for (stream, id) in [(false, "resp_buffer_0"), (true, "resp_stream_2")] {
            let response = send(
                &first,
                "/v1/responses",
                json!({"model":"shared-response","stream":stream}),
            )
            .await
            .unwrap();
            let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains(id));
            let before = calls.load(Ordering::SeqCst);
            assert!(matches!(
                send(
                    &other,
                    "/v1/responses",
                    json!({"model":"shared-response","previous_response_id":id})
                )
                .await,
                Err(AppError::Forbidden(_))
            ));
            assert_eq!(calls.load(Ordering::SeqCst), before);
            let response = send(
                &first,
                "/v1/responses",
                json!({"model":"shared-response","previous_response_id":id}),
            )
            .await
            .unwrap();
            to_bytes(response.into_body(), 10000).await.unwrap();
        }
        let before = calls.load(Ordering::SeqCst);
        assert!(send(
            &first,
            "/v1/responses",
            json!({"model":"shared-response","previous_response_id":"resp_unregistered"})
        )
        .await
        .is_err());
        for path in [
            "/v1/files",
            "/v1/responses/../admin",
            "/v1/responses/%2e%2e/admin",
        ] {
            assert!(send(&first, path, json!({"model":"shared-response"}))
                .await
                .is_err());
        }
        // The low-level single-credential path must reject unknown affinity too.
        assert!(crate::direct_provider::forward(
            &state,
            pid,
            Method::POST,
            "/v1/responses",
            &HeaderMap::new(),
            &json!({"model":"fixture","previous_response_id":"resp_unregistered"})
        )
        .await
        .is_err());
        sqlx::query("UPDATE provider_keys SET allowed_models='broken' WHERE provider_id=?")
            .bind(pid)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            send(&first, "/v1/responses", json!({"model":"shared-response"}))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), before);
        server.abort();
    }

    #[tokio::test]
    async fn direct_accounts_share_provider_address() {
        let state = state().await;
        let provider = create_provider(&state, 1, json!({"name":"shared", "provider_type":"openai-compatible", "base_url":"https://example.com/v1", "api_key":"secret"})).await.unwrap();
        let pid = provider["id"].as_i64().unwrap();
        let created =
            crate::direct_provider::create(&state, pid, &json!({"name":"second", "api_key":"key"}))
                .await
                .unwrap();
        let aid = created["id"].as_i64().unwrap();
        assert!(crate::direct_provider::create(
            &state,
            pid,
            &json!({"name":"wrong", "base_url":"https://other.test/v1"})
        )
        .await
        .is_err());
        assert!(crate::direct_provider::update(
            &state,
            pid,
            &json!({"account_id":aid, "name":"wrong", "base_url":"https://other.test/v1"})
        )
        .await
        .is_err());
        crate::direct_provider::update(
            &state,
            pid,
            &json!({"account_id":aid, "name":"renamed", "api_key":"replacement"}),
        )
        .await
        .unwrap();
        // Simulate an old account with a different stored endpoint.
        sqlx::query("UPDATE provider_keys SET base_url='https://legacy.test/v1' WHERE id=?")
            .bind(aid)
            .execute(&state.pool)
            .await
            .unwrap();
        let keys = crate::direct_provider::keys(&state, pid).await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys
            .iter()
            .all(|key| key.base_url == "https://example.com/v1"));
        assert_eq!(
            keys.iter().find(|key| key.id == aid).unwrap().name,
            "renamed"
        );
        update_provider(
            &state,
            1,
            json!({"id":pid, "base_url":"https://new.test/v1"}),
        )
        .await
        .unwrap();
        assert!(crate::direct_provider::keys(&state, pid)
            .await
            .unwrap()
            .iter()
            .all(|key| key.base_url == "https://new.test/v1"));
    }

    #[tokio::test]
    async fn custom_provider_avatar_is_saved_and_scoped() {
        let mut state = state().await;
        let oss = mock_oss().await;
        Arc::make_mut(&mut state.config).oss = oss.config.clone();
        let avatar = PNG_AVATAR;
        let created = create_provider(
            &state,
            1,
            json!({
                "name":"Custom avatar", "provider_type":"openai-compatible",
                "base_url":"https://example.com/v1", "avatar_url":avatar
            }),
        )
        .await
        .unwrap();
        let provider_id = created["id"].as_i64().unwrap();
        assert!(created["avatar_url"]
            .as_str()
            .unwrap()
            .starts_with("/media/avatars/"));
        assert_eq!(oss.objects.lock().unwrap().len(), 1);
        assert!(oss
            .objects
            .lock()
            .unwrap()
            .keys()
            .all(|key| key.starts_with("JunoHub/provider/1/")));
        let listed = list_providers(&state, 1).await.unwrap();
        assert_eq!(listed["list"][0]["avatar_url"], created["avatar_url"]);
        assert!(list_providers(&state, 2).await.unwrap()["list"]
            .as_array()
            .unwrap()
            .is_empty());

        let unchanged = update_provider(
            &state,
            1,
            json!({"id":provider_id, "name":"Renamed", "avatar_url":created["avatar_url"]}),
        )
        .await
        .unwrap();
        assert_eq!(unchanged["avatar_url"], created["avatar_url"]);
        assert_eq!(oss.objects.lock().unwrap().len(), 1);
        let updated_image =
            update_provider(&state, 1, json!({"id":provider_id, "avatar_url":avatar}))
                .await
                .unwrap();
        assert_ne!(updated_image["avatar_url"], created["avatar_url"]);
        assert_eq!(oss.objects.lock().unwrap().len(), 2);

        let updated = update_provider(&state, 1, json!({"id":provider_id, "avatar_url":""}))
            .await
            .unwrap();
        assert_eq!(updated["avatar_url"], "");
        assert_eq!(
            list_providers(&state, 1).await.unwrap()["list"][0]["avatar_url"],
            ""
        );
        assert!(
            update_provider(&state, 2, json!({"id":provider_id, "avatar_url":avatar}))
                .await
                .is_err()
        );
        assert!(update_provider(
            &state,
            1,
            json!({"id":provider_id, "avatar_url":"https://example.com/avatar.png"})
        )
        .await
        .is_err());
        assert!(create_provider(
            &state,
            1,
            json!({
                "name":"Built in avatar", "provider_type":"openai",
                "base_url":"https://example.com/v1", "avatar_url":avatar
            })
        )
        .await
        .is_err());
        let built_in = create_provider(
            &state,
            1,
            json!({
                "name":"Built in", "provider_type":"openai", "base_url":"https://example.com/v1"
            }),
        )
        .await
        .unwrap();
        assert!(
            update_provider(&state, 1, json!({"id":built_in["id"], "avatar_url":avatar}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn oss_avatar_read_failure_and_owner_rules() {
        use crate::image_storage::{persist_avatar, serve_avatar, AvatarPurpose};
        use std::sync::atomic::Ordering;
        let mut state = state().await;
        let oss = mock_oss().await;
        Arc::make_mut(&mut state.config).oss = oss.config.clone();
        let saved = update_account_profile(&state, 1, &json!({"avatar_url":PNG_AVATAR}))
            .await
            .unwrap();
        let url = saved["avatar_url"].as_str().unwrap();
        let asset_id = url.trim_start_matches("/media/avatars/").to_owned();
        let response = serve_avatar(State(state.clone()), Path(asset_id.clone()))
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "image/png");
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap();
        assert_eq!(
            bytes.as_ref(),
            STANDARD
                .decode(PNG_AVATAR.split_once(',').unwrap().1)
                .unwrap()
        );
        assert!(
            update_account_profile(&state, 2, &json!({"avatar_url":url}))
                .await
                .is_err()
        );
        assert!(persist_avatar(&state, 1, AvatarPurpose::Provider, url)
            .await
            .is_err());
        update_account_profile(
            &state,
            1,
            &json!({"display_name":"New Name", "avatar_url":url}),
        )
        .await
        .unwrap();
        assert_eq!(oss.objects.lock().unwrap().len(), 1);
        oss.fail.store(true, Ordering::SeqCst);
        assert!(update_account_profile(
            &state,
            1,
            &json!({"display_name":"Failed Name", "avatar_url":PNG_AVATAR})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("403"));
        let unchanged = profile(&state, 1).await.unwrap();
        assert_eq!(unchanged["avatar_url"], url);
        assert_eq!(unchanged["display_name"], "New Name");
        oss.fail.store(false, Ordering::SeqCst);
        update_account_profile(&state, 1, &json!({"avatar_url":""}))
            .await
            .unwrap();
        assert!(matches!(
            serve_avatar(State(state.clone()), Path(asset_id)).await,
            Err(AppError::NotFound(_))
        ));
        let unattached = persist_avatar(&state, 1, AvatarPurpose::User, PNG_AVATAR)
            .await
            .unwrap();
        assert!(serve_avatar(
            State(state),
            Path(unattached.trim_start_matches("/media/avatars/").into())
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn legacy_avatars_move_to_their_oss_directories() {
        let mut state = state().await;
        let oss = mock_oss().await;
        Arc::make_mut(&mut state.config).oss = oss.config.clone();
        sqlx::query("UPDATE users SET avatar_url=? WHERE id=1")
            .bind(PNG_AVATAR)
            .execute(&state.pool)
            .await
            .unwrap();
        let created = create_provider(&state, 1, json!({"name":"legacy", "provider_type":"openai-compatible", "base_url":"https://example.com/v1"})).await.unwrap();
        sqlx::query("UPDATE user_providers SET avatar_url=? WHERE id=?")
            .bind(PNG_AVATAR)
            .bind(created["id"].as_i64().unwrap())
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            crate::image_storage::migrate_legacy_avatars(&state)
                .await
                .unwrap(),
            2
        );
        assert!(profile(&state, 1).await.unwrap()["avatar_url"]
            .as_str()
            .unwrap()
            .starts_with("/media/avatars/"));
        {
            let objects = oss.objects.lock().unwrap();
            assert!(objects.keys().any(|key| key.starts_with("JunoHub/user/1/")));
            assert!(objects
                .keys()
                .any(|key| key.starts_with("JunoHub/provider/1/")));
        }
        assert_eq!(
            crate::image_storage::migrate_legacy_avatars(&state)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn removed_group_members_remain_saved_and_names_are_unique() {
        let state = state().await;
        let provider = create_provider(&state,1,json!({"name":"test","provider_type":"openai-compatible","base_url":"https://example.com/v1","api_key":"secret"})).await.unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        let first = crate::direct_provider::keys(&state, provider_id)
            .await
            .unwrap()[0]
            .id;
        let payload = json!({"name":"Work","base_url":"https://example.com/v1","api_key":"key"});
        crate::direct_provider::create(&state, provider_id, &payload)
            .await
            .unwrap();
        assert!(crate::direct_provider::create(
            &state,
            provider_id,
            &json!({"name":" work ","base_url":"https://example.com/v1"})
        )
        .await
        .is_err());
        assert!(crate::direct_provider::update(
            &state,
            provider_id,
            &json!({"account_id":first,"name":"WORK"})
        )
        .await
        .is_err());
        let update = crate::sub2_policy::AccountDispatch {
            member_order: 7,
            id: first,
            priority: 0,
            weight: 1,
            concurrency: 10,
            rpm: 0,
            allowed_models: vec![],
        };
        crate::direct_provider::update_dispatch(&state, provider_id, std::slice::from_ref(&update))
            .await
            .unwrap();
        let keys = crate::direct_provider::keys(&state, provider_id)
            .await
            .unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.iter().find(|k| k.id == first).unwrap().member_order, 7);
        assert_eq!(keys.iter().find(|k| k.id == first).unwrap().priority, 0);
        assert!(!crate::sub2_policy::rank_accounts(
            std::slice::from_ref(&update),
            "priority",
            0,
            &[]
        )
        .contains(&first));
        let listed = crate::direct_provider::list(&state, provider_id)
            .await
            .unwrap();
        assert_eq!(
            listed["list"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["id"] == first)
                .unwrap()["schedulable"],
            false
        );
        let mut restored = update;
        restored.priority = 2;
        crate::direct_provider::update_dispatch(&state, provider_id, &[restored])
            .await
            .unwrap();
        assert_eq!(
            crate::direct_provider::keys(&state, provider_id)
                .await
                .unwrap()
                .iter()
                .find(|k| k.id == first)
                .unwrap()
                .priority,
            2
        );
    }

    #[tokio::test]
    async fn connection_diagnostics_classify_responses_and_recheck_cooling_keys() {
        use axum::{
            http::{HeaderMap, StatusCode},
            routing::{get, post},
            Router,
        };
        let router = Router::new()
            .route(
                "/empty/models",
                get(|| async { axum::Json(json!({"data":[]})) }),
            )
            .route(
                "/v1/models",
                get(|headers: HeaderMap| async move {
                    if headers.get("authorization").and_then(|h| h.to_str().ok())
                        == Some("Bearer good")
                    {
                        (
                            StatusCode::OK,
                            axum::Json(json!({"data":[{"id":"test-model"}]})),
                        )
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(json!({"error":"secret must not leak"})),
                        )
                    }
                }),
            )
            .route(
                "/custom/chat/completions",
                post(|axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["model"], "test-model");
                    axum::Json(json!({"choices":[{"message":{"content":"Hi"}}]}))
                }),
            )
            .route("/html/models", get(|| async { "<html>login</html>" }))
            .route(
                "/error/models",
                get(|| async { axum::Json(json!({"error":{"message":"secret must not leak"}})) }),
            )
            .route(
                "/claude/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-api-key"], "good");
                    assert_eq!(headers["anthropic-version"], "2023-06-01");
                    axum::Json(json!({"data":[]}))
                }),
            )
            .route(
                "/gemini/v1beta/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-goog-api-key"], "good");
                    axum::Json(json!({"models":[{"name":"models/test"}]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let state = state().await;
        for (path, key, platform, expected) in [
            ("/empty", "", "custom", "success"),
            ("/v1/chat/completions", "good", "custom", "success"),
            ("/v1", "bad", "custom", "error"),
            ("/custom", "good", "custom", "warning"),
            ("/html", "", "custom", "warning"),
            ("/error", "", "custom", "error"),
            ("/claude", "good", "anthropic", "success"),
            ("/gemini", "good", "gemini", "success"),
        ] {
            let report =
                crate::upstream_probe::probe(&state, &format!("{base}{path}"), key, platform, None)
                    .await
                    .unwrap();
            assert_eq!(report.outcome, expected, "{path}: {}", report.message);
            assert!(!serde_json::to_string(&report)
                .unwrap()
                .contains("secret must not leak"));
        }
        let missing = crate::upstream_probe::probe(&state, &base, "", "custom", None)
            .await
            .unwrap();
        assert_eq!(missing.suggested_base_url, Some(format!("{base}/v1")));
        assert!(
            crate::upstream_probe::probe(
                &state,
                &format!("{base}/custom"),
                "good",
                "custom",
                Some("test-model")
            )
            .await
            .unwrap()
            .ok
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_providers")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "draft diagnostics must not save providers");
        let provider = create_provider(&state,1,json!({"name":"diagnostic","provider_type":"openai-compatible","base_url":format!("{base}/v1"),"api_key":"good"})).await.unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        crate::direct_provider::create(
            &state,
            provider_id,
            &json!({"name":"expired","base_url":format!("{base}/v1"),"api_key":"bad"}),
        )
        .await
        .unwrap();
        let cooldown = store::now() + 3600;
        sqlx::query("UPDATE provider_keys SET cooldown_until=? WHERE provider_id=?")
            .bind(cooldown)
            .bind(provider_id)
            .execute(&state.pool)
            .await
            .unwrap();
        let report = test_provider(&state, 1, json!({"id":provider_id}))
            .await
            .unwrap();
        assert_eq!(report["outcome"], "warning");
        let checks = report["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks.iter().filter(|r| r["ok"] == true).count(), 1);
        assert!(test_provider(&state, 2, json!({"id":provider_id}))
            .await
            .is_err());
        let unchanged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provider_keys WHERE provider_id=? AND cooldown_until=?",
        )
        .bind(provider_id)
        .bind(cooldown)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(unchanged, 2);
        server.abort();
    }

    #[test]
    fn avatar_validation_accepts_supported_small_images() {
        let encoded = STANDARD.encode([1_u8, 2, 3, 4]);
        assert!(validate_avatar(PNG_AVATAR).is_ok());
        assert!(validate_avatar(&format!("data:image/webp;base64,{encoded}")).is_err());
        assert!(validate_avatar("").is_ok());
        assert!(validate_avatar("data:image/svg+xml;base64,PHN2Zz4=").is_err());
        assert!(validate_avatar("data:image/png;base64,not-base64").is_err());
    }

    #[tokio::test]
    async fn direct_keys_and_limits_are_tenant_scoped() {
        let state = state().await;
        let p=create_provider(&state,1,json!({"name":"direct","provider_type":"openai-compatible","base_url":"https://example.com/v1","api_key":"first-secret"})).await.unwrap();
        let id = p["id"].as_i64().unwrap();
        let before = list_provider_accounts(&state, 1, &json!({"provider_id":id}))
            .await
            .unwrap();
        assert_eq!(before["total"], 1);
        assert!(!before.to_string().contains("first-secret"));
        let first = before["list"][0]["id"].as_i64().unwrap();
        let second = crate::direct_provider::create(
            &state,
            id,
            &json!({"name":"backup","api_key":"second-secret"}),
        )
        .await
        .unwrap()["id"]
            .as_i64()
            .unwrap();
        assert!(
            list_provider_accounts(&state, 2, &json!({"provider_id":id}))
                .await
                .is_err()
        );
        assert!(update_provider_account_config(
            &state,
            2,
            &json!({"provider_id":id,"account_id":first,"name":"stolen"})
        )
        .await
        .is_err());
        update_provider_account_config(
            &state,
            1,
            &json!({"provider_id":id,"account_id":second,"name":"backup","api_key":"replacement"}),
        )
        .await
        .unwrap();
        let keys = crate::direct_provider::keys(&state, id).await.unwrap();
        assert_eq!(
            secrets::decrypt(&state.config.encryption_key, &keys[0].encrypted_api_key).unwrap(),
            "first-secret"
        );
        assert_eq!(
            secrets::decrypt(&state.config.encryption_key, &keys[1].encrypted_api_key).unwrap(),
            "replacement"
        );
        let updates = vec![
            crate::sub2_policy::AccountDispatch {
                member_order: 0,
                id: first,
                priority: 1,
                weight: 7,
                concurrency: 2,
                rpm: 20,
                allowed_models: vec![],
            },
            crate::sub2_policy::AccountDispatch {
                member_order: 0,
                id: 999999,
                priority: 2,
                weight: 3,
                concurrency: 1,
                rpm: 10,
                allowed_models: vec![],
            },
        ];
        assert!(
            crate::direct_provider::update_dispatch(&state, id, &updates)
                .await
                .is_err()
        );
        assert_eq!(
            crate::direct_provider::keys(&state, id).await.unwrap()[0].weight,
            1
        );
        assert!(resource_limits(
            &state,
            2,
            "provider",
            id,
            true,
            &json!({"max_concurrency":1,"rpm":10})
        )
        .await
        .is_err());
        let limits = resource_limits(
            &state,
            1,
            "provider",
            id,
            true,
            &json!({"max_concurrency":1,"rpm":10}),
        )
        .await
        .unwrap();
        assert_eq!(limits["rpm"], 10);
        assert!(resource_limits(
            &state,
            1,
            "provider",
            id,
            true,
            &json!({"max_concurrency":-1,"rpm":10})
        )
        .await
        .is_err());
        assert_eq!(
            crate::gateway_limits::read(&state, "provider", id)
                .await
                .unwrap()
                .max_concurrency,
            1
        );
        sqlx::query("DELETE FROM provider_keys WHERE provider_id=?")
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(load_provider(&state, 1, id)
            .await
            .unwrap()
            .api_key
            .is_empty());
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn virtual_key_limits_hold_stream_and_count_once_through_failover() {
        use axum::{
            body::Body,
            http::{HeaderMap, Request},
            routing::post,
            Router,
        };
        use redis::AsyncCommands;
        let mut state = state().await;
        let mut redis = redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
            .unwrap()
            .get_connection_manager()
            .await
            .unwrap();
        state.sub2_redis = Some(redis.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let app=Router::new().route("/v1/chat/completions",post(|headers:HeaderMap|async move {
            if headers["authorization"]=="Bearer bad" {return axum::response::Response::builder().status(503).body(Body::from("unavailable")).unwrap()}
            axum::response::Response::builder().header("content-type","text/event-stream").body(Body::from_stream(async_stream::stream! {yield Ok::<_,std::io::Error>("data: first\n\n");std::future::pending::<()>().await;})).unwrap()
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider=create_provider(&state,1,json!({"name":"mock","provider_type":"openai-compatible","base_url":base,"api_key":"bad"})).await.unwrap()["id"].as_i64().unwrap();
        crate::direct_provider::create(
            &state,
            provider,
            &json!({"name":"backup","base_url":base,"api_key":"good"}),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO provider_dispatch_policy VALUES(?,'priority',3,0,5,5)")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        create_model(
            &state,
            1,
            json!({"provider_id":provider,"alias":"test","upstream_model":"test"}),
        )
        .await
        .unwrap();
        let created = create_key(&state, 1, json!({"name":"test-key"}))
            .await
            .unwrap();
        let key = created["key"].as_str().unwrap();
        let key_id = created["id"].as_i64().unwrap();
        let created2 = create_key(&state, 1, json!({"name":"another-key"}))
            .await
            .unwrap();
        let key2 = created2["key"].as_str().unwrap();
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        resource_limits(
            &state,
            1,
            "user",
            1,
            true,
            &json!({"max_concurrency":2,"rpm":3}),
        )
        .await
        .unwrap();
        let user_active = "junohub:admission:user:1:active";
        let user_rpm = "junohub:admission:user:1:rpm";
        let _: i64 = redis.del(&[user_active, user_rpm]).await.unwrap();
        resource_limits(
            &state,
            1,
            "key",
            key_id,
            true,
            &json!({"max_concurrency":1,"rpm":3}),
        )
        .await
        .unwrap();
        let active = format!("junohub:admission:virtual-key:{key_id}:active");
        let rpm = format!("junohub:admission:virtual-key:{key_id}:rpm");
        let _: i64 = redis.del(&[&active, &rpm]).await.unwrap();
        let request = |client_key: &str| {
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", format!("Bearer {client_key}"))
                .body(Body::from(
                    json!({"model":"test","stream":true}).to_string(),
                ))
                .unwrap()
        };
        let first = crate::proxy::passthrough(State(state.clone()), request(key))
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        assert_eq!(redis.zcard::<_, i64>(&rpm).await.unwrap(), 1);
        let error = crate::proxy::passthrough(State(state.clone()), request(key))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("访问密钥"));
        assert_eq!(redis.zcard::<_, i64>(&rpm).await.unwrap(), 1);
        let held = crate::proxy::passthrough(State(state.clone()), request(key2))
            .await
            .unwrap();
        assert_eq!(held.status(), 200);
        let key3 = create_key(&state, 1, json!({"name":"third-key"}))
            .await
            .unwrap();
        let error =
            crate::proxy::passthrough(State(state.clone()), request(key3["key"].as_str().unwrap()))
                .await
                .unwrap_err();
        assert!(error.to_string().contains("用户总量"));
        assert_eq!(redis.zcard::<_, i64>(user_rpm).await.unwrap(), 2);
        drop(held);
        drop(first);
        tokio::time::timeout(Duration::from_secs(3), async {
            while redis.zcard::<_, i64>(&active).await.unwrap() != 0
                || redis.zcard::<_, i64>(user_active).await.unwrap() != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let second = crate::proxy::passthrough(State(state.clone()), request(key2))
            .await
            .unwrap();
        assert_eq!(second.status(), 200);
        drop(second);
        assert_eq!(redis.zcard::<_, i64>(&rpm).await.unwrap(), 1);
        assert_eq!(redis.zcard::<_, i64>(user_rpm).await.unwrap(), 3);
        let error = crate::proxy::passthrough(State(state.clone()), request(key))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("用户总量已达到每分钟请求数"));
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires dedicated JUNOHUB_TEST_REDIS_URL"]
    async fn direct_gateway_limits_failover_and_stream_leases() {
        use crate::gateway_limits::{self, Limits};
        use axum::{
            body::{to_bytes, Body},
            http::{HeaderMap, Method, StatusCode},
            response::Response,
            routing::post,
            Router,
        };
        let mut state = state().await;
        state.client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .read_timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        let redis = redis::Client::open(
            std::env::var("JUNOHUB_TEST_REDIS_URL").expect("dedicated test Redis"),
        )
        .unwrap();
        state.sub2_redis = Some(redis.get_connection_manager().await.unwrap());
        // Per-run namespaces avoid mutating any other test data.
        let scope = format!("test:{}", uuid::Uuid::new_v4());
        let mut jobs = vec![];
        for _ in 0..12 {
            let state = state.clone();
            let scope = scope.clone();
            jobs.push(tokio::spawn(async move {
                gateway_limits::acquire(
                    &state,
                    &[(
                        scope,
                        Limits {
                            max_concurrency: 2,
                            rpm: 100,
                        },
                    )],
                )
                .await
            }));
        }
        let mut held = vec![];
        for job in jobs {
            if let Ok(lease) = job.await.unwrap() {
                held.push(lease)
            }
        }
        assert_eq!(held.len(), 2);
        let model_scope = format!("test-model:{}", uuid::Uuid::new_v4());
        let blocked = gateway_limits::acquire(
            &state,
            &[
                (
                    model_scope.clone(),
                    Limits {
                        max_concurrency: 0,
                        rpm: 1,
                    },
                ),
                (
                    scope.clone(),
                    Limits {
                        max_concurrency: 2,
                        rpm: 100,
                    },
                ),
            ],
        )
        .await;
        assert!(matches!(blocked, Err(AppError::TooManyRequests(_))));
        // A failed multi-scope check must not consume model RPM.
        assert!(gateway_limits::acquire(
            &state,
            &[(
                model_scope.clone(),
                Limits {
                    max_concurrency: 0,
                    rpm: 1
                }
            )]
        )
        .await
        .is_ok());
        assert!(gateway_limits::acquire(
            &state,
            &[(
                model_scope,
                Limits {
                    max_concurrency: 0,
                    rpm: 1
                }
            )]
        )
        .await
        .is_err());
        drop(held);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(gateway_limits::acquire(
            &state,
            &[(
                scope,
                Limits {
                    max_concurrency: 2,
                    rpm: 100
                }
            )]
        )
        .await
        .is_ok());
        let hits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_hits = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new().route("/v1/responses",post(|headers:HeaderMap|async move {Json(json!({"id":"resp_fixture","account":headers.get("authorization").unwrap().to_str().unwrap()}))})).route("/v1/chat/completions",post(move |headers:HeaderMap,Json(body):Json<Value>|{let hits=server_hits.clone();async move {
            let auth=headers.get("authorization").unwrap().to_str().unwrap().to_owned();hits.lock().unwrap().push(auth.clone());
            if body["model"]=="invalid" {return Response::builder().status(400).body(Body::from("invalid")).unwrap()}
            if body["model"]=="slow" {tokio::time::sleep(Duration::from_secs(6)).await;}
            if auth=="Bearer bad" {return Response::builder().status(503).body(Body::from("unavailable")).unwrap()}
            Response::builder().header("content-type","text/event-stream").body(Body::from_stream(async_stream::stream!{yield Ok::<_,std::io::Error>("data: first\n\n");tokio::time::sleep(Duration::from_secs(10)).await;yield Ok::<_,std::io::Error>("data: [DONE]\n\n");})).unwrap()
        }}))).await.unwrap()
        });
        let p=create_provider(&state,1,json!({"name":"fixture","provider_type":"openai-compatible","base_url":format!("http://{address}/v1"),"api_key":"bad"})).await.unwrap();
        let provider = p["id"].as_i64().unwrap();
        let first = crate::direct_provider::keys(&state, provider)
            .await
            .unwrap()[0]
            .id;
        let second = crate::direct_provider::create(
            &state,
            provider,
            &json!({"name":"backup","base_url":format!("http://{address}/v1"),"api_key":"good"}),
        )
        .await
        .unwrap()["id"]
            .as_i64()
            .unwrap();
        sqlx::query("INSERT INTO provider_dispatch_policy VALUES(?,'priority',3,0,5,5)")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE provider_keys SET concurrency=1,priority=CASE WHEN id=? THEN 1 ELSE 2 END WHERE provider_id=?").bind(first).bind(provider).execute(&state.pool).await.unwrap();
        let forward = |model: &str| {
            let state = state.clone();
            let model = model.to_owned();
            async move {
                crate::direct_provider::forward(
                    &state,
                    provider,
                    Method::POST,
                    "/v1/chat/completions",
                    &HeaderMap::new(),
                    &json!({"model":model,"stream":true}),
                )
                .await
            }
        };
        let response = forward("test").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*hits.lock().unwrap(), vec!["Bearer bad", "Bearer good"]);
        assert!(matches!(
            forward("test").await,
            Err(AppError::TooManyRequests(_))
        ));
        drop(response);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Dropping a never-polled body must release the lease too.
        let response = forward("test").await.unwrap();
        drop(response);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = hits.lock().unwrap().len();
        let invalid = forward("invalid").await.unwrap();
        assert_eq!(invalid.status(), 400);
        assert_eq!(hits.lock().unwrap().len(), before + 1);
        drop(invalid);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            forward("slow").await,
            Err(AppError::GatewayTimeout(_))
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        use redis::AsyncCommands;
        let mut redis = state.sub2_redis.clone().unwrap();
        let active: i64 = redis
            .zcard(format!("junohub:admission:direct-key:{second}:active"))
            .await
            .unwrap();
        assert_eq!(active, 0);
        sqlx::query("UPDATE provider_keys SET cooldown_until=0 WHERE provider_id=?")
            .bind(provider)
            .execute(&state.pool)
            .await
            .unwrap();
        let mut owner_headers = HeaderMap::new();
        owner_headers.insert("authorization", "Bearer owner-a".parse().unwrap());
        let response = crate::direct_provider::forward(
            &state,
            provider,
            Method::POST,
            "/v1/responses",
            &owner_headers,
            &json!({"model":"test"}),
        )
        .await
        .unwrap();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Bearer bad"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        sqlx::query("UPDATE provider_keys SET priority=CASE WHEN id=? THEN 2 ELSE 1 END WHERE provider_id=?").bind(first).bind(provider).execute(&state.pool).await.unwrap();
        let response = crate::direct_provider::forward(
            &state,
            provider,
            Method::POST,
            "/v1/responses",
            &owner_headers,
            &json!({"model":"test","previous_response_id":"resp_fixture"}),
        )
        .await
        .unwrap();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Bearer bad"));
        owner_headers.insert("authorization", "Bearer owner-b".parse().unwrap());
        assert!(matches!(
            crate::direct_provider::forward(
                &state,
                provider,
                Method::POST,
                "/v1/responses",
                &owner_headers,
                &json!({"model":"test","previous_response_id":"resp_fixture"})
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        // Exercise the actual proxy's provider + model admission and 429 logging.
        create_model(
            &state,
            1,
            json!({"provider_id":provider,"alias":"limited","upstream_model":"test"}),
        )
        .await
        .unwrap();
        let route = resolve_model(&state, 1, "limited").await.unwrap();
        resource_limits(
            &state,
            1,
            "model",
            route.id,
            true,
            &json!({"max_concurrency":0,"rpm":1}),
        )
        .await
        .unwrap();
        let client_key = create_key(&state, 1, json!({"name":"fixture"}))
            .await
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = || {
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", format!("Bearer {client_key}"))
                .body(Body::from(
                    json!({"model":"limited","stream":true}).to_string(),
                ))
                .unwrap()
        };
        let response = crate::proxy::passthrough(State(state.clone()), request())
            .await
            .unwrap();
        drop(response);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            crate::proxy::passthrough(State(state.clone()), request()).await,
            Err(AppError::TooManyRequests(_))
        ));
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs WHERE status_code=429")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(count >= 1);
        let _ = to_bytes(Body::empty(), 100).await;
        server.abort();
    }

    #[tokio::test]
    async fn account_profile_and_password_updates_are_persisted() {
        let mut state = state().await;
        let oss = mock_oss().await;
        Arc::make_mut(&mut state.config).oss = oss.config.clone();
        let current_hash = secrets::hash_password("current-password").unwrap();
        sqlx::query("UPDATE users SET password_hash=? WHERE id=1")
            .bind(current_hash)
            .execute(&state.pool)
            .await
            .unwrap();

        let avatar = PNG_AVATAR;
        let updated = update_account_profile(
            &state,
            1,
            &json!({"display_name":"New Name","avatar_url":avatar}),
        )
        .await
        .unwrap();
        assert_eq!(updated["display_name"], "New Name");
        assert!(updated["avatar_url"]
            .as_str()
            .unwrap()
            .starts_with("/media/avatars/"));
        assert!(oss
            .objects
            .lock()
            .unwrap()
            .keys()
            .all(|key| key.starts_with("JunoHub/user/1/")));

        update_account_password(
            &state,
            1,
            &json!({"current_password":"current-password","new_password":"new-password"}),
        )
        .await
        .unwrap();
        let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=1")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert!(secrets::verify_password("new-password", &stored));
        assert!(!secrets::verify_password("current-password", &stored));
    }

    #[tokio::test]
    async fn stats_counts_requests_since_the_client_day_start() {
        let state = state().await;
        for (user_id, created_at) in [(1_i64, 100_i64), (1, 200), (2, 300)] {
            sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,created_at) VALUES(?,0,'test',?)")
                .bind(user_id)
                .bind(created_at)
                .execute(&state.pool)
                .await
                .unwrap();
        }
        let result = stats(&state, 1, &json!({"since":150})).await.unwrap();
        assert_eq!(result["requests"], 1);
    }

    #[tokio::test]
    async fn log_filters_apply_to_the_full_authorized_result_set() {
        let state = state().await;
        for (user_id, model, provider, status, created_at) in [
            (1_i64, "alpha", "Provider A", 200_i64, 100_i64),
            (2, "beta", "Provider B", 500, 200),
            (2, "alpha", "Provider B", 404, 300),
        ] {
            sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,provider_name,status_code,created_at) VALUES(?,0,?,?,?,?)")
                .bind(user_id).bind(model).bind(provider).bind(status).bind(created_at)
                .execute(&state.pool).await.unwrap();
        }

        let own = list_logs(&state, 1, &json!({})).await.unwrap();
        assert_eq!(own["scope"], "own");
        assert_eq!(own["total"], 1);
        assert_eq!(own["filters"]["users"].as_array().unwrap().len(), 0);

        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let all = list_logs(&state, 1, &json!({})).await.unwrap();
        assert_eq!(all["scope"], "all");
        assert_eq!(all["total"], 3);
        assert_eq!(all["filters"]["users"].as_array().unwrap().len(), 2);

        let filtered = list_logs(&state, 1, &json!({"user_id":2,"model":"alpha","provider":"Provider B","status":"4xx","from":250,"to":400})).await.unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["list"][0]["owner_id"], 2);
        assert_eq!(filtered["list"][0]["status_code"], 404);
        assert_eq!(filtered["list"][0]["provider_name"], "Provider B");
        let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id=2")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        let partial = list_logs(&state, 1, &json!({"user_search":email.split('@').next().unwrap(),"model":"ALP","provider":"vider b"})).await.unwrap();
        assert_eq!(partial["total"], 1);
        assert_eq!(partial["list"][0]["owner_id"], 2);
        let escaped = list_logs(&state, 1, &json!({"model":"%"})).await.unwrap();
        assert_eq!(escaped["total"], 0);
        let searched = list_logs(&state, 1, &json!({"q":"beta Provider B"}))
            .await
            .unwrap();
        assert_eq!(searched["total"], 1);
        assert_eq!(searched["list"][0]["model_alias"], "beta");
    }

    #[tokio::test]
    async fn sub2api_provision_creates_provider_without_importing_models() {
        let state = state().await;
        let result = provision_sub2api_provider(&state, 1, "Sub2API · OpenAI", "sub2-secret-key")
            .await
            .unwrap();
        assert!(result["provider_id"].as_i64().unwrap() > 0);
        let provider: (String, String) = sqlx::query_as(
            "SELECT provider_type,encrypted_api_key FROM user_providers WHERE user_id=1",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(provider.0, "sub2api");
        assert_ne!(provider.1, "sub2-secret-key");
        assert_eq!(
            secrets::decrypt(&state.config.encryption_key, &provider.1).unwrap(),
            "sub2-secret-key"
        );
        let model_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_models WHERE user_id=1")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(model_count, 0);
        let duplicate =
            provision_sub2api_provider(&state, 1, "Sub2API · OpenAI", "another-key").await;
        assert!(duplicate.is_ok());
    }

    #[tokio::test]
    async fn provider_list_includes_managed_platform() {
        let state = state().await;
        let provider = create_provider(
            &state,
            1,
            json!({"name":"Sub2API · Claude","provider_type":"sub2api","base_url":"https://example.com/v1","api_key":"secret"}),
        )
        .await
        .unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        let timestamp = store::now();
        sqlx::query(
            "INSERT INTO sub2_managed_pools(provider_id,user_id,platform,group_id,api_key_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
        )
        .bind(provider_id)
        .bind(1_i64)
        .bind("anthropic")
        .bind(10_i64)
        .bind(20_i64)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&state.pool)
        .await
        .unwrap();

        let result = list_providers(&state, 1).await.unwrap();
        assert_eq!(result["list"][0]["managed"], true);
        assert_eq!(result["list"][0]["platform"], "anthropic");
    }

    #[tokio::test]
    async fn policy_round_trip_and_tenant_isolation() {
        let state = state().await;
        // Administrative dispatch operations require the current database role;
        // use two admins so the foreign-provider assertion exercises ownership.
        sqlx::query("UPDATE users SET role='admin'")
            .execute(&state.pool)
            .await
            .unwrap();
        let provider = create_provider(&state, 1, json!({"name":"pool","provider_type":"sub2api","base_url":"https://example.com/v1","api_key":"secret"})).await.unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        sqlx::query("INSERT INTO sub2_managed_pools(provider_id,user_id,platform,group_id,api_key_id,created_at,updated_at) VALUES(?,1,'openai',10,20,0,0)")
            .bind(provider_id).execute(&state.pool).await.unwrap();
        let user = |uid| {
            Extension(CurrentUser {
                uid,
                sub: format!("user-{uid}"),
                role: "admin".into(),
            })
        };
        let payload = json!({"id":provider_id,"strategy":"round_robin","max_attempts":3,"sticky_sessions":false});
        assert!(dispatch(
            State(state.clone()),
            user(2),
            Path("provider/policy/update".into()),
            Json(payload.clone())
        )
        .await
        .is_err());
        let _ = dispatch(
            State(state.clone()),
            user(1),
            Path("provider/policy/update".into()),
            Json(payload),
        )
        .await
        .unwrap();
        let saved = crate::sub2_policy::for_group(&state, 10).await.unwrap();
        assert_eq!(saved.strategy, "round_robin");
        assert_eq!(saved.max_attempts, 3);
        assert!(!saved.sticky_sessions);
        assert!(dispatch(
            State(state.clone()),
            user(2),
            Path("provider/policy".into()),
            Json(json!({"id":provider_id}))
        )
        .await
        .is_err());
        assert!(dispatch(State(state.clone()), user(1), Path("provider/policy/update".into()), Json(json!({"id":provider_id,"strategy":"invalid","max_attempts":3,"sticky_sessions":true}))).await.is_err());
        assert_eq!(
            crate::sub2_policy::for_group(&state, 10)
                .await
                .unwrap()
                .strategy,
            "round_robin"
        );
        assert_eq!(
            crate::sub2_policy::for_group(&state, 999)
                .await
                .unwrap()
                .strategy,
            "balanced"
        );
    }

    #[test]
    fn account_priority_updates_validate_range_and_duplicates() {
        let updates = account_priority_updates(&json!({
            "accounts":[{"id":11,"priority":40},{"id":12,"priority":60}]
        }))
        .unwrap();
        assert_eq!(updates, vec![(11, 40), (12, 60)]);
        assert!(account_priority_updates(&json!({
            "accounts":[{"id":11,"priority":0}]
        }))
        .is_err());
        assert!(account_priority_updates(&json!({
            "accounts":[{"id":11,"priority":40},{"id":11,"priority":60}]
        }))
        .is_err());
    }

    #[tokio::test]
    async fn model_import_adds_new_aliases_and_skips_duplicates() {
        let state = state().await;
        let provider = create_provider(
            &state,
            1,
            json!({"name":"sync","provider_type":"sub2api","base_url":"https://example.com/v1","api_key":"secret"}),
        )
        .await
        .unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        let first = import_models(
            &state,
            1,
            json!({"provider_id":provider_id,"models":["gpt-a","gpt-b"]}),
        )
        .await
        .unwrap();
        assert_eq!(first["imported_count"], 2);
        let second = import_models(
            &state,
            1,
            json!({"provider_id":provider_id,"models":["gpt-a","gpt-c"]}),
        )
        .await
        .unwrap();
        assert_eq!(second["imported_count"], 1);
        assert_eq!(second["skipped_count"], 1);
    }

    #[tokio::test]
    async fn admin_key_access_includes_owners_and_demotion_revokes_access() {
        let state = state().await;
        let own = create_key(&state, 1, json!({"name":"own"})).await.unwrap();
        let other = create_key(&state, 2, json!({"name":"other"}))
            .await
            .unwrap();
        let other_id = other["id"].as_i64().unwrap();
        let normal = list_keys(&state, 1).await.unwrap();
        assert_eq!(normal["total"], 1);
        assert_eq!(normal["scope"], "own");
        assert!(reveal_key(&state, 1, other_id).await.is_err());
        assert!(update_key(
            &state,
            1,
            json!({"id":other_id,"name":"forged","role":"admin"})
        )
        .await
        .is_err());
        assert!(delete_key(&state, 1, other_id).await.is_err());
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let all = list_keys(&state, 1).await.unwrap();
        assert_eq!(all["total"], 2);
        assert_eq!(all["scope"], "all");
        let own_as_admin = list_own_keys(&state, 1).await.unwrap();
        assert_eq!(own_as_admin["total"], 1);
        assert_eq!(own_as_admin["scope"], "own");
        assert_eq!(own_as_admin["list"][0]["id"], own["id"]);
        assert_eq!(all["list"][0]["owner_id"], 2);
        assert!(all["list"][0]["owner_email"]
            .as_str()
            .unwrap()
            .contains('@'));
        assert_eq!(
            reveal_key(&state, 1, other_id).await.unwrap()["key"],
            other["key"]
        );
        update_key(&state, 1, json!({"id":other_id,"name":"renamed"}))
            .await
            .unwrap();
        assert_eq!(
            list_keys(&state, 2).await.unwrap()["list"][0]["owner_id"],
            2
        );
        sqlx::query("UPDATE users SET role='user' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(reveal_key(&state, 1, other_id).await.is_err());
        assert_eq!(
            list_keys(&state, 1).await.unwrap()["list"][0]["id"],
            own["id"]
        );
        sqlx::query("UPDATE users SET role='admin' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        delete_key(&state, 1, other_id).await.unwrap();
        assert_eq!(list_keys(&state, 2).await.unwrap()["total"], 0);
    }

    #[tokio::test]
    async fn key_reveal_is_owned_encrypted_and_legacy_keys_backfill() {
        let state = state().await;
        let created = create_key(&state, 1, json!({"name":"reusable"}))
            .await
            .unwrap();
        let id = created["id"].as_i64().unwrap();
        let raw = created["key"].as_str().unwrap();
        assert_eq!(reveal_key(&state, 1, id).await.unwrap()["key"], raw);
        update_key(&state, 1, json!({"id":id,"name":"renamed","status":0}))
            .await
            .unwrap();
        assert!(gateway_key(&state, raw).await.unwrap().is_none());
        assert_eq!(reveal_key(&state, 1, id).await.unwrap()["key"], raw);
        assert!(update_key(&state, 1, json!({"id":id,"status":3}))
            .await
            .is_err());
        update_key(&state, 1, json!({"id":id,"status":1}))
            .await
            .unwrap();
        assert_eq!(
            list_keys(&state, 1).await.unwrap()["list"][0]["name"],
            "renamed"
        );

        assert!(reveal_key(&state, 2, id).await.is_err());
        let encrypted: String =
            sqlx::query_scalar("SELECT encrypted_key FROM virtual_keys WHERE id=?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(!encrypted.contains(raw));
        let listed = list_keys(&state, 1).await.unwrap();
        assert_eq!(listed["list"][0]["can_reveal"], true);
        assert!(listed["list"][0].get("key").is_none());
        sqlx::query("UPDATE virtual_keys SET encrypted_key=NULL WHERE id=?")
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(reveal_key(&state, 1, id).await.is_err());
        assert!(gateway_key(&state, "invalid-key").await.unwrap().is_none());
        assert!(gateway_key(&state, raw).await.unwrap().is_some());
        assert_eq!(reveal_key(&state, 1, id).await.unwrap()["key"], raw);
    }

    #[tokio::test]
    async fn key_cost_accumulates_without_repricing_history_or_hiding_unknown_usage() {
        let state = state().await;
        let provider = create_provider(&state, 1, json!({"name":"prices","provider_type":"openai","base_url":"https://example.com/v1","api_key":"test"})).await.unwrap();
        let model = create_model(&state, 1, json!({"provider_id":provider["id"],"alias":"priced-key","upstream_model":"upstream","input_price_per_million":2.5,"output_price_per_million":10})).await.unwrap();
        let route = resolve_model(&state, 1, "priced-key").await.unwrap();
        let created = create_key(&state, 1, json!({"name":"usage"}))
            .await
            .unwrap();
        let key = gateway_key(&state, created["key"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        record_request(
            &state, &key, &route, "openai", 200, 10, "", 1_000_000, 100_000,
        )
        .await;
        update_model(
            &state,
            1,
            json!({"id":model["id"],"input_price_per_million":5,"output_price_per_million":null}),
        )
        .await
        .unwrap();
        let route = resolve_model(&state, 1, "priced-key").await.unwrap();
        record_request(
            &state, &key, &route, "openai", 200, 10, "", 1_000_000, 100_000,
        )
        .await;
        update_key(&state, 1, json!({"id":key.id,"name":"renamed usage"}))
            .await
            .unwrap();
        let logs = list_logs(&state, 1, &json!({})).await.unwrap();
        assert_eq!(logs["list"][0]["key_name"], "usage");
        assert_eq!(logs["list"][1]["key_name"], "usage");
        let listed = list_keys(&state, 1).await.unwrap();
        let usage = &listed["list"][0];
        assert_eq!(usage["cost_usd"], 3.5);
        assert_eq!(usage["unpriced_requests"], 1);
        assert_eq!(usage["total_requests"], 2);
        assert_eq!(usage["input_tokens"], 2_000_000);
        assert_eq!(usage["output_tokens"], 200_000);
        let costs: Vec<Option<f64>> =
            sqlx::query_scalar("SELECT cost_usd FROM user_request_logs ORDER BY id")
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(costs, vec![Some(3.5), None]);
    }

    #[tokio::test]
    async fn model_prices_persist_clear_and_preserve_on_partial_update() {
        let state = state().await;
        let provider = create_provider(&state, 1, json!({"name":"Named supplier","provider_type":"openai","base_url":"https://example.com/v1","api_key":"test"})).await.unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        let created = create_model(&state, 1, json!({"provider_id":provider_id,"alias":"priced","upstream_model":"upstream","input_price_per_million":2.5,"output_price_per_million":0})).await.unwrap();
        let id = created["id"].as_i64().unwrap();
        let listed = list_models(&state, 1).await.unwrap();
        assert_eq!(listed["list"][0]["provider_name"], "Named supplier");
        assert_eq!(listed["list"][0]["input_price_per_million"], 2.5);
        let updated = update_model(&state, 1, json!({"id":id,"alias":"renamed"}))
            .await
            .unwrap();
        assert_eq!(updated["input_price_per_million"], 2.5);
        assert_eq!(updated["output_price_per_million"], 0.0);
        assert!(
            update_model(&state, 2, json!({"id":id,"input_price_per_million":1}))
                .await
                .is_err()
        );
        for bad in [json!(-1), json!("abc"), json!(1e20)] {
            assert!(
                update_model(&state, 1, json!({"id":id,"input_price_per_million":bad}))
                    .await
                    .is_err()
            );
        }
        let cleared = update_model(&state, 1, json!({"id":id,"input_price_per_million":null}))
            .await
            .unwrap();
        assert!(cleared["input_price_per_million"].is_null());
        assert_eq!(cleared["output_price_per_million"], 0.0);
    }

    #[tokio::test]
    async fn provider_model_and_virtual_key_are_user_isolated() {
        let state = state().await;
        let provider = create_provider(
            &state,
            1,
            json!({
                "name":"private-openai",
                "provider_type":"openai",
                "base_url":"https://example.com/v1",
                "api_key":"upstream-secret"
            }),
        )
        .await
        .unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        create_model(
            &state,
            1,
            json!({"provider_id":provider_id,"alias":"private-model","upstream_model":"gpt-test"}),
        )
        .await
        .unwrap();
        let key = create_key(&state, 1, json!({"name":"client"}))
            .await
            .unwrap();
        let raw_key = key["key"].as_str().unwrap();

        assert_eq!(gateway_models(&state, 1).await.unwrap(), ["private-model"]);
        assert!(gateway_models(&state, 2).await.unwrap().is_empty());
        assert!(matches!(
            resolve_model(&state, 2, "private-model").await,
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            load_provider(&state, 2, provider_id).await,
            Err(AppError::NotFound(_))
        ));
        let resolved_key = gateway_key(&state, raw_key).await.unwrap().unwrap();
        assert_eq!(resolved_key.user_id, 1);

        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(gateway_key(&state, raw_key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn provider_secret_is_encrypted_at_rest_and_decrypts_for_owner() {
        let state = state().await;
        let provider = create_provider(
            &state,
            1,
            json!({"name":"encrypted","provider_type":"openai","base_url":"https://example.com/v1","api_key":"plain-secret"}),
        )
        .await
        .unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        let stored: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM user_providers WHERE id=?")
                .bind(provider_id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_ne!(stored, "plain-secret");
        assert!(!stored.contains("plain-secret"));
        assert_eq!(
            load_provider(&state, 1, provider_id).await.unwrap().api_key,
            "plain-secret"
        );
    }

    #[tokio::test]
    async fn deleting_provider_cascades_owned_model_routes() {
        let state = state().await;
        let provider = create_provider(
            &state,
            1,
            json!({"name":"cascade","provider_type":"openai","base_url":"https://example.com/v1","api_key":"secret"}),
        )
        .await
        .unwrap();
        let provider_id = provider["id"].as_i64().unwrap();
        create_model(
            &state,
            1,
            json!({"provider_id":provider_id,"alias":"cascade-model","upstream_model":"gpt-test"}),
        )
        .await
        .unwrap();

        delete_provider(&state, 1, provider_id).await.unwrap();

        let provider_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_providers WHERE id=?")
                .bind(provider_id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        let model_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_models WHERE provider_id=?")
                .bind(provider_id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!((provider_count, model_count), (0, 0));
    }
}
