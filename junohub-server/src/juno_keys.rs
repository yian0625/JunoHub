use axum::{
    extract::State,
    http::{header, HeaderMap},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    console::{self, GatewayKey},
    error::{AppError, AppResult},
    models::Envelope,
    secrets,
    state::AppState,
    store,
};

#[derive(Deserialize)]
pub struct EnsureJunoKeyRequest {
    juno_uid: i64,
    #[serde(default)]
    display_name: String,
}

pub async fn ensure_juno_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EnsureJunoKeyRequest>,
) -> AppResult<Json<Envelope<Value>>> {
    let master = authenticate_provisioner(&state, &headers).await?;
    let (key_id, key) =
        ensure_key(&state.pool, &state.config.encryption_key, &master, &request).await?;
    Ok(Json(Envelope::ok(json!({ "key_id": key_id, "key": key }))))
}

pub async fn juno_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Envelope<Value>>> {
    let master = authenticate_provisioner(&state, &headers).await?;
    let aliases = provisioned_aliases(&state.pool, &master).await?;
    let models = aliases
        .into_iter()
        .map(|alias| json!({ "model_id": alias, "display_name": alias }))
        .collect::<Vec<_>>();
    Ok(Json(Envelope::ok(json!({ "models": models }))))
}


#[derive(Deserialize)]
pub struct JunoUsageRequest {
    juno_uid: i64,
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_page_size")]
    page_size: i64,
}
fn default_page() -> i64 { 1 }
fn default_page_size() -> i64 { 20 }

pub async fn juno_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<JunoUsageRequest>,
) -> AppResult<Json<Envelope<Value>>> {
    let master = authenticate_provisioner(&state, &headers).await?;
    Ok(Json(Envelope::ok(usage_for_user(&state.pool, master.user_id, &request).await?)))
}

async fn usage_for_user(pool: &SqlitePool, owner_id: i64, request: &JunoUsageRequest) -> AppResult<Value> {
    if request.juno_uid <= 0 { return Err(AppError::BadRequest("Juno UID 无效".into())); }
    let page = request.page.clamp(1, 1_000_000);
    let page_size = request.page_size.clamp(1, 100);
    let external_ref = format!("juno:{}", request.juno_uid);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_request_logs l JOIN virtual_keys k ON k.id=l.virtual_key_id AND k.user_id=l.user_id WHERE l.user_id=? AND k.external_ref=?"
    ).bind(owner_id).bind(&external_ref).fetch_one(pool).await?;
    let rows = sqlx::query(
        "SELECT l.id,l.request_id,l.model_alias,l.upstream_model,l.input_tokens,l.output_tokens,l.total_tokens,l.cost_usd,l.status_code,l.duration_ms,l.error_message,l.created_at FROM user_request_logs l JOIN virtual_keys k ON k.id=l.virtual_key_id AND k.user_id=l.user_id WHERE l.user_id=? AND k.external_ref=? ORDER BY l.id DESC LIMIT ? OFFSET ?"
    ).bind(owner_id).bind(&external_ref).bind(page_size).bind((page-1)*page_size).fetch_all(pool).await?;
    let list: Vec<Value> = rows.into_iter().map(|r| json!({
        "id":r.get::<i64,_>("id"),"request_id":r.get::<String,_>("request_id"),"model_alias":r.get::<String,_>("model_alias"),
        "upstream_model":r.get::<String,_>("upstream_model"),
        "input_tokens":r.get::<i64,_>("input_tokens"),"output_tokens":r.get::<i64,_>("output_tokens"),
        "cost_usd":r.get::<Option<f64>,_>("cost_usd"),"status_code":r.get::<i64,_>("status_code"),
        "duration_ms":r.get::<i64,_>("duration_ms"),"error_message":r.get::<String,_>("error_message"),
        "create_time":r.get::<i64,_>("created_at")
    })).collect();
    Ok(json!({"list":list,"total":total,"page":page,"page_size":page_size,"supports_request_id":true}))
}

async fn authenticate_provisioner(state: &AppState, headers: &HeaderMap) -> AppResult<GatewayKey> {
    let provisioner_id = std::env::var("JUNOHUB_JUNO_PROVISIONER_KEY_ID")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or(AppError::Unauthorized)?;
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::bearer_token)
        .ok_or(AppError::Unauthorized)?;
    console::gateway_key(state, raw)
        .await?
        .filter(|key| key.id == provisioner_id)
        .filter(|key| key.expires_at == 0 || key.expires_at > store::now())
        .ok_or(AppError::Unauthorized)
}

async fn junohub_model_aliases(pool: &SqlitePool, owner_id: i64) -> AppResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT m.alias FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id \
         WHERE m.user_id=? AND m.enabled=1 AND p.enabled=1 ORDER BY m.alias",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| row.get("alias")).collect())
}

async fn provisioned_aliases(pool: &SqlitePool, master: &GatewayKey) -> AppResult<Vec<String>> {
    Ok(junohub_model_aliases(pool,master.user_id).await?.into_iter()
        .filter(|alias| crate::proxy::allows_model(master,alias)).collect())
}

async fn ensure_key(
    pool: &SqlitePool,
    encryption_key: &str,
    master: &GatewayKey,
    request: &EnsureJunoKeyRequest,
) -> AppResult<(i64, String)> {
    if request.juno_uid <= 0 {
        return Err(AppError::BadRequest("Juno UID 无效".into()));
    }
    let display_name = request
        .display_name
        .trim()
        .chars()
        .take(80)
        .collect::<String>();
    let name = if display_name.is_empty() {
        format!("Juno 用户 {}", request.juno_uid)
    } else {
        format!("Juno {} · {}", request.juno_uid, display_name)
    };
    let external_ref = format!("juno:{}", request.juno_uid);
    let aliases = provisioned_aliases(pool, master).await?;
    if aliases.is_empty() {
        return Err(AppError::BadRequest("JunoHub 中没有启用的模型".into()));
    }
    let allowed_models = serde_json::to_string(&aliases)?;
    let raw = format!("sk-juno-{}", Uuid::new_v4().simple());
    let encrypted = secrets::encrypt(encryption_key, &raw)?;
    let timestamp = store::now();
    sqlx::query(
        "INSERT OR IGNORE INTO virtual_keys(user_id,name,key_prefix,key_hash,allowed_models,status,expires_at,created_at,updated_at,encrypted_key,external_ref) \
         VALUES(?,?,?,?,?,1,?,?,?,?,?)",
    )
    .bind(master.user_id)
    .bind(&name)
    .bind(raw.chars().take(16).collect::<String>())
    .bind(store::hash_key(&raw))
    .bind(&allowed_models)
    .bind(master.expires_at)
    .bind(timestamp)
    .bind(timestamp)
    .bind(encrypted)
    .bind(&external_ref)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT id,status,encrypted_key FROM virtual_keys WHERE user_id=? AND external_ref=?",
    )
    .bind(master.user_id)
    .bind(&external_ref)
    .fetch_one(pool)
    .await?;
    if row.get::<i64, _>("status") != 1 {
        return Err(AppError::Unauthorized);
    }
    let key_id: i64 = row.get("id");
    let encrypted_key: Option<String> = row.get("encrypted_key");
    let encrypted_key = encrypted_key
        .ok_or_else(|| AppError::Internal("Juno 用户 Key 无法读取，请在 JunoHub 中检查".into()))?;
    sqlx::query(
        "UPDATE virtual_keys SET name=?,allowed_models=?,expires_at=?,updated_at=? WHERE id=? AND user_id=?",
    )
    .bind(name)
    .bind(allowed_models)
    .bind(master.expires_at)
    .bind(timestamp)
    .bind(key_id)
    .bind(master.user_id)
    .execute(pool)
    .await?;
    Ok((key_id, secrets::decrypt(encryption_key, &encrypted_key)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn usage_is_scoped_to_owner_and_exact_juno_uid() {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query("INSERT INTO virtual_keys(id,user_id,name,key_prefix,key_hash,external_ref,created_at,updated_at) VALUES(1,3,'one','one','one','juno:9007199254740993',0,0),(2,4,'two','two','two','juno:9007199254740993',0,0),(3,3,'three','three','three','juno:9007199254740994',0,0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_request_logs(user_id,virtual_key_id,model_alias,created_at,input_tokens,output_tokens,cost_usd,status_code) VALUES(3,1,'mine-old',1,12,30,0.02,200),(3,1,'mine-new',2,13,31,NULL,500),(4,2,'other-owner',3,0,0,1,200),(3,3,'other-juno-user',4,0,0,1,200)")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE user_request_logs SET request_id='59c3d76b-d779-43c2-9da4-b3bc5d9c2e5c' WHERE model_alias='mine-old'").execute(&pool).await.unwrap();
        let mut req = JunoUsageRequest { juno_uid: 9007199254740993, page: 1, page_size: 1 };
        let first = usage_for_user(&pool, 3, &req).await.unwrap();
        assert_eq!(first["total"], 2);
        assert_eq!(first["supports_request_id"], true);
        assert_eq!(first["list"][0]["model_alias"], "mine-new");
        assert!(first["list"][0]["cost_usd"].is_null());
        req.page = 2;
        let second = usage_for_user(&pool, 3, &req).await.unwrap();
        assert_eq!(second["list"][0]["request_id"], "59c3d76b-d779-43c2-9da4-b3bc5d9c2e5c");
        assert_eq!(second["list"][0]["input_tokens"], 12);
        assert_eq!(second["list"][0]["cost_usd"], 0.02);
        req.juno_uid = 123;
        assert_eq!(usage_for_user(&pool, 3, &req).await.unwrap()["total"], 0);
        req.juno_uid = 0;
        assert!(usage_for_user(&pool, 3, &req).await.is_err());
    }

    #[tokio::test]
    async fn creates_one_key_per_juno_uid_and_updates_display_name() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_providers(id,user_id,name,provider_type,base_url,encrypted_api_key,created_at,updated_at) VALUES(10,3,'TokenHub','openai-compatible','https://example.com','test',0,0),(11,3,'OpenAI','sub2api','https://example.com','test',0,0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,created_at,updated_at) VALUES(3,10,'tokenhub/claude-opus-4-8','claude-opus-4-8',0,0),(3,11,'gpt-6-sol','gpt-6-sol',0,0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,enabled,created_at,updated_at) VALUES(3,10,'disabled-model','disabled-model',0,0,0)")
            .execute(&pool).await.unwrap();
        let mut master = GatewayKey {
            id: 20,
            user_id: 3,
            allowed_models: vec!["tokenhub/claude-opus-4-8".into()],
            expires_at: 0,
        };
        let first = ensure_key(
            &pool,
            "test-encryption-key",
            &master,
            &EnsureJunoKeyRequest {
                juno_uid: 42,
                display_name: "Alice".into(),
            },
        )
        .await
        .unwrap();
        let second = ensure_key(
            &pool,
            "test-encryption-key",
            &master,
            &EnsureJunoKeyRequest {
                juno_uid: 42,
                display_name: "Alice 新名字".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        let other = ensure_key(
            &pool,
            "test-encryption-key",
            &master,
            &EnsureJunoKeyRequest {
                juno_uid: 43,
                display_name: "Alice".into(),
            },
        )
        .await
        .unwrap();
        assert_ne!(first.0, other.0);
        assert_ne!(first.1, other.1);
        let row = sqlx::query("SELECT name,allowed_models FROM virtual_keys WHERE id=?")
            .bind(first.0)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("name"), "Juno 42 · Alice 新名字");
        assert_eq!(
            row.get::<String, _>("allowed_models"),
            r#"["tokenhub/claude-opus-4-8"]"#
        );
        assert_eq!(
            junohub_model_aliases(&pool, 3).await.unwrap(),
            vec!["gpt-6-sol", "tokenhub/claude-opus-4-8"]
        );
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,created_at,updated_at) VALUES(3,10,'tokenhub/new-model','new-model',0,0)")
            .execute(&pool).await.unwrap();
        master.allowed_models = vec!["tokenhub/*".into()];
        let refreshed = ensure_key(
            &pool,
            "test-encryption-key",
            &master,
            &EnsureJunoKeyRequest {
                juno_uid: 42,
                display_name: "Alice 新名字".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(refreshed, first);
        let scopes: String =
            sqlx::query_scalar("SELECT allowed_models FROM virtual_keys WHERE id=?")
                .bind(first.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            scopes,
            r#"["tokenhub/claude-opus-4-8","tokenhub/new-model"]"#
        );
        master.allowed_models = vec!["missing-model".into()];
        assert!(ensure_key(&pool,"test-encryption-key",&master,&EnsureJunoKeyRequest {juno_uid:44,display_name:String::new()}).await.is_err());
        master.allowed_models.clear();
        ensure_key(&pool,"test-encryption-key",&master,&EnsureJunoKeyRequest {juno_uid:42,display_name:String::new()}).await.unwrap();
        let scopes: String=sqlx::query_scalar("SELECT allowed_models FROM virtual_keys WHERE id=?").bind(first.0).fetch_one(&pool).await.unwrap();
        assert_eq!(scopes,r#"["gpt-6-sol","tokenhub/claude-opus-4-8","tokenhub/new-model"]"#);
    }
}
