use reqwest::Url;
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
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
    let token = state
        .oauth_sessions
        .exchange_openai(&state.client, uid, session_id, code, oauth_state)
        .await?;
    let (credentials, extra, default_name) = token.account_payload();
    let account_name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&default_name)
        .to_owned();
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let sub2_user_id = repository
        .sub2_user_id(&state.config.sub2api_admin_email)
        .await?;
    let bundle = repository
        .create_bundle(CreateBundleInput {
            sub2_user_id,
            group_name: format!("JunoHub {uid} OpenAI OAuth {}", crate::store::now()),
            account_name,
            platform: "openai".into(),
            kind: "oauth".into(),
            credentials,
            extra,
            concurrency: 3,
            priority: 50,
        })
        .await?;
    finish_provisioning(state, uid, "Sub2API · OpenAI Codex", repository, bundle).await
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
        "openai" | "anthropic" | "gemini" | "grok" | "antigravity" | "kimi" | "zhipu" | "deepseek"
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
    let repository = Sub2Repository::new(state.sub2_pool.clone());
    let sub2_user_id = repository
        .sub2_user_id(&state.config.sub2api_admin_email)
        .await?;
    let bundle = repository
        .create_bundle(CreateBundleInput {
            sub2_user_id,
            group_name: format!("JunoHub {uid} {display} {}", crate::store::now()),
            account_name: account_name.to_owned(),
            platform: platform.clone(),
            kind: "apikey".into(),
            credentials: json!({"api_key":api_key,"base_url":base_url}),
            extra: json!({}),
            concurrency: 1,
            priority: 50,
        })
        .await?;
    finish_provisioning(
        state,
        uid,
        &format!("Sub2API · {display}"),
        repository,
        bundle,
    )
    .await
}

async fn finish_provisioning(
    state: &AppState,
    uid: i64,
    provider_name: &str,
    repository: Sub2Repository,
    bundle: crate::sub2_repository::CreatedBundle,
) -> AppResult<Value> {
    let result = async {
        let models = gateway_models(state, &bundle.api_key).await?;
        crate::console::provision_sub2api_provider(
            state,
            uid,
            provider_name,
            &bundle.api_key,
            &models,
        )
        .await
    }
    .await;
    if result.is_err() {
        let _ = repository.delete_bundle(&bundle).await;
    }
    result
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

async fn gateway_models(state: &AppState, api_key: &str) -> AppResult<Vec<String>> {
    let models = Sub2Repository::new(state.sub2_pool.clone())
        .model_ids_for_api_key(api_key)
        .await?;
    if models.is_empty() {
        return Err(AppError::BadRequest("账户没有返回可用模型".into()));
    }
    Ok(models)
}
