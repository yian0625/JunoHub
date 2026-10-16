use reqwest::Url;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    secrets,
    state::AppState,
    sub2_repository::{CreateBundleInput, Sub2Repository},
};

pub async fn complete_openai_oauth(
    state: &AppState,
    uid: i64,
    payload: &Value,
) -> AppResult<Value> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = payload
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let oauth_state = payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    crate::oauth_completion::run(
        state,
        uid,
        "openai",
        payload,
        || {
            state
                .oauth_sessions
                .exchange_openai(&state.client, uid, session_id, code, oauth_state)
        },
        |token| provision_openai_token(state, uid, payload, token),
    )
    .await
}

async fn provision_openai_token(
    state: &AppState,
    uid: i64,
    payload: &Value,
    token: crate::oauth::OpenAITokenResponse,
) -> AppResult<Value> {
    let (credentials, extra, default_name) = token.account_payload();
    let credentials = secrets::protect_credentials(&state.config.encryption_key, &credentials)?;
    let account_name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&default_name)
        .to_owned();
    if let Some(result) = update_oauth_account_if_requested(
        state,
        uid,
        payload,
        "openai",
        &account_name,
        &credentials,
        &extra,
    )
    .await?
    {
        return Ok(result);
    }
    let sub2_user_id = Sub2Repository::new(state.sub2_pool.clone())
        .sub2_user_id(&state.config.sub2api_admin_email)
        .await?;
    create_or_pool(
        state,
        uid,
        payload.get("target_provider_id").and_then(Value::as_i64),
        "OpenAI",
        CreateBundleInput {
            sub2_user_id,
            group_name: format!("JunoHub {uid} OpenAI OAuth {}", crate::store::now()),
            account_name,
            platform: "openai".into(),
            kind: "oauth".into(),
            credentials,
            extra,
            concurrency: 3,
            priority: 40,
        },
    )
    .await
}

pub async fn complete_provider_oauth(
    state: &AppState,
    uid: i64,
    provider: &str,
    payload: &Value,
) -> AppResult<Value> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = payload
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let oauth_state = payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    crate::oauth_completion::run(
        state,
        uid,
        provider,
        payload,
        || {
            state.oauth_sessions.exchange_provider(
                &state.client,
                uid,
                provider,
                session_id,
                code,
                oauth_state,
            )
        },
        |token| provision_oauth_token(state, uid, provider, payload, token),
    )
    .await
}

pub async fn import_oauth_token(
    state: &AppState,
    uid: i64,
    provider: &str,
    payload: &Value,
) -> AppResult<Value> {
    if !matches!(provider, "anthropic" | "gemini" | "grok" | "antigravity") {
        return Err(AppError::BadRequest("不支持的 OAuth 平台".into()));
    }
    let refresh_token = payload
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let access_token = payload
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let client_id = payload
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let client_secret = payload
        .get("client_secret")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let oauth_type = payload
        .get("oauth_type")
        .and_then(Value::as_str)
        .unwrap_or("code_assist");
    let project_id = payload
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let token = if !refresh_token.is_empty() {
        crate::oauth::refresh_provider_token(
            &state.client,
            provider,
            refresh_token,
            &json!({"client_id":client_id,"client_secret":client_secret}),
        )
        .await?
    } else if !access_token.is_empty() {
        crate::oauth::ProviderTokenResponse {
            access_token: access_token.into(),
            refresh_token: String::new(),
            id_token: String::new(),
            token_type: "Bearer".into(),
            expires_in: payload
                .get("expires_in")
                .and_then(Value::as_i64)
                .unwrap_or(3600),
            scope: String::new(),
            organization: Value::Null,
            account: Value::Null,
            provider: provider.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            oauth_type: oauth_type.into(),
            project_id: project_id.into(),
        }
    } else {
        return Err(AppError::BadRequest(
            "请输入 refresh_token 或 access_token".into(),
        ));
    };
    let mut token = token;
    if token.oauth_type.is_empty() {
        token.oauth_type = oauth_type.into();
    }
    if token.project_id.is_empty() {
        token.project_id = project_id.into();
    }
    if token.client_id.is_empty() {
        token.client_id = client_id.into();
    }
    if token.client_secret.is_empty() {
        token.client_secret = client_secret.into();
    }
    provision_oauth_token(state, uid, provider, payload, token).await
}

async fn provision_oauth_token(
    state: &AppState,
    uid: i64,
    provider: &str,
    payload: &Value,
    token: crate::oauth::ProviderTokenResponse,
) -> AppResult<Value> {
    let expires_at =
        crate::store::now().saturating_add(token.expires_in.saturating_sub(300).max(60));
    let mut email = String::new();
    let mut identity_name = String::new();
    let mut subject = String::new();
    if matches!(provider, "gemini" | "antigravity") {
        (email, identity_name) =
            crate::oauth::google_identity(&state.client, &token.access_token).await;
    } else if provider == "anthropic" {
        email = token
            .account
            .get("email_address")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
    } else if provider == "grok" {
        if let Some(claims) = decode_jwt_payload(&token.id_token) {
            subject = claims
                .get("sub")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            email = claims
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            identity_name = claims
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
        }
    }
    let mut project_id = token.project_id.trim().to_owned();
    let mut tier_id = String::new();
    let mut privacy_set = false;
    let google_context =
        if provider == "antigravity" || (provider == "gemini" && token.oauth_type != "ai_studio") {
            Some(
                crate::oauth::initialize_google_account(
                    &state.client,
                    &token.access_token,
                    provider,
                    &project_id,
                )
                .await?,
            )
        } else {
            None
        };
    if let Some(context) = &google_context {
        project_id = context.project_id.clone();
        tier_id = context.tier_id.clone();
        privacy_set = context.privacy_set;
    }
    if provider == "gemini" {
        tier_id = match token.oauth_type.as_str() {
            "google_one" if tier_id.is_empty() || tier_id == "gcp_standard" => {
                "google_one_free".into()
            }
            "ai_studio" if tier_id.is_empty() => "aistudio_free".into(),
            _ => tier_id,
        };
    }
    let platform = provider.to_owned();
    let base_url = match provider {
        "anthropic" => "https://api.anthropic.com",
        "gemini" if token.oauth_type == "ai_studio" => "https://generativelanguage.googleapis.com",
        "gemini" => google_context
            .as_ref()
            .map(|context| context.base_url.as_str())
            .unwrap_or("https://cloudcode-pa.googleapis.com"),
        "grok" => "https://cli-chat-proxy.grok.com/v1",
        "antigravity" => "https://cloudcode-pa.googleapis.com",
        _ => return Err(AppError::BadRequest("不支持的 OAuth 平台".into())),
    };
    let credentials = json!({
        "access_token":token.access_token, "refresh_token":token.refresh_token, "id_token":token.id_token,
        "token_type":token.token_type, "expires_at":expires_at, "scope":token.scope,
        "client_id":token.client_id, "client_secret":token.client_secret, "oauth_type":token.oauth_type,
        "project_id":project_id, "tier_id":tier_id, "base_url":base_url, "email":email,
        "sub":subject,
        "org_uuid":token.organization.get("uuid").and_then(Value::as_str).unwrap_or_default(),
        "account_uuid":token.account.get("uuid").and_then(Value::as_str).unwrap_or_default(),
    });
    let credentials = secrets::protect_credentials(&state.config.encryption_key, &credentials)?;
    let display = platform_label(provider);
    let fallback_name = if !email.is_empty() {
        email.clone()
    } else if !identity_name.is_empty() {
        identity_name
    } else {
        format!("{display} OAuth")
    };
    let account_name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(&fallback_name)
        .to_owned();
    let extra = json!({"email":email,"privacy_mode":if privacy_set {"privacy_set"} else if provider == "antigravity" {"privacy_set_failed"} else {"unknown"}});
    if let Some(result) = update_oauth_account_if_requested(
        state,
        uid,
        payload,
        &platform,
        &account_name,
        &credentials,
        &extra,
    )
    .await?
    {
        return Ok(result);
    }
    let sub2_user_id = Sub2Repository::new(state.sub2_pool.clone())
        .sub2_user_id(&state.config.sub2api_admin_email)
        .await?;
    create_or_pool(
        state,
        uid,
        payload.get("target_provider_id").and_then(Value::as_i64),
        display,
        CreateBundleInput {
            sub2_user_id,
            group_name: format!("JunoHub {uid} {display} OAuth {}", crate::store::now()),
            account_name,
            platform,
            kind: "oauth".into(),
            credentials,
            extra,
            concurrency: if provider == "grok" { 1 } else { 3 },
            priority: 50,
        },
    )
    .await
}

async fn update_oauth_account_if_requested(
    state: &AppState,
    uid: i64,
    payload: &Value,
    platform: &str,
    account_name: &str,
    credentials: &Value,
    extra: &Value,
) -> AppResult<Option<Value>> {
    let provider_id = payload.get("provider_id").and_then(Value::as_i64);
    let account_id = payload.get("account_id").and_then(Value::as_i64);
    let ids = match (provider_id, account_id) {
        (None, None) => return Ok(None),
        (Some(provider_id), Some(account_id)) if provider_id > 0 && account_id > 0 => {
            (provider_id, account_id)
        }
        _ => {
            return Err(AppError::BadRequest(
                "编辑 OAuth 账户时必须同时提供 provider_id 和 account_id".into(),
            ))
        }
    };
    let group_id: i64 = sqlx::query_scalar(
        "SELECT group_id FROM sub2_managed_pools WHERE provider_id=? AND user_id=? AND platform=?",
    )
    .bind(ids.0)
    .bind(uid)
    .bind(platform)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("内置供应商不存在".into()))?;
    Sub2Repository::new(state.sub2_pool.clone())
        .replace_managed_oauth_credentials(
            group_id,
            ids.1,
            platform,
            account_name,
            credentials.clone(),
            extra.clone(),
        )
        .await?;
    Ok(Some(
        json!({"provider_id":ids.0,"account_id":ids.1,"updated":true}),
    ))
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let body = token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()
}

pub async fn create_api_key_account(
    state: &AppState,
    uid: i64,
    payload: &Value,
) -> AppResult<Value> {
    let platform = payload
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if !matches!(
        platform.as_str(),
        "openai" | "anthropic" | "gemini" | "grok" | "kimi" | "zhipu" | "deepseek"
    ) {
        return Err(AppError::BadRequest("不支持的账户平台".into()));
    }
    let api_key = payload
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if api_key.is_empty() || api_key.len() > 8192 {
        return Err(AppError::BadRequest("请输入有效 API Key".into()));
    }
    let base_url = payload
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_else(|| default_base_url(&platform))
        .trim();
    let url = Url::parse(base_url).map_err(|_| AppError::BadRequest("Base URL 无效".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AppError::BadRequest("Base URL 必须是 HTTP(S) 地址".into()));
    }
    let display = platform_label(&platform);
    let account_name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(display);
    let sub2_user_id = Sub2Repository::new(state.sub2_pool.clone())
        .sub2_user_id(&state.config.sub2api_admin_email)
        .await?;
    create_or_pool(
        state,
        uid,
        payload.get("target_provider_id").and_then(Value::as_i64),
        display,
        CreateBundleInput {
            sub2_user_id,
            group_name: format!("JunoHub {uid} {display} {}", crate::store::now()),
            account_name: account_name.to_owned(),
            platform: platform.clone(),
            kind: "apikey".into(),
            credentials: secrets::protect_credentials(
                &state.config.encryption_key,
                &json!({"api_key":api_key,"base_url":base_url}),
            )?,
            extra: json!({}),
            concurrency: 1,
            priority: if platform == "openai" { 60 } else { 50 },
        },
    )
    .await
}

async fn create_or_pool(
    state: &AppState,
    uid: i64,
    target_provider_id: Option<i64>,
    provider_name: &str,
    input: CreateBundleInput,
) -> AppResult<Value> {
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let existing = sqlx::query(
        "SELECT m.provider_id,m.group_id FROM sub2_managed_pools m JOIN user_providers p ON p.id=m.provider_id AND p.user_id=m.user_id WHERE m.user_id=? AND m.platform=? AND (? IS NULL OR m.provider_id=?) ORDER BY m.provider_id LIMIT 1",
    )
    .bind(uid)
    .bind(&input.platform)
    .bind(target_provider_id)
    .bind(target_provider_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(existing) = existing {
        let provider_id: i64 = existing.get("provider_id");
        let group_id: i64 = existing.get("group_id");
        let account_id = repository.add_account_to_group(group_id, input).await?;
        return Ok(json!({"provider_id":provider_id,"account_id":account_id,"pooled":true}));
    }
    if target_provider_id.is_some() {
        return Err(AppError::NotFound("目标供应商不存在或平台不匹配".into()));
    }
    let platform = input.platform.clone();
    let bundle = repository.create_bundle(input).await?;
    finish_provisioning(state, uid, provider_name, &platform, repository, bundle).await
}

async fn finish_provisioning(
    state: &AppState,
    uid: i64,
    provider_name: &str,
    platform: &str,
    repository: Sub2Repository,
    bundle: crate::sub2_repository::CreatedBundle,
) -> AppResult<Value> {
    let result = match crate::console::provision_sub2api_provider(
        state,
        uid,
        provider_name,
        &bundle.api_key,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = repository.delete_bundle(&bundle).await;
            return Err(error);
        }
    };
    let provider_id = result
        .get("provider_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::Internal("供应商创建结果缺少 provider_id".into()))?;
    let timestamp = crate::store::now();
    if let Err(error) = sqlx::query(
        "INSERT INTO sub2_managed_pools(provider_id,user_id,platform,group_id,api_key_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
    )
    .bind(provider_id)
    .bind(uid)
    .bind(platform)
    .bind(bundle.group_id)
    .bind(bundle.api_key_id)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&state.pool)
    .await
    {
        let _ = sqlx::query("DELETE FROM user_providers WHERE id=? AND user_id=?")
            .bind(provider_id)
            .bind(uid)
            .execute(&state.pool)
            .await;
        let _ = repository.delete_bundle(&bundle).await;
        return Err(error.into());
    }
    Ok(result)
}

fn default_base_url(platform: &str) -> &'static str {
    match platform {
        "openai" => "https://api.openai.com/v1",
        "anthropic" => "https://api.anthropic.com",
        "gemini" => "https://generativelanguage.googleapis.com",
        "grok" => "https://api.x.ai/v1",
        "kimi" => "https://api.moonshot.cn/v1",
        "zhipu" => "https://open.bigmodel.cn/api/paas/v4",
        "deepseek" => "https://api.deepseek.com",
        _ => "https://example.com/v1",
    }
}

fn platform_label(platform: &str) -> &'static str {
    match platform {
        "openai" => "OpenAI",
        "anthropic" => "Claude",
        "gemini" => "Gemini",
        "grok" => "Grok",
        "antigravity" => "Antigravity",
        "kimi" => "Kimi",
        "zhipu" => "智谱 GLM",
        "deepseek" => "DeepSeek",
        _ => "AI Service",
    }
}
