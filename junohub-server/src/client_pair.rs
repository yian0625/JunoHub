use axum::{extract::State, Json};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    models::Envelope,
    secrets,
    state::AppState,
    store,
};

// Repair only the legacy JunoHub Pi rows whose two fields were reversed by
// the unlabelled editor. A real gateway alias must confirm the intended ID.
fn repair_reversed_pi_models(settings: &mut Value, aliases: &[String]) -> Vec<(String, String)> {
    let mut repaired = Vec::new();
    if let Some(models) = settings.get_mut("models").and_then(Value::as_array_mut) {
        for model in models {
            let id = model
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !aliases.contains(&id)
                && aliases.contains(&name)
                && id.contains(' ')
                && id.replace(' ', "-") == name
            {
                model["id"] = json!(name);
                model["name"] = json!(id);
                repaired.push((id, name));
            }
        }
    }
    repaired
}

pub async fn repair_legacy_junohub_pi_models(state: &AppState) -> AppResult<usize> {
    let rows = sqlx::query("SELECT id,user_id,encrypted_settings,models,metadata FROM integration_profiles WHERE encrypted_settings IS NOT NULL AND json_extract(metadata,'$.nativeClient')='pi' AND json_extract(metadata,'$.presetId')='junohub'")
        .fetch_all(&state.pool).await?;
    let mut count = 0;
    for row in rows {
        let uid: i64 = row.get("user_id");
        let aliases: Vec<String> =
            sqlx::query_scalar("SELECT alias FROM user_models WHERE user_id=? AND enabled=1")
                .bind(uid)
                .fetch_all(&state.pool)
                .await?;
        let encrypted: String = row.get("encrypted_settings");
        let mut settings: Value =
            serde_json::from_str(&secrets::decrypt(&state.config.encryption_key, &encrypted)?)?;
        let repaired = repair_reversed_pi_models(&mut settings, &aliases);
        if repaired.is_empty() {
            continue;
        }
        let mut models: Value = serde_json::from_str(row.get::<&str, _>("models"))?;
        let mut metadata: Value = serde_json::from_str(row.get::<&str, _>("metadata"))?;
        for (old, new) in &repaired {
            if models["pi"]["model"].as_str() == Some(old) {
                models["pi"]["model"] = json!(new);
            }
        }
        metadata["legacyModelFieldRepair"] = json!(repaired);
        // Optimistic condition preserves concurrent edits; rerunning is a no-op.
        let result = sqlx::query("UPDATE integration_profiles SET encrypted_settings=?,models=?,metadata=? WHERE id=? AND encrypted_settings=?")
            .bind(secrets::encrypt(&state.config.encryption_key, &serde_json::to_string(&settings)?)?)
            .bind(models.to_string()).bind(metadata.to_string()).bind(row.get::<String,_>("id")).bind(encrypted)
            .execute(&state.pool).await?;
        count += result.rows_affected() as usize;
    }
    Ok(count)
}

#[cfg(test)]
mod model_field_repair_tests {
    use super::*;
    #[test]
    fn repairs_confirmed_reversal_and_is_idempotent() {
        let aliases = vec!["gpt-5.6-terra".to_owned()];
        let mut config =
            json!({"models":[{"id":"gpt 5.6 terra","name":"gpt-5.6-terra","cost":{"input":2}}]});
        assert_eq!(repair_reversed_pi_models(&mut config, &aliases).len(), 1);
        assert_eq!(config["models"][0]["id"], "gpt-5.6-terra");
        assert_eq!(config["models"][0]["name"], "gpt 5.6 terra");
        assert_eq!(config["models"][0]["cost"]["input"], 2);
        assert!(repair_reversed_pi_models(&mut config, &aliases).is_empty());
    }
    #[test]
    fn preserves_real_aliases_and_unconfirmed_names() {
        for aliases in [vec!["gpt 5.6 terra".into(), "gpt-5.6-terra".into()], vec![]] {
            let mut config = json!({"models":[{"id":"gpt 5.6 terra","name":"gpt-5.6-terra"}]});
            let original = config.clone();
            assert!(repair_reversed_pi_models(&mut config, &aliases).is_empty());
            assert_eq!(config, original);
        }
    }
}

const PAIRING_TTL_SECONDS: i64 = 10 * 60;

#[derive(Deserialize)]
pub struct RedeemRequest {
    token: String,
}

fn pairing_scope(row: &sqlx::sqlite::SqliteRow) -> AppResult<Vec<String>> {
    let status: Option<i64> = row.try_get("status")?;
    let expiry: Option<i64> = row.try_get("expires_at")?;
    if status != Some(1)
        || expiry.is_none_or(|expiry| expiry < 0 || (expiry > 0 && expiry <= store::now()))
    {
        return Err(AppError::BadRequest("访问密钥已停用或过期".into()));
    }
    let stored: Option<String> = row.try_get("allowed_models")?;
    crate::console::parse_key_models(
        stored
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("访问密钥不存在".into()))?,
    )
}

fn configured_aliases(models: &Value) -> Vec<&str> {
    [
        "/claude/model",
        "/claude/haikuModel",
        "/claude/sonnetModel",
        "/claude/opusModel",
        "/codex/model",
        "/gemini/model",
        "/grok/model",
        "/opencode/model",
        "/openclaw/model",
        "/hermes/model",
        "/pi/model",
    ]
    .into_iter()
    .filter_map(|path| models.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|alias| !alias.is_empty())
    .collect()
}

pub async fn redeem(
    State(state): State<AppState>,
    Json(input): Json<RedeemRequest>,
) -> AppResult<Json<Envelope<Value>>> {
    let token = input.token.trim();
    if token.len() < 32 {
        return Err(AppError::BadRequest("接入凭证无效".into()));
    }
    let token_hash = store::hash_key(token);
    let now = store::now();
    let mut transaction = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = sqlx::query(
        "SELECT t.id,t.user_id,t.client,t.model_alias,t.config_json,t.encrypted_api_key AS token_key,t.base_url,k.encrypted_key,k.status,k.expires_at,k.allowed_models \
         FROM client_connect_tokens t JOIN users u ON u.id=t.user_id AND u.status=1 LEFT JOIN virtual_keys k ON k.id=t.virtual_key_id AND k.user_id=t.user_id \
         WHERE t.token_hash=? AND t.consumed_at IS NULL AND t.expires_at>?",
    )
    .bind(&token_hash)
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| AppError::BadRequest("接入凭证不存在、已使用或已过期".into()))?;
    let token_key: Option<String> = row.get("token_key");
    let config: Value = serde_json::from_str(row.get("config_json"))?;
    if token_key.is_none() {
        let allowed = pairing_scope(&row)?;
        let aliases = configured_aliases(config.get("models").unwrap_or(&Value::Null));
        let fallback: &str = row.get("model_alias");
        let aliases = if aliases.is_empty() {
            vec![fallback]
        } else {
            aliases
        };
        for alias in aliases {
            if !crate::proxy::allows_scope(&allowed, alias) {
                return Err(AppError::BadRequest(format!(
                    "访问密钥不允许使用模型 {alias}"
                )));
            }
            let active: i64=sqlx::query_scalar("SELECT COUNT(*) FROM user_models m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id JOIN users owner ON owner.id=m.user_id WHERE m.alias=? AND m.enabled=1 AND p.enabled=1 AND owner.status=1 AND (m.user_id=? OR owner.role='admin')")
                .bind(alias).bind(row.get::<i64,_>("user_id")).fetch_one(&mut *transaction).await?;
            if active == 0 {
                return Err(AppError::BadRequest(format!("模型 {alias} 不存在或未启用")));
            }
        }
    }
    let encrypted: Option<String> = token_key.or_else(|| row.get("encrypted_key"));
    let encrypted = encrypted.ok_or_else(|| {
        AppError::BadRequest("该旧密钥没有可用于本地接入的完整值，请创建新密钥".into())
    })?;
    let mut models = config.get("models").cloned().unwrap_or_else(|| json!({}));
    if let Some(encrypted) = config.get("encryptedNative").and_then(Value::as_str) {
        models["native"] = json!({
            "client":config.pointer("/metadata/nativeClient"),
            "providerKey":config.pointer("/metadata/providerKey"),
            "apiFormat":config.pointer("/metadata/apiFormat"),
            "codexChatReasoning":config.pointer("/metadata/codexChatReasoning"),
            "authBinding":config.pointer("/metadata/authBinding"),
            "commonConfigEnabled":config.pointer("/metadata/commonConfigEnabled"),
            "pricing":config.pointer("/metadata/pricing"),
            "settingsConfig":serde_json::from_str::<Value>(&secrets::decrypt(&state.config.encryption_key, encrypted)?)?
        });
    }
    let api_key = secrets::decrypt(&state.config.encryption_key, &encrypted)?;
    // Prepare the complete response before consuming this single-use token.
    // A decryption/configuration failure must remain retryable after repair.
    let result = sqlx::query(
        "UPDATE client_connect_tokens SET consumed_at=? WHERE id=? AND consumed_at IS NULL",
    )
    .bind(now)
    .bind(row.get::<i64, _>("id"))
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() != 1 {
        return Err(AppError::BadRequest("接入凭证已使用".into()));
    }
    transaction.commit().await?;
    Ok(Json(Envelope::ok(json!({
        "client": row.get::<String, _>("client"),
        "model": row.get::<String, _>("model_alias"),
        "api_key": api_key,
        "apps": config.get("apps").cloned().unwrap_or(Value::Null),
        "models": models,
        "name": config.get("name").and_then(Value::as_str).unwrap_or("JunoHub"),
        "provider_id": config.get("providerId").and_then(Value::as_str).unwrap_or(""),
        "base_url": row.get::<String,_>("base_url"),
    }))))
}

pub async fn create(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let key_id = payload
        .get("key_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::BadRequest("请选择访问密钥".into()))?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("请选择模型".into()))?;
    let client = payload
        .get("client")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("all");
    if !matches!(client, "claude" | "codex" | "gemini" | "all") {
        return Err(AppError::BadRequest("接入客户端无效".into()));
    }
    let now = store::now();
    let key = sqlx::query(
        "SELECT allowed_models,status,expires_at,encrypted_key FROM virtual_keys WHERE id=? AND user_id=?",
    )
    .bind(key_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("访问密钥不存在或不属于当前用户".into()))?;
    if key.get::<i64, _>("status") != 1
        || key.get::<i64, _>("expires_at") > 0 && key.get::<i64, _>("expires_at") <= now
    {
        return Err(AppError::BadRequest("访问密钥已停用或过期".into()));
    }
    if key.get::<Option<String>, _>("encrypted_key").is_none() {
        return Err(AppError::BadRequest(
            "该旧密钥没有可用于本地接入的完整值，请创建新密钥".into(),
        ));
    }
    if crate::console::resolve_model(state, uid, model)
        .await
        .is_err()
    {
        return Err(AppError::BadRequest("所选模型不存在或未启用".into()));
    }
    let allowed = pairing_scope(&key)?;
    if !crate::proxy::allows_scope(&allowed, model) {
        return Err(AppError::BadRequest("该访问密钥不允许使用所选模型".into()));
    }
    let mut random = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let token = hex::encode(random);
    sqlx::query(
        "INSERT INTO client_connect_tokens(token_hash,user_id,virtual_key_id,client,model_alias,expires_at,created_at,config_json) VALUES(?,?,?,?,?,?,?,'{}')",
    )
    .bind(store::hash_key(&token))
    .bind(uid)
    .bind(key_id)
    .bind(client)
    .bind(model)
    .bind(now + PAIRING_TTL_SECONDS)
    .bind(now)
    .execute(&state.pool)
    .await?;
    sqlx::query("DELETE FROM client_connect_tokens WHERE expires_at<?")
        .bind(now - 86_400)
        .execute(&state.pool)
        .await?;
    Ok(json!({"token":token,"expires_at":now + PAIRING_TTL_SECONDS}))
}

pub async fn list_profiles(state: &AppState, uid: i64) -> AppResult<Value> {
    let rows = sqlx::query(
        "SELECT p.id,p.name,p.provider_type,p.virtual_key_id,p.apps,p.models,p.website_url,p.notes,p.base_url,p.encrypted_api_key,p.created_at,p.updated_at,p.sort_order,p.metadata,p.encrypted_settings,k.name AS key_name,k.key_prefix,k.status AS key_status \
         FROM integration_profiles p LEFT JOIN virtual_keys k ON k.id=p.virtual_key_id AND k.user_id=p.user_id \
         WHERE p.user_id=? ORDER BY p.sort_order,p.created_at,p.id",
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;
    let list = rows.into_iter().map(|row| json!({
        "id": row.get::<String,_>("id"),
        "name": row.get::<String,_>("name"),
        "providerType": row.get::<String,_>("provider_type"),
        "keyId": row.get::<i64,_>("virtual_key_id"),
        "keyName": row.get::<Option<String>,_>("key_name").unwrap_or_else(|| "密钥已删除".into()),
        "keyPrefix": row.get::<Option<String>,_>("key_prefix").unwrap_or_default(),
        "keyStatus": row.get::<Option<i64>,_>("key_status").unwrap_or(0),
        "baseUrl": row.get::<String,_>("base_url"),
        "hasApiKey": row.get::<Option<String>,_>("encrypted_api_key").is_some(),
        "apps": serde_json::from_str::<Value>(row.get("apps")).unwrap_or_else(|_| json!({})),
        "models": serde_json::from_str::<Value>(row.get("models")).unwrap_or_else(|_| json!({})),
        "websiteUrl": row.get::<String,_>("website_url"),
        "notes": row.get::<String,_>("notes"),
        "createdAt": row.get::<i64,_>("created_at"),
        "updatedAt": row.get::<i64,_>("updated_at"),
        "sortOrder": row.get::<i64,_>("sort_order"),
        "metadata": serde_json::from_str::<Value>(row.get("metadata")).unwrap_or_else(|_|json!({})),
        "hasNativeConfig": row.get::<Option<String>,_>("encrypted_settings").is_some(),
    })).collect::<Vec<_>>();
    Ok(json!({"list":list,"total":list.len()}))
}

pub async fn profile_detail(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let row =
        sqlx::query("SELECT encrypted_settings FROM integration_profiles WHERE id=? AND user_id=?")
            .bind(payload.get("id").and_then(Value::as_str).unwrap_or(""))
            .bind(uid)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| AppError::NotFound("供应商不存在".into()))?;
    let config = row
        .get::<Option<String>, _>("encrypted_settings")
        .map(|encrypted| {
            let text = secrets::decrypt(&state.config.encryption_key, &encrypted)?;
            Ok::<Value, AppError>(serde_json::from_str(&text)?)
        })
        .transpose()?;
    Ok(json!({"settingsConfig":config}))
}

async fn upsert_native_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("请输入供应商名称".into()));
    }
    let metadata = payload
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let client = metadata
        .get("nativeClient")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !matches!(
        client,
        "claude" | "codex" | "gemini" | "grok" | "opencode" | "openclaw" | "hermes" | "pi"
    ) {
        return Err(AppError::BadRequest("请选择所属客户端".into()));
    }
    if let Some(pricing) = metadata.get("pricing") {
        crate::client_config_validation::validate_pricing(pricing)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
    }
    let normalized =
        crate::client_config_validation::normalize(client, payload.get("settingsConfig").unwrap());
    let settings = &normalized;
    crate::client_config_validation::validate(client, settings)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    if !settings.is_object() {
        return Err(AppError::BadRequest("配置必须为 JSON 对象".into()));
    }
    if matches!(client, "codex" | "grok") {
        settings
            .get("config")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::BadRequest("缺少 config TOML 配置".into()))?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| AppError::BadRequest("TOML 配置格式无效".into()))?;
    }
    let text = serde_json::to_string(settings)?;
    if text.len() > 2 * 1024 * 1024 {
        return Err(AppError::BadRequest("配置超过 2 MB".into()));
    }
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let existing_owner: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM integration_profiles WHERE id=?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    if existing_owner.is_some_and(|owner| owner != uid) {
        return Err(AppError::NotFound("供应商不存在".into()));
    }
    if matches!(client, "opencode" | "openclaw" | "hermes" | "pi") {
        let provider_key = metadata
            .get("providerKey")
            .and_then(Value::as_str)
            .unwrap_or("");
        if provider_key.trim().is_empty() || provider_key.contains(['/', '\\', '\n', '\r', '\0']) {
            return Err(AppError::BadRequest(
                "供应商标识不能为空或包含路径分隔符".into(),
            ));
        }
        let previous: Option<String> = sqlx::query_scalar("SELECT json_extract(metadata,'$.providerKey') FROM integration_profiles WHERE id=? AND encrypted_settings IS NOT NULL")
            .bind(&id).fetch_optional(&state.pool).await?.flatten();
        if previous.is_some_and(|key| key != provider_key) {
            return Err(AppError::BadRequest(
                "供应商标识创建后不能修改，请复制为新供应商".into(),
            ));
        }
        let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM integration_profiles WHERE user_id=? AND id<>? AND json_extract(metadata,'$.nativeClient')=? AND json_extract(metadata,'$.providerKey')=?")
            .bind(uid).bind(&id).bind(client).bind(provider_key).fetch_one(&state.pool).await?;
        if duplicate > 0 {
            return Err(AppError::BadRequest("该客户端已存在相同供应商标识".into()));
        }
    }
    let apps = json!({client:true});
    let models = payload.get("models").cloned().unwrap_or_else(|| json!({}));
    let encrypted = secrets::encrypt(&state.config.encryption_key, &text)?;
    // Native providers may use official login or environment references instead of a static key.
    let token_key = secrets::encrypt(
        &state.config.encryption_key,
        payload.get("apiKey").and_then(Value::as_str).unwrap_or(""),
    )?;
    let now = store::now();
    sqlx::query("INSERT INTO integration_profiles(id,user_id,name,provider_type,virtual_key_id,apps,models,website_url,notes,base_url,encrypted_api_key,created_at,updated_at,sort_order,metadata,encrypted_settings) VALUES(?,?,?,'custom_gateway',0,?,?,?,?,?,?,?,?,(SELECT COALESCE(MAX(sort_order),-1)+1 FROM integration_profiles WHERE user_id=?),?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,provider_type=excluded.provider_type,virtual_key_id=0,apps=excluded.apps,models=excluded.models,website_url=excluded.website_url,notes=excluded.notes,base_url=excluded.base_url,encrypted_api_key=excluded.encrypted_api_key,metadata=excluded.metadata,encrypted_settings=excluded.encrypted_settings,updated_at=excluded.updated_at WHERE integration_profiles.user_id=excluded.user_id")
        .bind(&id).bind(uid).bind(name).bind(serde_json::to_string(&apps)?).bind(serde_json::to_string(&models)?)
        .bind(payload.get("websiteUrl").and_then(Value::as_str).unwrap_or(""))
        .bind(payload.get("notes").and_then(Value::as_str).unwrap_or(""))
        .bind(payload.get("baseUrl").and_then(Value::as_str).unwrap_or(""))
        .bind(token_key).bind(now).bind(now).bind(uid).bind(serde_json::to_string(&metadata)?).bind(encrypted)
        .execute(&state.pool).await?;
    Ok(json!({"id":id}))
}

pub async fn upsert_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    if payload.get("settingsConfig").is_some() {
        return upsert_native_profile(state, uid, payload).await;
    }
    let profile_id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("请输入接入名称".into()))?;
    let provider_type = payload
        .get("providerType")
        .and_then(Value::as_str)
        .unwrap_or("junohub");
    if !matches!(provider_type, "junohub" | "newapi" | "custom_gateway") {
        return Err(AppError::BadRequest("供应商类型无效".into()));
    }
    let key_id = payload.get("keyId").and_then(Value::as_i64).unwrap_or(0);
    let base_url = payload
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let api_key = payload
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let mut encrypted_api_key = None;
    let mut allowed = Vec::<String>::new();
    if provider_type == "junohub" {
        if key_id <= 0 {
            return Err(AppError::BadRequest("请选择访问密钥".into()));
        }
        let key = sqlx::query(
            "SELECT allowed_models,status,expires_at,encrypted_key FROM virtual_keys WHERE id=? AND user_id=?",
        )
        .bind(key_id)
        .bind(uid)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("访问密钥不存在或无权使用".into()))?;
        if key.get::<i64, _>("status") != 1
            || (key.get::<i64, _>("expires_at") > 0
                && key.get::<i64, _>("expires_at") <= store::now())
            || key.get::<Option<String>, _>("encrypted_key").is_none()
        {
            return Err(AppError::BadRequest(
                "访问密钥已停用、过期或缺少完整密钥".into(),
            ));
        }
        allowed = pairing_scope(&key)?;
    } else {
        let parsed = url::Url::parse(base_url)
            .map_err(|_| AppError::BadRequest("请输入有效的 Base URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(AppError::BadRequest(
                "Base URL 必须使用 HTTP 或 HTTPS".into(),
            ));
        }
        if api_key.is_empty() {
            if let Some(existing_id) = payload.get("id").and_then(Value::as_str) {
                encrypted_api_key = sqlx::query_scalar(
                    "SELECT encrypted_api_key FROM integration_profiles WHERE id=? AND user_id=?",
                )
                .bind(existing_id)
                .bind(uid)
                .fetch_optional(&state.pool)
                .await?
                .flatten();
            }
            if encrypted_api_key.is_none() {
                if let Some(source_id) = payload.get("copyFromId").and_then(Value::as_str) {
                    encrypted_api_key = sqlx::query_scalar(
                        "SELECT encrypted_api_key FROM integration_profiles WHERE id=? AND user_id=? AND provider_type!='junohub'",
                    )
                    .bind(source_id)
                    .bind(uid)
                    .fetch_optional(&state.pool)
                    .await?
                    .flatten();
                }
            }
            if encrypted_api_key.is_none() {
                return Err(AppError::BadRequest("请输入 API Key".into()));
            }
        } else {
            encrypted_api_key = Some(secrets::encrypt(&state.config.encryption_key, api_key)?);
        }
    }
    let apps = payload
        .get("apps")
        .cloned()
        .unwrap_or_else(|| json!({"claude":true,"codex":true,"gemini":true}));
    let enabled = [
        "claude", "codex", "gemini", "grok", "opencode", "openclaw", "hermes", "pi",
    ]
    .iter()
    .filter(|app| apps.get(**app).and_then(Value::as_bool) == Some(true))
    .count();
    if enabled == 0 {
        return Err(AppError::BadRequest("至少启用一个客户端".into()));
    }
    let models = payload.get("models").cloned().unwrap_or_else(|| json!({}));
    for app in [
        "claude", "codex", "gemini", "grok", "opencode", "openclaw", "hermes", "pi",
    ] {
        if apps.get(app).and_then(Value::as_bool) == Some(true)
            && models
                .pointer(&format!("/{app}/model"))
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AppError::BadRequest(format!("请配置 {app} 的默认模型")));
        }
    }
    for (app, allowed_protocols) in [
        ("grok", &["responses", "chat"][..]),
        (
            "opencode",
            &[
                "@ai-sdk/openai",
                "@ai-sdk/openai-compatible",
                "@ai-sdk/anthropic",
            ][..],
        ),
        (
            "openclaw",
            &[
                "openai-responses",
                "openai-completions",
                "anthropic-messages",
            ][..],
        ),
        (
            "hermes",
            &["codex_responses", "chat_completions", "anthropic_messages"][..],
        ),
        (
            "pi",
            &[
                "openai-responses",
                "openai-completions",
                "anthropic-messages",
            ][..],
        ),
    ] {
        if let Some(protocol) = models
            .pointer(&format!("/{app}/protocol"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !allowed_protocols.contains(&protocol) {
                return Err(AppError::BadRequest(format!("{app} 的 API 协议无效")));
            }
        }
    }
    let configured = [
        "/claude/model",
        "/claude/haikuModel",
        "/claude/sonnetModel",
        "/claude/opusModel",
        "/codex/model",
        "/gemini/model",
        "/grok/model",
        "/opencode/model",
        "/openclaw/model",
        "/hermes/model",
        "/pi/model",
    ]
    .into_iter()
    .filter_map(|path| models.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<std::collections::HashSet<_>>();
    for alias in configured {
        if provider_type == "junohub" && !crate::proxy::allows_scope(&allowed, alias) {
            return Err(AppError::BadRequest(format!(
                "访问密钥不允许使用模型 {alias}"
            )));
        }
        if provider_type == "junohub"
            && crate::console::resolve_model(state, uid, alias)
                .await
                .is_err()
        {
            return Err(AppError::BadRequest(format!("模型 {alias} 不存在或未启用")));
        }
    }
    let now = store::now();
    let sort_order: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sort_order),-1)+1 FROM integration_profiles WHERE user_id=?",
    )
    .bind(uid)
    .fetch_one(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO integration_profiles(id,user_id,name,provider_type,virtual_key_id,apps,models,website_url,notes,base_url,encrypted_api_key,created_at,updated_at,sort_order,metadata) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,provider_type=excluded.provider_type,virtual_key_id=excluded.virtual_key_id,apps=excluded.apps,models=excluded.models,website_url=excluded.website_url,notes=excluded.notes,base_url=excluded.base_url,encrypted_api_key=excluded.encrypted_api_key,metadata=excluded.metadata,updated_at=excluded.updated_at WHERE integration_profiles.user_id=excluded.user_id",
    )
    .bind(&profile_id).bind(uid).bind(name)
    .bind(provider_type)
    .bind(key_id).bind(serde_json::to_string(&apps)?).bind(serde_json::to_string(&models)?)
    .bind(payload.get("websiteUrl").and_then(Value::as_str).unwrap_or(""))
    .bind(payload.get("notes").and_then(Value::as_str).unwrap_or(""))
    .bind(base_url)
    .bind(encrypted_api_key)
    .bind(now).bind(now).bind(sort_order).bind(serde_json::to_string(&payload.get("metadata").cloned().unwrap_or_else(||json!({})))?).execute(&state.pool).await?;
    Ok(json!({"id":profile_id}))
}

pub async fn reorder_profiles(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let ids = payload
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("供应商顺序无效".into()))?
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| AppError::BadRequest("供应商顺序无效".into()))?;
    let owned: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM integration_profiles WHERE user_id=?")
            .bind(uid)
            .fetch_one(&state.pool)
            .await?;
    let unique = ids.iter().collect::<std::collections::HashSet<_>>();
    if ids.len() as i64 != owned || unique.len() != ids.len() {
        return Err(AppError::BadRequest(
            "供应商顺序必须包含全部供应商且不能重复".into(),
        ));
    }
    let mut transaction = state.pool.begin().await?;
    for (order, id) in ids.iter().enumerate() {
        let result =
            sqlx::query("UPDATE integration_profiles SET sort_order=? WHERE id=? AND user_id=?")
                .bind(order as i64)
                .bind(id)
                .bind(uid)
                .execute(&mut *transaction)
                .await?;
        if result.rows_affected() != 1 {
            return Err(AppError::BadRequest("供应商顺序包含无权访问的项目".into()));
        }
    }
    transaction.commit().await?;
    Ok(json!({"updated":ids.len()}))
}

pub async fn delete_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let profile_id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let result = sqlx::query("DELETE FROM integration_profiles WHERE id=? AND user_id=?")
        .bind(profile_id)
        .bind(uid)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() != 1 {
        return Err(AppError::NotFound("接入配置不存在".into()));
    }
    Ok(json!({"deleted":true}))
}

pub async fn sync_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let profile_id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let row = sqlx::query(
        "SELECT p.name,p.provider_type,p.virtual_key_id,p.apps,p.models,p.base_url,p.encrypted_api_key,p.encrypted_settings,p.metadata,k.status,k.expires_at,k.encrypted_key,k.allowed_models \
         FROM integration_profiles p LEFT JOIN virtual_keys k ON k.id=p.virtual_key_id AND k.user_id=p.user_id WHERE p.id=? AND p.user_id=?",
    ).bind(profile_id).bind(uid).fetch_optional(&state.pool).await?
        .ok_or_else(|| AppError::NotFound("接入配置不存在".into()))?;
    let now = store::now();
    let provider_type: String = row.get("provider_type");
    let encrypted_api_key = if provider_type == "junohub" {
        let status: Option<i64> = row.get("status");
        let expires_at: Option<i64> = row.get("expires_at");
        let encrypted: Option<String> = row.get("encrypted_key");
        if status != Some(1)
            || expires_at.is_some_and(|expires| expires > 0 && expires <= now)
            || encrypted.is_none()
        {
            return Err(AppError::BadRequest(
                "访问密钥已停用、过期或缺少完整值".into(),
            ));
        }
        None
    } else {
        Some(
            row.get::<Option<String>, _>("encrypted_api_key")
                .ok_or_else(|| AppError::BadRequest("供应商缺少 API Key".into()))?,
        )
    };
    let mut apps: Value = serde_json::from_str(row.get("apps"))?;
    let models: Value = serde_json::from_str(row.get("models"))?;
    if provider_type == "junohub" {
        let allowed = pairing_scope(&row)?;
        for alias in configured_aliases(&models) {
            if !crate::proxy::allows_scope(&allowed, alias) {
                return Err(AppError::BadRequest(format!(
                    "访问密钥不允许使用模型 {alias}"
                )));
            }
            crate::console::resolve_model(state, uid, alias).await?;
        }
    }
    let requested_client = payload
        .get("client")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(client) = requested_client {
        if !matches!(
            client,
            "claude" | "codex" | "gemini" | "grok" | "opencode" | "openclaw" | "hermes" | "pi"
        ) {
            return Err(AppError::BadRequest("接入客户端无效".into()));
        }
        if apps.get(client).and_then(Value::as_bool) != Some(true) {
            return Err(AppError::BadRequest(format!(
                "供应商未启用 {} 客户端",
                client
            )));
        }
        apps = json!({
            "claude": client == "claude",
            "codex": client == "codex",
            "gemini": client == "gemini",
            "grok": client == "grok",
            "opencode": client == "opencode",
            "openclaw": client == "openclaw",
            "hermes": client == "hermes",
            "pi": client == "pi"
        });
    }
    let client = [
        "claude", "codex", "gemini", "grok", "opencode", "openclaw", "hermes", "pi",
    ]
    .into_iter()
    .filter(|app| apps.get(*app).and_then(Value::as_bool) == Some(true))
    .collect::<Vec<_>>()
    .join(",");
    let model = requested_client
        .and_then(|app| models.get(app))
        .and_then(|value| value.get("model"))
        .or_else(|| {
            [
                "claude", "codex", "gemini", "grok", "opencode", "openclaw", "hermes", "pi",
            ]
            .into_iter()
            .find_map(|app| {
                (apps.get(app).and_then(Value::as_bool) == Some(true))
                    .then(|| models.get(app).and_then(|value| value.get("model")))
                    .flatten()
            })
        })
        .and_then(Value::as_str)
        .unwrap_or("default");
    let mut random = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let token = hex::encode(random);
    let metadata: Value = serde_json::from_str(row.get("metadata"))?;
    let config = json!({"providerId":profile_id,"name":row.get::<String,_>("name"),"apps":apps,"models":models,
        "encryptedNative":row.get::<Option<String>,_>("encrypted_settings"), "metadata":metadata});
    sqlx::query("INSERT INTO client_connect_tokens(token_hash,user_id,virtual_key_id,client,model_alias,expires_at,created_at,config_json,encrypted_api_key,base_url) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(store::hash_key(&token)).bind(uid)
        .bind((provider_type == "junohub").then(|| row.get::<i64,_>("virtual_key_id")))
        .bind(client).bind(model).bind(now + PAIRING_TTL_SECONDS).bind(now)
        .bind(serde_json::to_string(&config)?).bind(encrypted_api_key)
        .bind(if provider_type == "junohub" { "" } else { row.get::<&str,_>("base_url") })
        .execute(&state.pool).await?;
    Ok(json!({"token":token,"expires_at":now + PAIRING_TTL_SECONDS}))
}

pub async fn test_profile(state: &AppState, uid: i64, payload: &Value) -> AppResult<Value> {
    let profile_id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("供应商标识无效".into()))?;
    let client = payload
        .get("client")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("codex");
    if !matches!(
        client,
        "claude" | "codex" | "gemini" | "grok" | "opencode" | "openclaw" | "hermes" | "pi"
    ) {
        return Err(AppError::BadRequest("接入客户端无效".into()));
    }
    let row = sqlx::query(
        "SELECT p.provider_type,p.virtual_key_id,p.apps,p.models,p.metadata,p.base_url,p.encrypted_api_key,k.status,k.expires_at,k.encrypted_key \
         FROM integration_profiles p LEFT JOIN virtual_keys k ON k.id=p.virtual_key_id AND k.user_id=p.user_id \
         WHERE p.id=? AND p.user_id=?",
    )
    .bind(profile_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("供应商不存在".into()))?;
    let apps: Value = serde_json::from_str(row.get("apps"))?;
    if apps.get(client).and_then(Value::as_bool) != Some(true) {
        return Err(AppError::BadRequest(format!(
            "供应商未启用 {client} 客户端"
        )));
    }
    let provider_type: String = row.get("provider_type");
    if provider_type == "junohub" {
        let now = store::now();
        let status: Option<i64> = row.get("status");
        let expires_at: Option<i64> = row.get("expires_at");
        let encrypted: Option<String> = row.get("encrypted_key");
        if status != Some(1)
            || expires_at.is_some_and(|expires| expires > 0 && expires <= now)
            || encrypted.is_none()
        {
            return Ok(
                json!({"ok":false,"outcome":"error","status_code":0,"message":"JunoHub 访问密钥已停用、过期或缺少完整值。","model_count":0,"models":[],"latency_ms":0}),
            );
        }
        return Ok(
            json!({"ok":true,"outcome":"success","status_code":200,"message":"JunoHub 地址、访问密钥和客户端配置均有效。","model_count":0,"models":[],"latency_ms":0}),
        );
    }
    let encrypted: Option<String> = row.get("encrypted_api_key");
    let api_key = secrets::decrypt(
        &state.config.encryption_key,
        &encrypted.ok_or_else(|| AppError::BadRequest("供应商缺少 API Key".into()))?,
    )?;
    if api_key.is_empty() || api_key.starts_with('!') {
        return Ok(
            json!({"ok":false,"outcome":"warning","status_code":0,"message":"当前配置没有可供检测的 API Key；若使用客户端登录，请在客户端中验证。","model_count":0,"models":[],"latency_ms":0}),
        );
    }
    let models: Value = serde_json::from_str(row.get("models"))?;
    let protocol = models
        .get(client)
        .and_then(|v| v.get("protocol"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let platform = match client {
        "claude" => "anthropic",
        "gemini" => "gemini",
        _ if matches!(
            protocol,
            "anthropic-messages" | "anthropic_messages" | "@ai-sdk/anthropic"
        ) =>
        {
            "anthropic"
        }
        _ if matches!(protocol, "google-generative-ai" | "@ai-sdk/google") => "gemini",
        _ => "custom",
    };
    let report = crate::upstream_probe::probe(
        state,
        row.get::<&str, _>("base_url"),
        &api_key,
        platform,
        None,
    )
    .await?;
    Ok(serde_json::to_value(report)?)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use axum::extract::State;
    use reqwest::Client;
    use sqlx::{postgres::PgPoolOptions, sqlite::SqlitePoolOptions};

    use super::*;
    use crate::{config::Config, oauth::OAuthSessionStore};

    async fn state() -> AppState {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let now = store::now();
        sqlx::query("INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) VALUES('user@example.com','unused','User','user',1,?,?)")
            .bind(now).bind(now).execute(&pool).await.unwrap();
        // Catalog resolution joins enabled providers; an orphan model is not a
        // usable route. Keep the pairing fixture consistent with production.
        sqlx::query("INSERT INTO user_providers(user_id,name,provider_type,base_url,encrypted_api_key,enabled,created_at,updated_at) VALUES(1,'fixture','openai','https://example.test/v1','',1,?,?)")
            .bind(now).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at) VALUES(1,1,'route-model','upstream','[]',1,?,?)")
            .bind(now).bind(now).execute(&pool).await.unwrap();
        let encrypted = secrets::encrypt("test-encryption-key", "sk-juno-test").unwrap();
        sqlx::query("INSERT INTO virtual_keys(user_id,name,key_prefix,key_hash,status,allowed_models,expires_at,created_at,updated_at,encrypted_key) VALUES(1,'test','sk-juno','hash',1,'[]',0,?,?,?)")
            .bind(now).bind(now).bind(encrypted).execute(&pool).await.unwrap();
        AppState {
            pool,
            sub2_pool: PgPoolOptions::new()
                .connect_lazy("postgres://test:test@127.0.0.1/test")
                .unwrap(),
            sub2_redis: None,
            oauth_sessions: OAuthSessionStore::default(),
            config: Arc::new(Config {
                bind: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                database_url: "sqlite::memory:".into(),
                jwt_secret: "test".into(),
                bootstrap_admin_email: String::new(),
                bootstrap_admin_password: String::new(),
                encryption_key: "test-encryption-key".into(),
                email: Default::default(),
                oss: Default::default(),
                sub2api_admin_email: String::new(),
                sub2api_database_url: "postgres://test:test@127.0.0.1/test".into(),
                sub2api_redis_url: "redis://127.0.0.1/".into(),
                sub2api_slot_ttl_seconds: 1800,
                sub2api_wait_ttl_seconds: 120,
                sub2api_sticky_ttl_seconds: 3600,
                sub2api_response_sticky_ttl_seconds: 3600,
                sub2api_session_hash_read_old_fallback: true,
                sub2api_session_hash_dual_write_old: true,
                scheduler_worker_enabled: false,
                account_engine_gateway_enabled: false,
                allowed_origins: Vec::new(),
                log_retention_days: 0,
            }),
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn third_review_pairing_scopes_and_revocation() {
        let state = state().await;
        sqlx::query("UPDATE virtual_keys SET allowed_models='[\"route-*\"]' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let created = create(
            &state,
            1,
            &json!({"key_id":1,"model":"route-model","client":"codex"}),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE virtual_keys SET allowed_models='[\"other\"]' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: created["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
        sqlx::query("UPDATE virtual_keys SET allowed_models='not-json' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            create(&state, 1, &json!({"key_id":1,"model":"route-model"}))
                .await
                .is_err()
        );
        sqlx::query("UPDATE virtual_keys SET allowed_models='[]',expires_at=-1 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            create(&state, 1, &json!({"key_id":1,"model":"route-model"}))
                .await
                .is_err()
        );
        sqlx::query("UPDATE virtual_keys SET expires_at=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let created = create(&state, 1, &json!({"key_id":1,"model":"route-model"}))
            .await
            .unwrap();
        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: created["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn third_review_profile_sync_checks_current_scope_and_redemption_is_atomic() {
        let state = state().await;
        sqlx::query("UPDATE virtual_keys SET allowed_models='[\"route-*\"]' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let profile=upsert_profile(&state,1,&json!({"name":"Scoped","providerType":"junohub","keyId":1,"apps":{"codex":true},"models":{"codex":{"model":"route-model"}}})).await.unwrap();
        let token = sync_profile(&state, 1, &profile).await.unwrap();
        sqlx::query("UPDATE virtual_keys SET allowed_models='[\"different\"]' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(sync_profile(&state, 1, &profile).await.is_err());
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
        sqlx::query("UPDATE virtual_keys SET allowed_models='[]' WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE user_models SET enabled=0 WHERE alias='route-model'")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());

        let native=upsert_profile(&state,1,&json!({"name":"Native","metadata":{"nativeClient":"pi","providerKey":"custom"},"settingsConfig":{"apiKey":"native-secret","models":[{"id":"external"}]}})).await.unwrap();
        let token = sync_profile(&state, 1, &native).await.unwrap();
        let hash = store::hash_key(token["token"].as_str().unwrap());
        let original: String =
            sqlx::query_scalar("SELECT config_json FROM client_connect_tokens WHERE token_hash=?")
                .bind(&hash)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        let mut broken: Value = serde_json::from_str(&original).unwrap();
        broken["encryptedNative"] = json!("bad-ciphertext");
        sqlx::query("UPDATE client_connect_tokens SET config_json=? WHERE token_hash=?")
            .bind(broken.to_string())
            .bind(&hash)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
        let consumed: Option<i64> =
            sqlx::query_scalar("SELECT consumed_at FROM client_connect_tokens WHERE token_hash=?")
                .bind(&hash)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(consumed.is_none());
        sqlx::query("UPDATE client_connect_tokens SET config_json=? WHERE token_hash=?")
            .bind(original)
            .bind(&hash)
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET status=0 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
        sqlx::query("UPDATE users SET status=1 WHERE id=1")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn same_preset_name_can_be_used_across_clients() {
        let state = state().await;
        let pi = json!({"name":"JunoHub","apiKey":"pi-secret","metadata":{"nativeClient":"pi","providerKey":"junohub"},"settingsConfig":{"apiKey":"pi-secret","models":[{"id":"route-model"}]}});
        let first = upsert_profile(&state, 1, &pi).await.unwrap();
        let mut claude = json!({"name":"JunoHub","apiKey":"claude-secret","metadata":{"nativeClient":"claude"},"settingsConfig":{"env":{"ANTHROPIC_API_KEY":"claude-secret"}}});
        let second = upsert_profile(&state, 1, &claude).await.unwrap();
        assert_ne!(first["id"], second["id"]);
        claude["id"] = second["id"].clone();
        upsert_profile(&state, 1, &claude).await.unwrap();
        assert!(
            upsert_profile(&state, 1, &pi).await.is_err(),
            "same Pi provider key must still be rejected"
        );
        let detail = profile_detail(&state, 1, &first).await.unwrap();
        assert_eq!(detail["settingsConfig"]["apiKey"], "pi-secret");
        assert_eq!(
            list_profiles(&state, 1).await.unwrap()["list"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn pairing_token_reveals_key_once() {
        let state = state().await;
        let created = create(
            &state,
            1,
            &json!({"key_id":1,"model":"route-model","client":"codex"}),
        )
        .await
        .unwrap();
        let token = created["token"].as_str().unwrap().to_string();
        let first = redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: token.clone(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(first.0.data["api_key"], "sk-juno-test");
        assert_eq!(first.0.data["client"], "codex");
        assert!(redeem(State(state), Json(RedeemRequest { token }))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn universal_profile_persists_and_redeems_per_client_models() {
        let state = state().await;
        let now = store::now();
        for alias in [
            "claude-route",
            "haiku-route",
            "sonnet-route",
            "opus-route",
            "codex-route",
        ] {
            sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at) VALUES(1,1,?,?, '[]',1,?,?)")
                .bind(alias).bind(alias).bind(now).bind(now).execute(&state.pool).await.unwrap();
        }
        let saved = upsert_profile(
            &state,
            1,
            &json!({
                "name":"JunoHub",
                "providerType":"junohub",
                "keyId":1,
                "apps":{"claude":true,"codex":true,"gemini":false},
                "models":{
                    "claude":{"model":"claude-route","haikuModel":"haiku-route","sonnetModel":"sonnet-route","opusModel":"opus-route"},
                    "codex":{"model":"codex-route","reasoningEffort":"xhigh"}
                }
            }),
        ).await.unwrap();
        let profile_id = saved["id"].as_str().unwrap();
        let listed = list_profiles(&state, 1).await.unwrap();
        assert_eq!(listed["total"], 1);
        assert_eq!(
            listed["list"][0]["models"]["codex"]["reasoningEffort"],
            "xhigh"
        );

        let issued = sync_profile(&state, 1, &json!({"id":profile_id}))
            .await
            .unwrap();
        let redeemed = redeem(
            State(state),
            Json(RedeemRequest {
                token: issued["token"].as_str().unwrap().to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(redeemed.0.data["client"], "claude,codex");
        assert_eq!(
            redeemed.0.data["models"]["claude"]["opusModel"],
            "opus-route"
        );
        assert_eq!(redeemed.0.data["models"]["codex"]["model"], "codex-route");
    }

    #[tokio::test]
    async fn profile_sync_can_target_one_enabled_client() {
        let state = state().await;
        let now = store::now();
        sqlx::query("INSERT INTO user_models(user_id,provider_id,alias,upstream_model,capabilities,enabled,created_at,updated_at) VALUES(1,1,'codex-route','codex-route','[]',1,?,?)")
            .bind(now).bind(now).execute(&state.pool).await.unwrap();
        let saved = upsert_profile(
            &state,
            1,
            &json!({
                "name":"Targeted",
                "providerType":"junohub",
                "keyId":1,
                "apps":{"claude":true,"codex":true,"gemini":false},
                "models":{"claude":{"model":"route-model"},"codex":{"model":"codex-route"}}
            }),
        )
        .await
        .unwrap();
        let issued = sync_profile(&state, 1, &json!({"id":saved["id"],"client":"codex"}))
            .await
            .unwrap();
        let redeemed = redeem(
            State(state),
            Json(RedeemRequest {
                token: issued["token"].as_str().unwrap().to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(redeemed.0.data["client"], "codex");
        assert_eq!(redeemed.0.data["model"], "codex-route");
        assert_eq!(
            redeemed.0.data["apps"],
            json!({
                "claude": false,
                "codex": true,
                "gemini": false,
                "grok": false,
                "opencode": false,
                "openclaw": false,
                "hermes": false,
                "pi": false
            })
        );
    }

    #[tokio::test]
    async fn custom_provider_is_managed_without_exposing_its_key() {
        let state = state().await;
        let saved = upsert_profile(
            &state,
            1,
            &json!({
                "name":"Team Gateway",
                "providerType":"custom_gateway",
                "baseUrl":"https://gateway.example.com/api",
                "apiKey":"secret-upstream-key",
                "apps":{"claude":true,"codex":true,"gemini":true},
                "models":{
                    "claude":{"model":"claude-custom"},
                    "codex":{"model":"gpt-custom","reasoningEffort":"high"},
                    "gemini":{"model":"gemini-custom"}
                }
            }),
        )
        .await
        .unwrap();
        let id = saved["id"].as_str().unwrap();
        let listed = list_profiles(&state, 1).await.unwrap();
        assert_eq!(
            listed["list"][0]["baseUrl"],
            "https://gateway.example.com/api"
        );
        assert_eq!(listed["list"][0]["hasApiKey"], true);
        assert!(listed.to_string().find("secret-upstream-key").is_none());
        let stored: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM integration_profiles WHERE id=?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_ne!(stored, "secret-upstream-key");

        let copied = upsert_profile(
            &state,
            1,
            &json!({
                "name":"Team Gateway Copy",
                "providerType":"custom_gateway",
                "copyFromId":id,
                "baseUrl":"https://gateway.example.com/api",
                "apps":{"claude":true,"codex":false,"gemini":false},
                "models":{"claude":{"model":"claude-custom"}}
            }),
        )
        .await
        .unwrap();
        let copied_key: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM integration_profiles WHERE id=?")
                .bind(copied["id"].as_str().unwrap())
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(copied_key, stored);

        let issued = sync_profile(&state, 1, &json!({"id":id})).await.unwrap();
        let redeemed = redeem(
            State(state),
            Json(RedeemRequest {
                token: issued["token"].as_str().unwrap().to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(redeemed.0.data["name"], "Team Gateway");
        assert_eq!(
            redeemed.0.data["base_url"],
            "https://gateway.example.com/api"
        );
        assert_eq!(redeemed.0.data["api_key"], "secret-upstream-key");
    }

    #[tokio::test]
    async fn native_edit_rotates_key_and_preserves_unique_identity() {
        let state = state().await;
        let mut payload = json!({"name":"Pi","apiKey":"old-key","metadata":{"scope":"client","nativeClient":"pi","providerKey":"custom"},"settingsConfig":{"apiKey":"old-key","models":[{"id":"one"}]}});
        let saved = upsert_profile(&state, 1, &payload).await.unwrap();
        assert!(upsert_profile(&state, 1, &payload).await.is_err());
        payload["id"] = saved["id"].clone();
        payload["apiKey"] = json!("new-key");
        payload["settingsConfig"]["apiKey"] = json!("new-key");
        upsert_profile(&state, 1, &payload).await.unwrap();
        let stored: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM integration_profiles WHERE id=?")
                .bind(saved["id"].as_str().unwrap())
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(
            secrets::decrypt(&state.config.encryption_key, &stored).unwrap(),
            "new-key"
        );
        let detail = profile_detail(&state, 1, &json!({"id":saved["id"]}))
            .await
            .unwrap();
        assert_eq!(detail["settingsConfig"]["apiKey"], "new-key");
        payload["metadata"]["providerKey"] = json!("renamed");
        assert!(upsert_profile(&state, 1, &payload).await.is_err());
    }

    #[tokio::test]
    async fn native_configuration_roundtrips_encrypted_with_owner_and_client_checks() {
        let state = state().await;
        let settings = json!({"apiKey":"native-secret","headers":{"Authorization":"header-secret"},"api":"google-generative-ai","models":[{"id":"one","contextWindow":1000000},{"id":"two","reasoning":true}]});
        let saved=upsert_profile(&state,1,&json!({"name":"Pi native","metadata":{"scope":"client","nativeClient":"pi","providerKey":"google"},"settingsConfig":settings})).await.unwrap();
        let id = saved["id"].as_str().unwrap();
        let list = list_profiles(&state, 1).await.unwrap();
        assert!(!list.to_string().contains("native-secret"));
        assert!(!list.to_string().contains("header-secret"));
        assert_eq!(list["list"][0]["metadata"]["nativeClient"], "pi");
        let encrypted: String =
            sqlx::query_scalar("SELECT encrypted_settings FROM integration_profiles WHERE id=?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(!encrypted.contains("native-secret"));
        assert!(profile_detail(&state, 2, &json!({"id":id})).await.is_err());
        assert!(sync_profile(&state, 1, &json!({"id":id,"client":"codex"}))
            .await
            .is_err());
        let detail = profile_detail(&state, 1, &json!({"id":id})).await.unwrap();
        assert_eq!(detail["settingsConfig"], settings);
        let issued = sync_profile(&state, 1, &json!({"id":id,"client":"pi"}))
            .await
            .unwrap();
        let result = redeem(
            State(state.clone()),
            Json(RedeemRequest {
                token: issued["token"].as_str().unwrap().into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            result.0.data["models"]["native"]["settingsConfig"],
            settings
        );
        assert_eq!(result.0.data["models"]["native"]["providerKey"], "google");
        assert!(redeem(
            State(state),
            Json(RedeemRequest {
                token: issued["token"].as_str().unwrap().into()
            })
        )
        .await
        .is_err());
    }
}
