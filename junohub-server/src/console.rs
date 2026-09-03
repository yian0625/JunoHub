use std::collections::HashSet;

use axum::{
    extract::{Extension, Path, State},
    http::HeaderMap,
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::{issue_user_token, CurrentUser},
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
    #[serde(default)]
    pub display_name: String,
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
    let password_hash = secrets::hash_password(password)?;
    let timestamp = store::now();
    sqlx::query(
        "INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) \
         VALUES(?,?,'JunoHub','admin',1,?,?) \
         ON CONFLICT(email) DO UPDATE SET role='admin',status=1,updated_at=excluded.updated_at",
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
    Json(input): Json<Credentials>,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let email = normalize_email(&input.email)?;
    validate_password(&input.password)?;
    let display_name = if input.display_name.trim().is_empty() {
        email.split('@').next().unwrap_or("user").to_owned()
    } else {
        input.display_name.trim().to_owned()
    };
    let timestamp = store::now();
    let password_hash = secrets::hash_password(&input.password)?;
    let result = sqlx::query("INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) VALUES(?,?,?,'user',1,?,?)")
        .bind(&email)
        .bind(password_hash)
        .bind(&display_name)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&state.pool)
        .await
        .map_err(|error| {
            if error.to_string().contains("UNIQUE") {
                AppError::BadRequest("该邮箱已注册".into())
            } else {
                AppError::Sql(error)
            }
        })?;
    login_response(
        &state,
        result.last_insert_rowid(),
        email,
        display_name,
        String::new(),
        "user".into(),
    )
}

pub async fn login(
    State(state): State<AppState>,
    Json(input): Json<Credentials>,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let email = normalize_email(&input.email)?;
    let row = sqlx::query(
        "SELECT id,password_hash,display_name,avatar_url,role,status FROM users WHERE email=?",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Unauthorized)?;
    if row.get::<i64, _>("status") != 1
        || !secrets::verify_password(&input.password, row.get("password_hash"))
    {
        return Err(AppError::Unauthorized);
    }
    login_response(
        &state,
        row.get("id"),
        email,
        row.get("display_name"),
        row.get("avatar_url"),
        row.get("role"),
    )
}

fn login_response(
    state: &AppState,
    uid: i64,
    email: String,
    display_name: String,
    avatar_url: String,
    role: String,
) -> AppResult<(HeaderMap, Json<Envelope<Value>>)> {
    let token = issue_user_token(state, uid, email.clone(), role.clone())?;
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
    let data = match path {
        "profile" => profile(&state, user.uid).await?,
        "provider/list" => list_providers(&state, user.uid).await?,
        "provider/create" => create_provider(&state, user.uid, payload).await?,
        "provider/update" => update_provider(&state, user.uid, payload).await?,
        "provider/delete" => delete_provider(&state, user.uid, id(&payload)?).await?,
        "provider/test" => test_provider(&state, user.uid, payload).await?,
        "provider/models" => provider_models(&state, user.uid, id(&payload)?).await?,
        "model/list" => list_models(&state, user.uid).await?,
        "model/create" => create_model(&state, user.uid, payload).await?,
        "model/import" => import_models(&state, user.uid, payload).await?,
        "model/update" => update_model(&state, user.uid, payload).await?,
        "model/delete" => delete_model(&state, user.uid, id(&payload)?).await?,
        "key/list" => list_keys(&state, user.uid).await?,
        "key/create" => create_key(&state, user.uid, payload).await?,
        "key/update" => update_key(&state, user.uid, payload).await?,
        "key/delete" => delete_key(&state, user.uid, id(&payload)?).await?,
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
        _ => {
            return Err(AppError::NotFound(format!(
                "用户接口不存在: /console/{path}"
            )))
        }
    };
    Ok(Json(Envelope::ok(data)))
}

fn normalize_email(value: &str) -> AppResult<String> {
    let email = value.trim().to_lowercase();
    if email.len() < 5 || !email.contains('@') || email.len() > 254 {
        return Err(AppError::BadRequest("请输入有效邮箱".into()));
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
        validate_avatar(value)?;
        value.to_owned()
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
    if !secrets::verify_password(&current_password, &password_hash) {
        return Err(AppError::BadRequest("当前密码不正确".into()));
    }
    if current_password == new_password {
        return Err(AppError::BadRequest("新密码不能与当前密码相同".into()));
    }
    let new_hash = secrets::hash_password(&new_password)?;
    sqlx::query("UPDATE users SET password_hash=?,updated_at=? WHERE id=?")
        .bind(new_hash)
        .bind(store::now())
        .bind(uid)
        .execute(&state.pool)
        .await?;
    Ok(json!({}))
}

fn validate_avatar(value: &str) -> AppResult<()> {
    if value.is_empty() {
        return Ok(());
    }
    let encoded = [
        "data:image/webp;base64,",
        "data:image/png;base64,",
        "data:image/jpeg;base64,",
    ]
    .iter()
    .find_map(|prefix| value.strip_prefix(prefix))
    .ok_or_else(|| AppError::BadRequest("头像必须是 WebP、PNG 或 JPEG 图片".into()))?;
    if encoded.len() > 350_000 {
        return Err(AppError::BadRequest("头像不能超过 256KB".into()));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| AppError::BadRequest("头像数据无效".into()))?;
    if bytes.is_empty() || bytes.len() > 256 * 1024 {
        return Err(AppError::BadRequest("头像不能超过 256KB".into()));
    }
    Ok(())
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

fn validate_base_url(value: &str) -> AppResult<String> {
    let url =
        reqwest::Url::parse(value).map_err(|_| AppError::BadRequest("Base URL 无效".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
    {
        return Err(AppError::BadRequest(
            "Base URL 必须是没有内嵌凭据的 HTTP(S) 地址".into(),
        ));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

async fn list_providers(state: &AppState, uid: i64) -> AppResult<Value> {
    let rows = sqlx::query("SELECT id,name,provider_type,base_url,encrypted_api_key,enabled,created_at,updated_at FROM user_providers WHERE user_id=? ORDER BY id DESC")
        .bind(uid).fetch_all(&state.pool).await?;
    let list = rows.into_iter().map(|row| json!({
        "id":row.get::<i64,_>("id"), "name":row.get::<String,_>("name"),
        "provider_type":row.get::<String,_>("provider_type"), "base_url":row.get::<String,_>("base_url"),
        "has_api_key":!row.get::<String,_>("encrypted_api_key").is_empty(), "enabled":row.get::<i64,_>("enabled"),
        "create_time":row.get::<i64,_>("created_at"), "update_time":row.get::<i64,_>("updated_at")
    })).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

async fn create_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let name = string(&payload, "name");
    let provider_type = string(&payload, "provider_type");
    let base_url = validate_base_url(&string(&payload, "base_url"))?;
    let api_key = string(&payload, "api_key");
    if name.is_empty() || provider_type.is_empty() {
        return Err(AppError::BadRequest("名称和供应商类型不能为空".into()));
    }
    let encrypted = if api_key.is_empty() {
        String::new()
    } else {
        secrets::encrypt(&state.config.encryption_key, &api_key)?
    };
    let timestamp = store::now();
    let result = sqlx::query("INSERT INTO user_providers(user_id,name,provider_type,base_url,encrypted_api_key,enabled,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?)")
        .bind(uid).bind(&name).bind(&provider_type).bind(&base_url).bind(encrypted).bind(timestamp).bind(timestamp)
        .execute(&state.pool).await.map_err(map_unique("供应商名称已存在"))?;
    Ok(
        json!({"id":result.last_insert_rowid(),"name":name,"provider_type":provider_type,"base_url":base_url,"has_api_key":true,"enabled":1,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = id(&payload)?;
    let row = sqlx::query("SELECT name,provider_type,base_url,encrypted_api_key,enabled,created_at FROM user_providers WHERE id=? AND user_id=?")
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
    let timestamp = store::now();
    sqlx::query("UPDATE user_providers SET name=?,provider_type=?,base_url=?,encrypted_api_key=?,enabled=?,updated_at=? WHERE id=? AND user_id=?")
        .bind(&name).bind(&provider_type).bind(&base_url).bind(encrypted).bind(enabled).bind(timestamp).bind(provider_id).bind(uid)
        .execute(&state.pool).await.map_err(map_unique("供应商名称已存在"))?;
    Ok(
        json!({"id":provider_id,"name":name,"provider_type":provider_type,"base_url":base_url,"has_api_key":true,"enabled":enabled,"create_time":row.get::<i64,_>("created_at"),"update_time":timestamp}),
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
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_models WHERE user_id=? AND provider_id=?")
            .bind(uid)
            .bind(provider_id)
            .fetch_one(&state.pool)
            .await?;
    if count > 0 {
        return Err(AppError::BadRequest("请先删除该供应商下的模型".into()));
    }
    let result = sqlx::query("DELETE FROM user_providers WHERE id=? AND user_id=?")
        .bind(provider_id)
        .bind(uid)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("供应商不存在".into()));
    }
    Ok(json!({}))
}

async fn provider_models(state: &AppState, uid: i64, provider_id: i64) -> AppResult<Value> {
    let provider = load_provider(state, uid, provider_id).await?;
    let mut models = if provider.provider_type == "sub2api" {
        Sub2Repository::new(state.sub2_pool.clone())
            .model_ids_for_api_key(&provider.api_key)
            .await?
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

async fn upstream_models(
    state: &AppState,
    provider: &ProviderSecret,
) -> AppResult<Vec<String>> {
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    let mut request = state.client.get(url);
    if !provider.api_key.is_empty() {
        request = request.bearer_auth(&provider.api_key);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::BadRequest(format!(
            "无法读取上游模型，HTTP {}",
            status.as_u16()
        )));
    }
    let body: Value = response.json().await?;
    Ok(body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .map(str::to_owned)
        .collect())
}

async fn test_provider(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = id(&payload)?;
    let provider = load_provider(state, uid, provider_id).await?;
    let ok = if provider.provider_type == "sub2api" {
        !Sub2Repository::new(state.sub2_pool.clone())
            .model_ids_for_api_key(&provider.api_key)
            .await?
            .is_empty()
    } else {
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let mut request = state.client.get(url);
        if !provider.api_key.is_empty() {
            request = request.bearer_auth(provider.api_key);
        }
        request
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    };
    Ok(json!({"ok":ok,"status_code":if ok {200} else {502}}))
}

pub struct ProviderSecret {
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
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
    let rows=sqlx::query("SELECT m.id,m.provider_id,m.alias,m.upstream_model,m.capabilities,m.enabled,m.created_at,m.updated_at,p.name provider_name,p.provider_type FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id WHERE m.user_id=? ORDER BY m.id DESC").bind(uid).fetch_all(&state.pool).await?;
    let list=rows.into_iter().map(|row|json!({"id":row.get::<i64,_>("id"),"provider_id":row.get::<i64,_>("provider_id"),"provider_name":row.get::<String,_>("provider_name"),"provider_type":row.get::<String,_>("provider_type"),"alias":row.get::<String,_>("alias"),"upstream_model":row.get::<String,_>("upstream_model"),"capabilities":serde_json::from_str::<Value>(row.get("capabilities")).unwrap_or(json!(["chat"])),"enabled":row.get::<i64,_>("enabled"),"create_time":row.get::<i64,_>("created_at"),"update_time":row.get::<i64,_>("updated_at")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

pub(crate) async fn provision_sub2api_provider(
    state: &AppState,
    uid: i64,
    provider_name: &str,
    api_key: &str,
    models: &[String],
) -> AppResult<Value> {
    if models.is_empty() {
        return Err(AppError::BadRequest("Sub2API 分组没有可用模型".into()));
    }
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
    let mut transaction = state.pool.begin().await?;
    let provider = sqlx::query("INSERT INTO user_providers(user_id,name,provider_type,base_url,encrypted_api_key,enabled,created_at,updated_at) VALUES(?,?, 'sub2api','http://junohub-server:9003/v1',?,1,?,?)")
        .bind(uid)
        .bind(name)
        .bind(encrypted)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await
        .map_err(map_unique("供应商名称已存在"))?;
    let provider_id = provider.last_insert_rowid();
    let mut existing =
        sqlx::query_scalar::<_, String>("SELECT alias FROM user_models WHERE user_id=?")
            .bind(uid)
            .fetch_all(&mut *transaction)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
    let mut imported = 0;
    let mut skipped = 0;
    for model in models {
        let model = model.trim();
        if model.is_empty() || model.len() > 200 || !existing.insert(model.to_owned()) {
            skipped += 1;
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
        imported += 1;
    }
    transaction.commit().await?;
    Ok(json!({"provider_id":provider_id,"imported_count":imported,"skipped_count":skipped}))
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

async fn create_model(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    load_provider(state, uid, provider_id).await?;
    let alias = string(&payload, "alias");
    let upstream = string(&payload, "upstream_model");
    if alias.is_empty() || upstream.is_empty() {
        return Err(AppError::BadRequest("模型别名和上游模型不能为空".into()));
    }
    let capabilities = payload
        .get("capabilities")
        .cloned()
        .unwrap_or(json!(["chat"]));
    let timestamp = store::now();
    let result=sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?)").bind(uid).bind(provider_id).bind(&alias).bind(&upstream).bind(capabilities.to_string()).bind(timestamp).bind(timestamp).execute(&state.pool).await.map_err(map_unique("模型别名已存在"))?;
    Ok(
        json!({"id":result.last_insert_rowid(),"provider_id":provider_id,"alias":alias,"upstream_model":upstream,"capabilities":capabilities,"enabled":1,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_model(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let model_id = id(&payload)?;
    let row=sqlx::query("SELECT provider_id,alias,upstream_model,capabilities,enabled,created_at FROM user_models WHERE id=? AND user_id=?").bind(model_id).bind(uid).fetch_optional(&state.pool).await?.ok_or_else(||AppError::NotFound("模型不存在".into()))?;
    let provider_id = payload
        .get("provider_id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("provider_id"));
    load_provider(state, uid, provider_id).await?;
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
    let timestamp = store::now();
    sqlx::query("UPDATE user_models SET provider_id=?,alias=?,upstream_model=?,capabilities=?,enabled=?,updated_at=? WHERE id=? AND user_id=?").bind(provider_id).bind(&alias).bind(&upstream).bind(capabilities.to_string()).bind(enabled).bind(timestamp).bind(model_id).bind(uid).execute(&state.pool).await.map_err(map_unique("模型别名已存在"))?;
    Ok(
        json!({"id":model_id,"provider_id":provider_id,"alias":alias,"upstream_model":upstream,"capabilities":capabilities,"enabled":enabled,"create_time":row.get::<i64,_>("created_at"),"update_time":timestamp}),
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

pub struct UserModelRoute {
    pub alias: String,
    pub upstream_model: String,
    pub provider_id: i64,
}
pub async fn resolve_model(state: &AppState, uid: i64, alias: &str) -> AppResult<UserModelRoute> {
    let row=sqlx::query("SELECT alias,upstream_model,provider_id FROM user_models WHERE user_id=? AND alias=? AND enabled=1").bind(uid).bind(alias).fetch_optional(&state.pool).await?.ok_or_else(||AppError::NotFound(format!("模型 {alias} 不存在")))?;
    Ok(UserModelRoute {
        alias: row.get("alias"),
        upstream_model: row.get("upstream_model"),
        provider_id: row.get("provider_id"),
    })
}
pub async fn gateway_models(state: &AppState, uid: i64) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT alias FROM user_models WHERE user_id=? AND enabled=1 ORDER BY alias",
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?)
}

async fn list_keys(state: &AppState, uid: i64) -> AppResult<Value> {
    let rows=sqlx::query("SELECT id,name,key_prefix,allowed_models,status,expires_at,created_at,updated_at,last_used_at,total_requests FROM virtual_keys WHERE user_id=? ORDER BY id DESC").bind(uid).fetch_all(&state.pool).await?;
    let list=rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"name":r.get::<String,_>("name"),"key_prefix":r.get::<String,_>("key_prefix"),"key_masked":format!("{}••••••••",r.get::<String,_>("key_prefix")),"allowed_models":serde_json::from_str::<Value>(r.get("allowed_models")).unwrap_or(json!([])),"status":r.get::<i64,_>("status"),"expires_at":r.get::<i64,_>("expires_at"),"create_time":r.get::<i64,_>("created_at"),"update_time":r.get::<i64,_>("updated_at"),"last_used_time":r.get::<i64,_>("last_used_at"),"total_requests":r.get::<i64,_>("total_requests")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

async fn create_key(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let name = string(&payload, "name");
    if name.is_empty() {
        return Err(AppError::BadRequest("Key 名称不能为空".into()));
    }
    let raw = format!("sk-juno-{}", Uuid::new_v4().simple());
    let prefix = raw.chars().take(16).collect::<String>();
    let allowed = payload.get("allowed_models").cloned().unwrap_or(json!([]));
    let expires = payload
        .get("expires_at")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let timestamp = store::now();
    let result=sqlx::query("INSERT INTO virtual_keys(user_id,name,key_prefix,key_hash,allowed_models,status,expires_at,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?,?)").bind(uid).bind(&name).bind(&prefix).bind(store::hash_key(&raw)).bind(allowed.to_string()).bind(expires).bind(timestamp).bind(timestamp).execute(&state.pool).await?;
    Ok(
        json!({"id":result.last_insert_rowid(),"name":name,"key":raw,"key_prefix":prefix,"key_masked":format!("{}••••••••",prefix),"allowed_models":allowed,"status":1,"expires_at":expires,"create_time":timestamp,"update_time":timestamp}),
    )
}

async fn update_key(state: &AppState, uid: i64, payload: Value) -> AppResult<Value> {
    let key_id = id(&payload)?;
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
        .unwrap_or_else(|| serde_json::from_str(row.get("allowed_models")).unwrap_or(json!([])));
    let status = payload
        .get("status")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("status"));
    let expires = payload
        .get("expires_at")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.get("expires_at"));
    sqlx::query("UPDATE virtual_keys SET name=?,allowed_models=?,status=?,expires_at=?,updated_at=? WHERE id=? AND user_id=?").bind(&name).bind(allowed.to_string()).bind(status).bind(expires).bind(store::now()).bind(key_id).bind(uid).execute(&state.pool).await?;
    Ok(
        json!({"id":key_id,"name":name,"allowed_models":allowed,"status":status,"expires_at":expires}),
    )
}
async fn delete_key(state: &AppState, uid: i64, key_id: i64) -> AppResult<Value> {
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
pub async fn gateway_key(state: &AppState, raw: &str) -> AppResult<Option<GatewayKey>> {
    let row = sqlx::query(
        "SELECT k.id,k.user_id,k.allowed_models,k.expires_at \
         FROM virtual_keys k JOIN users u ON u.id=k.user_id \
         WHERE k.key_hash=? AND k.status=1 AND u.status=1",
    )
    .bind(store::hash_key(raw))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map(|r| GatewayKey {
        id: r.get("id"),
        user_id: r.get("user_id"),
        allowed_models: serde_json::from_str(r.get("allowed_models")).unwrap_or_default(),
        expires_at: r.get("expires_at"),
    }))
}

async fn list_logs(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let page = payload
        .get("page")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .max(1);
    let size = payload
        .get("page_size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 100);
    let rows=sqlx::query("SELECT id,virtual_key_id,model_alias,upstream_model,provider_type,status_code,duration_ms,error_message,created_at FROM user_request_logs WHERE user_id=? ORDER BY id DESC LIMIT ? OFFSET ?").bind(uid).bind(size).bind((page-1)*size).fetch_all(&state.pool).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_request_logs WHERE user_id=?")
        .bind(uid)
        .fetch_one(&state.pool)
        .await?;
    let list=rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"api_key_id":r.get::<i64,_>("virtual_key_id"),"model_alias":r.get::<String,_>("model_alias"),"upstream_model":r.get::<String,_>("upstream_model"),"provider_type":r.get::<String,_>("provider_type"),"status_code":r.get::<i64,_>("status_code"),"duration_ms":r.get::<i64,_>("duration_ms"),"error_message":r.get::<String,_>("error_message"),"create_time":r.get::<i64,_>("created_at")})).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":total,"page":page,"page_size":size}))
}
async fn stats(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
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
    let requests: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_request_logs WHERE user_id=? AND created_at>=?",
    )
    .bind(uid)
    .bind(since)
    .fetch_one(&state.pool)
    .await?;
    Ok(json!({"providers":providers,"models":models,"keys":keys,"requests":requests}))
}

pub async fn admin_users(state: &AppState, payload: &Value) -> AppResult<Value> {
    let page = payload
        .get("page")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .max(1);
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
pub async fn admin_update_user(state: &AppState, payload: &Value) -> AppResult<Value> {
    let user_id = id(payload)?;
    let current = sqlx::query("SELECT role,status FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&state.pool)
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
    sqlx::query("UPDATE users SET role=?,status=?,updated_at=? WHERE id=?")
        .bind(role)
        .bind(status)
        .bind(store::now())
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    Ok(json!({"id":user_id,"role":role,"status":status}))
}

pub async fn record_request(
    state: &AppState,
    key: &GatewayKey,
    model: &UserModelRoute,
    provider_type: &str,
    status: u16,
    duration: i64,
    error: &str,
) {
    let _=sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,upstream_model,provider_type,status_code,duration_ms,error_message,created_at) VALUES(?,?,?,?,?,?,?,?,?)").bind(key.user_id).bind(key.id).bind(&model.alias).bind(&model.upstream_model).bind(provider_type).bind(status as i64).bind(duration).bind(error).bind(store::now()).execute(&state.pool).await;
    let _ = sqlx::query(
        "UPDATE virtual_keys SET total_requests=total_requests+1,last_used_at=? WHERE id=?",
    )
    .bind(store::now())
    .bind(key.id)
    .execute(&state.pool)
    .await;
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use reqwest::Client;
    use serde_json::json;
    use sqlx::{postgres::PgPoolOptions, sqlite::SqlitePoolOptions};

    use super::*;
    use crate::config::Config;

    async fn state() -> AppState {
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
            }),
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .unwrap(),
        }
    }

    #[test]
    fn avatar_validation_accepts_supported_small_images() {
        let encoded = STANDARD.encode([1_u8, 2, 3, 4]);
        assert!(validate_avatar(&format!("data:image/webp;base64,{encoded}")).is_ok());
        assert!(validate_avatar("").is_ok());
        assert!(validate_avatar("data:image/svg+xml;base64,PHN2Zz4=").is_err());
        assert!(validate_avatar("data:image/png;base64,not-base64").is_err());
    }

    #[tokio::test]
    async fn account_profile_and_password_updates_are_persisted() {
        let state = state().await;
        let current_hash = secrets::hash_password("current-password").unwrap();
        sqlx::query("UPDATE users SET password_hash=? WHERE id=1")
            .bind(current_hash)
            .execute(&state.pool)
            .await
            .unwrap();

        let avatar = format!("data:image/webp;base64,{}", STANDARD.encode([1_u8, 2, 3]));
        let updated = update_account_profile(
            &state,
            1,
            &json!({"display_name":"New Name","avatar_url":avatar}),
        )
        .await
        .unwrap();
        assert_eq!(updated["display_name"], "New Name");
        assert_eq!(updated["avatar_url"], avatar);

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
    async fn sub2api_provision_creates_one_provider_and_imports_models() {
        let state = state().await;
        let result = provision_sub2api_provider(
            &state,
            1,
            "Sub2API · OpenAI",
            "sub2-secret-key",
            &["gpt-a".into(), "gpt-b".into()],
        )
        .await
        .unwrap();
        assert_eq!(result["imported_count"], 2);
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
        assert!(provision_sub2api_provider(
            &state,
            1,
            "Sub2API · OpenAI",
            "another-key",
            &["gpt-c".into()]
        )
        .await
        .is_ok());
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
}
