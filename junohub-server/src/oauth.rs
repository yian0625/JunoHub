use std::{collections::HashMap, sync::Arc, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;
use url::Url;

use crate::{
    error::{AppError, AppResult},
    secrets, store,
};

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OPENAI_SCOPE: &str = "openid profile email offline_access";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const CLAUDE_SCOPE: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const GEMINI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const GEMINI_CLIENT_SECRET: &str = "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";
const GEMINI_REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
const GEMINI_LOCAL_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const GEMINI_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";
const GEMINI_AI_STUDIO_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/generative-language.retriever";
const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
pub const ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:8085/callback";
const ANTIGRAVITY_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";
const GROK_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const GROK_AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const GROK_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const GROK_REDIRECT_URI: &str = "http://127.0.0.1:56121/callback";
const GROK_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OAuthSession {
    owner_id: i64,
    state: String,
    code_verifier: String,
    client_id: String,
    redirect_uri: String,
    provider: String,
    client_secret: String,
    oauth_type: String,
    project_id: String,
    created_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderTokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub organization: Value,
    #[serde(default)]
    pub account: Value,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub oauth_type: String,
    #[serde(default)]
    pub project_id: String,
}

fn default_token_type() -> String {
    "Bearer".into()
}

#[derive(Clone, Debug, Serialize)]
pub struct OAuthStart {
    pub auth_url: String,
    pub session_id: String,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenAITokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    pub token_type: String,
    pub expires_in: i64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub client_id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OpenAIIDTokenClaims {
    #[serde(default)]
    email: String,
    #[serde(default, rename = "https://api.openai.com/auth")]
    auth: Option<OpenAIAuthClaims>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OpenAIAuthClaims {
    #[serde(default)]
    chatgpt_account_id: String,
    #[serde(default)]
    chatgpt_user_id: String,
    #[serde(default)]
    chatgpt_plan_type: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    organizations: Vec<OpenAIOrganization>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OpenAIOrganization {
    #[serde(default)]
    id: String,
    #[serde(default)]
    is_default: bool,
}

impl OpenAITokenResponse {
    pub fn account_payload(&self) -> (Value, Value, String) {
        let claims = decode_id_token(&self.id_token).unwrap_or_default();
        let auth = claims.auth.unwrap_or_default();
        let organization_id = auth
            .organizations
            .iter()
            .find(|organization| organization.is_default)
            .or_else(|| auth.organizations.first())
            .map(|organization| organization.id.clone())
            .unwrap_or_default();
        let expires_at = Utc::now()
            .timestamp()
            .saturating_add(self.expires_in.saturating_sub(300).max(60));
        let credentials = json!({
            "access_token":self.access_token,
            "refresh_token":self.refresh_token,
            "id_token":self.id_token,
            "token_type":self.token_type,
            "expires_at":expires_at,
            "client_id":self.client_id,
            "auth_mode":"oauth",
            "email":claims.email,
            "chatgpt_account_id":auth.chatgpt_account_id,
            "chatgpt_user_id":auth.chatgpt_user_id,
            "user_id":auth.user_id,
            "organization_id":organization_id,
            "plan_type":auth.chatgpt_plan_type,
        });
        let extra = json!({
            "email":claims.email,
            "privacy_mode":"training_off",
        });
        let display_name = if claims.email.trim().is_empty() {
            "OpenAI Codex".into()
        } else {
            claims.email
        };
        (credentials, extra, display_name)
    }
}

pub async fn refresh_openai_token(
    client: &reqwest::Client,
    refresh_token: &str,
    client_id: Option<&str>,
) -> AppResult<OpenAITokenResponse> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err(AppError::BadRequest("OpenAI refresh_token 不能为空".into()));
    }
    let client_id = client_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(OPENAI_CLIENT_ID);
    let response = client
        .post(OPENAI_TOKEN_URL)
        .header(
            "User-Agent",
            "codex-tui/0.200.1 (Ubuntu 22.4.0; x86_64) xterm-256color",
        )
        .header("originator", "codex-tui")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::BadRequest(format!(
            "OpenAI Token 刷新失败，HTTP {}",
            status.as_u16()
        )));
    }
    let mut token = response.json::<OpenAITokenResponse>().await?;
    if token.access_token.trim().is_empty() {
        return Err(AppError::BadRequest(
            "OpenAI 刷新未返回 access_token".into(),
        ));
    }
    if token.refresh_token.trim().is_empty() {
        token.refresh_token = refresh_token.to_owned();
    }
    token.client_id = client_id.to_owned();
    Ok(token)
}

pub async fn refresh_provider_token(
    client: &reqwest::Client,
    provider: &str,
    refresh_token: &str,
    credentials: &Value,
) -> AppResult<ProviderTokenResponse> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{provider} refresh_token 不能为空"
        )));
    }
    let (token_url, default_client_id, json_body) = match provider {
        "anthropic" => (CLAUDE_TOKEN_URL, CLAUDE_CLIENT_ID, true),
        "gemini" => (GOOGLE_TOKEN_URL, GEMINI_CLIENT_ID, false),
        "antigravity" => (GOOGLE_TOKEN_URL, ANTIGRAVITY_CLIENT_ID, false),
        "grok" => (GROK_TOKEN_URL, GROK_CLIENT_ID, false),
        _ => return Err(AppError::BadRequest("不支持的 OAuth 平台".into())),
    };
    let client_id = credentials
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(default_client_id);
    let client_secret = credentials
        .get("client_secret")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut attempt = 0_u32;
    let response = loop {
        let sent = if json_body {
            client.post(token_url).header("User-Agent", "axios/1.13.6").json(&json!({"grant_type":"refresh_token","refresh_token":refresh_token,"client_id":client_id})).send().await
        } else {
            let mut form = vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
            ];
            if !client_secret.is_empty() {
                form.push(("client_secret", client_secret));
            }
            client
                .post(token_url)
                .header("User-Agent", "JunoHub OAuth/1.0")
                .form(&form)
                .send()
                .await
        };
        match sent {
            Ok(response)
                if attempt < 3
                    && (response.status().as_u16() == 429
                        || response.status().is_server_error()) =>
            {
                tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                attempt += 1;
            }
            Ok(response) => break response,
            Err(_) if attempt < 3 => {
                tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                attempt += 1;
            }
            Err(error) => return Err(AppError::Http(error)),
        }
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::BadRequest(format!(
            "{provider} Token 刷新失败，HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        )));
    }
    let mut token = response.json::<ProviderTokenResponse>().await?;
    if token.access_token.trim().is_empty() {
        return Err(AppError::BadRequest(format!(
            "{provider} 刷新未返回 access_token"
        )));
    }
    if token.refresh_token.trim().is_empty() {
        token.refresh_token = refresh_token.to_owned();
    }
    token.provider = provider.into();
    token.client_id = client_id.into();
    token.client_secret = client_secret.into();
    Ok(token)
}

pub async fn google_identity(client: &reqwest::Client, access_token: &str) -> (String, String) {
    let Ok(response) = client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
    else {
        return (String::new(), String::new());
    };
    let Ok(value) = response.json::<Value>().await else {
        return (String::new(), String::new());
    };
    (
        value
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

#[derive(Clone, Debug)]
pub struct GoogleAccountContext {
    pub project_id: String,
    pub tier_id: String,
    pub base_url: String,
    pub privacy_set: bool,
}

pub async fn initialize_google_account(
    client: &reqwest::Client,
    access_token: &str,
    provider: &str,
    requested_project: &str,
) -> AppResult<GoogleAccountContext> {
    let antigravity = provider == "antigravity";
    let bases: &[&str] = if antigravity {
        &[
            "https://cloudcode-pa.googleapis.com",
            "https://daily-cloudcode-pa.googleapis.com",
        ]
    } else {
        &["https://cloudcode-pa.googleapis.com"]
    };
    initialize_google_account_with_bases(client, access_token, provider, requested_project, bases)
        .await
}

async fn initialize_google_account_with_bases(
    client: &reqwest::Client,
    access_token: &str,
    provider: &str,
    requested_project: &str,
    bases: &[&str],
) -> AppResult<GoogleAccountContext> {
    let antigravity = provider == "antigravity";
    let user_agent = if antigravity {
        "antigravity/1.23.2 windows/amd64"
    } else {
        "GeminiCLI/0.1.5 (Windows; AMD64)"
    };
    let metadata = if antigravity {
        json!({"ideType":"ANTIGRAVITY","ideVersion":"1.23.2","ideName":"antigravity"})
    } else {
        json!({"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"})
    };
    let mut project_id = requested_project.trim().to_owned();
    let mut tier_id = String::new();
    let mut onboarding_tier = None;
    let mut initialization_error = None;
    let mut selected_base = bases[0].to_owned();
    for base in bases {
        for attempt in 0..3 {
            let response = client
                .post(format!("{base}/v1internal:loadCodeAssist"))
                .bearer_auth(access_token)
                .header("User-Agent", user_agent)
                .json(&json!({"metadata":metadata}))
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let value = response.json::<Value>().await.unwrap_or(Value::Null);
                    if antigravity {
                        validate_antigravity_eligibility(&value)?;
                    } else {
                        validate_gemini_eligibility(&value)?;
                    }
                    onboarding_tier = google_onboarding_tier(&value, antigravity);
                    if project_id.is_empty() {
                        project_id = extract_google_project(&value).unwrap_or_default();
                    }
                    tier_id = extract_google_tier(&value).unwrap_or_default();
                    selected_base = (*base).to_owned();
                    break;
                }
                Ok(response)
                    if (response.status().as_u16() == 429
                        || response.status().is_server_error())
                        && attempt < 2 =>
                {
                    tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                }
                Err(_) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                }
                Ok(response) => {
                    initialization_error = Some(format!(
                        "loadCodeAssist 返回 HTTP {}",
                        response.status().as_u16()
                    ));
                    break;
                }
                Err(_) => {
                    initialization_error = Some("loadCodeAssist 网络请求失败".into());
                    break;
                }
            }
        }
        if !project_id.is_empty() {
            break;
        }
    }
    if let Some(onboarding_tier) = onboarding_tier.filter(|_| project_id.is_empty()) {
        for base in bases {
            for attempt in 0..5 {
                let response = client
                    .post(format!("{base}/v1internal:onboardUser"))
                    .bearer_auth(access_token)
                    .header("User-Agent", user_agent)
                    .json(&json!({
                        "tierId":onboarding_tier,
                        "metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}
                    }))
                    .send()
                    .await;
                if let Ok(response) = response {
                    if response.status().is_success() {
                        let value = response.json::<Value>().await.unwrap_or(Value::Null);
                        if value.get("done").and_then(Value::as_bool) == Some(true) {
                            project_id = value
                                .get("response")
                                .and_then(extract_google_project)
                                .unwrap_or_default();
                            if !project_id.is_empty() {
                                selected_base = (*base).to_owned();
                                break;
                            }
                            initialization_error = Some("onboardUser 已完成，但未返回项目".into());
                        }
                    } else {
                        initialization_error = Some(format!(
                            "onboardUser 返回 HTTP {}",
                            response.status().as_u16()
                        ));
                    }
                } else {
                    initialization_error = Some("onboardUser 网络请求失败".into());
                }
                if attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt.min(3)))).await;
                }
            }
            if !project_id.is_empty() {
                break;
            }
        }
    }
    if project_id.is_empty() && !antigravity {
        project_id = discover_active_google_cloud_project(client, access_token).await;
    }
    if project_id.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{} 账户初始化未完成：{}。未获取到可用 Project ID",
            if antigravity { "Antigravity" } else { "Gemini" },
            initialization_error.unwrap_or_else(|| "Google 未返回已分配项目或可初始化等级".into())
        )));
    }
    let privacy_set = if antigravity {
        // Upstream records privacy failure independently of OAuth provisioning.
        set_antigravity_privacy(client, access_token, &project_id)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    Ok(GoogleAccountContext {
        project_id,
        tier_id: canonical_google_tier(provider, &tier_id),
        base_url: selected_base,
        privacy_set,
    })
}

async fn discover_active_google_cloud_project(
    client: &reqwest::Client,
    access_token: &str,
) -> String {
    let Ok(response) = client
        .get("https://cloudresourcemanager.googleapis.com/v1/projects")
        .bearer_auth(access_token)
        .header("User-Agent", "GeminiCLI/0.1.5 (Windows; AMD64)")
        .send()
        .await
    else {
        return String::new();
    };
    let Ok(value) = response.json::<Value>().await else {
        return String::new();
    };
    value
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|project| project.get("lifecycleState").and_then(Value::as_str) == Some("ACTIVE"))
        .and_then(|project| project.get("projectId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

async fn set_antigravity_privacy(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
) -> AppResult<bool> {
    let response = client
        .post("https://daily-cloudcode-pa.googleapis.com/v1internal:setUserSettings")
        .timeout(Duration::from_secs(10))
        .bearer_auth(access_token)
        .header("User-Agent", "antigravity/1.23.2 windows/amd64")
        .header("X-Goog-Api-Client", "gl-node/22.21.1")
        .json(&json!({"user_settings":{}}))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "Antigravity 隐私设置失败，HTTP {}",
            response.status().as_u16()
        )));
    }
    let value = response.json::<Value>().await.unwrap_or(Value::Null);
    let set = value
        .get("userSettings")
        .and_then(Value::as_object)
        .is_none_or(|settings| !settings.contains_key("telemetryEnabled"));
    if !set {
        return Ok(false);
    }
    let response = client
        .post("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo")
        .timeout(Duration::from_secs(10))
        .bearer_auth(access_token)
        .header("User-Agent", "antigravity/1.23.2 windows/amd64")
        .header("X-Goog-Api-Client", "gl-node/22.21.1")
        .json(&json!({"project":project_id}))
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(false);
    }
    let value: Value = response.json().await?;
    Ok(value
        .get("userSettings")
        .and_then(Value::as_object)
        .is_none_or(|settings| !settings.contains_key("telemetryEnabled")))
}

fn has_registered_google_tier(value: &Value) -> bool {
    ["currentTier", "paidTier"].iter().any(|key| {
        value
            .get(key)
            .and_then(|tier| {
                tier.as_str()
                    .or_else(|| tier.get("id").and_then(Value::as_str))
            })
            .is_some_and(|tier| !tier.trim().is_empty())
    })
}

fn validate_antigravity_eligibility(value: &Value) -> AppResult<()> {
    if !has_registered_google_tier(value)
        && value
            .get("ineligibleTiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier["reasonCode"] == "UNSUPPORTED_LOCATION")
            })
    {
        return Err(AppError::BadRequest("Google 已返回授权凭证，但当前账户因地区资格限制无法使用 Antigravity（UNSUPPORTED_LOCATION），未分配可用项目。重复授权或手动填写 Project ID 不能解除此限制。".into()));
    }
    Ok(())
}

pub(crate) fn validate_gemini_eligibility(value: &Value) -> AppResult<()> {
    if !has_registered_google_tier(value)
        && value
            .get("ineligibleTiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier["reasonCode"] == "UNSUPPORTED_CLIENT")
            })
    {
        return Err(AppError::BadRequest("Google 已停止支持此客户端接入个人版 Gemini Code Assist（UNSUPPORTED_CLIENT）。重复授权不能恢复此入口；请按 Google 提示使用 Antigravity。".into()));
    }
    Ok(())
}

fn google_onboarding_tier(value: &Value, antigravity: bool) -> Option<String> {
    if !antigravity && has_registered_google_tier(value) {
        return None;
    }
    let tiers = value.get("allowedTiers").and_then(Value::as_array);
    let tier = tiers.and_then(|tiers| {
        tiers.iter().find(|tier| {
            tier["isDefault"] == true && tier["id"].as_str().is_some_and(|id| !id.trim().is_empty())
        })
    });
    let tier = tier.or_else(|| {
        if antigravity {
            None
        } else {
            tiers.and_then(|tiers| {
                tiers
                    .iter()
                    .find(|tier| tier["id"].as_str().is_some_and(|id| !id.trim().is_empty()))
            })
        }
    });
    tier.and_then(|tier| tier["id"].as_str())
        .map(|id| id.trim().to_owned())
        .or_else(|| {
            if antigravity {
                None
            } else {
                Some("LEGACY".into())
            }
        })
}

fn extract_google_project(value: &Value) -> Option<String> {
    let candidate = value.get("cloudaicompanionProject")?;
    candidate
        .as_str()
        .or_else(|| candidate.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn extract_google_tier(value: &Value) -> Option<String> {
    for pointer in ["/paidTier/id", "/currentTier/id"] {
        if let Some(tier) = value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(tier.to_owned());
        }
    }
    for name in ["paidTier", "currentTier"] {
        if let Some(tier) = value
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(tier.to_owned());
        }
    }
    value
        .get("allowedTiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|tier| tier.get("isDefault").and_then(Value::as_bool) == Some(true))
        .or_else(|| {
            value
                .get("allowedTiers")
                .and_then(Value::as_array)
                .and_then(|tiers| tiers.first())
        })
        .and_then(|tier| tier.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn canonical_google_tier(provider: &str, value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if provider == "antigravity" {
        return if value.is_empty() {
            "free-tier".into()
        } else {
            value
        };
    }
    match value.as_str() {
        "enterprise" | "ultra" | "ultra-tier" => "gcp_enterprise".into(),
        "" | "legacy" | "standard" | "pro" | "standard-tier" | "pro-tier" => "gcp_standard".into(),
        _ => value,
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct OAuthExchangeSession {
    pub code_verifier: String,
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Clone)]
pub struct OAuthSessionStore {
    sessions: Arc<RwLock<HashMap<String, OAuthSession>>>,
    persistence: Option<OAuthSessionPersistence>,
}

#[derive(Clone)]
struct OAuthSessionPersistence {
    pool: SqlitePool,
    encryption_key: Arc<str>,
}

impl Default for OAuthSessionStore {
    fn default() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: None,
        }
    }
}

impl OAuthSessionStore {
    pub fn persistent(pool: SqlitePool, encryption_key: impl Into<Arc<str>>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: Some(OAuthSessionPersistence {
                pool,
                encryption_key: encryption_key.into(),
            }),
        }
    }

    async fn save(&self, session_id: &str, session: OAuthSession) -> AppResult<()> {
        if let Some(persistence) = &self.persistence {
            let serialized = serde_json::to_string(&session)?;
            let encrypted = secrets::encrypt(&persistence.encryption_key, &serialized)?;
            let expires_at = session
                .created_at
                .saturating_add(SESSION_TTL.as_secs() as i64);
            sqlx::query(
                "INSERT INTO oauth_sessions(session_hash,owner_id,provider,state_hash,encrypted_payload,expires_at,created_at) VALUES(?,?,?,?,?,?,?) \
                 ON CONFLICT(session_hash) DO UPDATE SET owner_id=excluded.owner_id,provider=excluded.provider,state_hash=excluded.state_hash,encrypted_payload=excluded.encrypted_payload,expires_at=excluded.expires_at,created_at=excluded.created_at",
            )
            .bind(store::hash_key(session_id))
            .bind(session.owner_id)
            .bind(&session.provider)
            .bind(store::hash_key(&session.state))
            .bind(encrypted)
            .bind(expires_at)
            .bind(session.created_at)
            .execute(&persistence.pool)
            .await?;
        }
        self.sessions
            .write()
            .await
            .insert(session_id.to_owned(), session);
        Ok(())
    }

    async fn load(&self, session_id: &str) -> AppResult<OAuthSession> {
        if let Some(persistence) = &self.persistence {
            let row = sqlx::query(
                "SELECT encrypted_payload FROM oauth_sessions WHERE session_hash=? AND expires_at>=?",
            )
            .bind(store::hash_key(session_id))
            .bind(store::now())
            .fetch_optional(&persistence.pool)
            .await?
            .ok_or_else(|| AppError::BadRequest("OAuth 会话不存在或已过期，请重新生成授权链接；重新打开旧授权页不能恢复会话。".into()))?;
            let encrypted: String = row.get("encrypted_payload");
            let serialized = secrets::decrypt(&persistence.encryption_key, &encrypted)?;
            return serde_json::from_str(&serialized)
                .map_err(|_| AppError::Internal("OAuth 会话数据损坏".into()));
        }
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .filter(|session| {
                store::now().saturating_sub(session.created_at) <= SESSION_TTL.as_secs() as i64
            })
            .ok_or_else(|| AppError::BadRequest("OAuth 会话不存在或已过期".into()))
    }

    async fn consume(
        &self,
        owner_id: i64,
        provider: &str,
        session_id: &str,
        state: &str,
    ) -> AppResult<OAuthSession> {
        if state.is_empty() {
            return Err(AppError::BadRequest("OAuth state 无效".into()));
        }
        if let Some(persistence) = &self.persistence {
            let row = sqlx::query(
                "DELETE FROM oauth_sessions WHERE session_hash=? AND owner_id=? AND provider=? AND state_hash=? AND expires_at>=? RETURNING encrypted_payload",
            )
            .bind(store::hash_key(session_id))
            .bind(owner_id)
            .bind(provider)
            .bind(store::hash_key(state))
            .bind(store::now())
            .fetch_optional(&persistence.pool)
            .await?
            .ok_or_else(|| AppError::BadRequest("OAuth 会话不存在、已过期或校验失败".into()))?;
            self.sessions.write().await.remove(session_id);
            let encrypted: String = row.get("encrypted_payload");
            let serialized = secrets::decrypt(&persistence.encryption_key, &encrypted)?;
            return serde_json::from_str(&serialized)
                .map_err(|_| AppError::Internal("OAuth 会话数据损坏".into()));
        }
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::BadRequest("OAuth 会话不存在或已过期".into()))?;
        if session.owner_id != owner_id
            || session.provider != provider
            || store::now().saturating_sub(session.created_at) > SESSION_TTL.as_secs() as i64
            || !constant_time_eq(state.as_bytes(), session.state.as_bytes())
        {
            return Err(AppError::BadRequest(
                "OAuth 会话不存在、已过期或校验失败".into(),
            ));
        }
        sessions.remove(session_id);
        Ok(session)
    }

    pub async fn validate_session(
        &self,
        owner_id: i64,
        provider: &str,
        session_id: &str,
        state: &str,
    ) -> AppResult<()> {
        let session = self.load(session_id).await?;
        if state.is_empty()
            || session.owner_id != owner_id
            || session.provider != provider
            || !constant_time_eq(state.as_bytes(), session.state.as_bytes())
        {
            return Err(AppError::BadRequest(
                "OAuth 回调与当前用户、平台或授权会话不匹配，请使用本次生成的链接。".into(),
            ));
        }
        Ok(())
    }

    pub async fn start_openai(
        &self,
        owner_id: i64,
        redirect_uri: Option<&str>,
    ) -> AppResult<OAuthStart> {
        if owner_id <= 0 {
            return Err(AppError::Unauthorized);
        }
        self.remove_expired().await;
        let state = random_hex(32);
        let session_id = random_hex(16);
        let code_verifier = random_hex(64);
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let redirect_uri = redirect_uri
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(OPENAI_REDIRECT_URI)
            .to_owned();
        validate_redirect_uri(&redirect_uri)?;
        let mut url = Url::parse(OPENAI_AUTHORIZE_URL)
            .map_err(|_| AppError::Internal("OpenAI OAuth URL 无效".into()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", OPENAI_CLIENT_ID)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", OPENAI_SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true");
        self.save(
            &session_id,
            OAuthSession {
                owner_id,
                state: state.clone(),
                code_verifier,
                client_id: OPENAI_CLIENT_ID.into(),
                redirect_uri,
                provider: "openai".into(),
                client_secret: String::new(),
                oauth_type: String::new(),
                project_id: String::new(),
                created_at: store::now(),
            },
        )
        .await?;
        Ok(OAuthStart {
            auth_url: url.into(),
            session_id,
            state,
        })
    }

    pub async fn start_provider(
        &self,
        owner_id: i64,
        provider: &str,
        options: &Value,
    ) -> AppResult<OAuthStart> {
        if provider == "openai" {
            return self.start_openai(owner_id, None).await;
        }
        if owner_id <= 0 {
            return Err(AppError::Unauthorized);
        }
        self.remove_expired().await;
        let state = random_hex(32);
        let session_id = random_hex(16);
        let code_verifier = URL_SAFE_NO_PAD.encode(random_bytes(32));
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let oauth_type = options
            .get("oauth_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let project_id = options
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let (authorize_url, client_id, client_secret, redirect_uri, scope) = match provider {
            "anthropic" => (
                CLAUDE_AUTHORIZE_URL,
                CLAUDE_CLIENT_ID.to_owned(),
                String::new(),
                CLAUDE_REDIRECT_URI.to_owned(),
                CLAUDE_SCOPE.to_owned(),
            ),
            "gemini" => {
                if oauth_type == "ai_studio" {
                    let client_id = option_required(
                        options,
                        "client_id",
                        "AI Studio OAuth Client ID 不能为空",
                    )?;
                    let client_secret = option_required(
                        options,
                        "client_secret",
                        "AI Studio OAuth Client Secret 不能为空",
                    )?;
                    (
                        GOOGLE_AUTHORIZE_URL,
                        client_id,
                        client_secret,
                        GEMINI_LOCAL_REDIRECT_URI.to_owned(),
                        GEMINI_AI_STUDIO_SCOPE.to_owned(),
                    )
                } else {
                    (
                        GOOGLE_AUTHORIZE_URL,
                        GEMINI_CLIENT_ID.to_owned(),
                        GEMINI_CLIENT_SECRET.to_owned(),
                        GEMINI_REDIRECT_URI.to_owned(),
                        GEMINI_SCOPE.to_owned(),
                    )
                }
            }
            "antigravity" => (
                GOOGLE_AUTHORIZE_URL,
                ANTIGRAVITY_CLIENT_ID.to_owned(),
                ANTIGRAVITY_CLIENT_SECRET.to_owned(),
                ANTIGRAVITY_REDIRECT_URI.to_owned(),
                ANTIGRAVITY_SCOPE.to_owned(),
            ),
            "grok" => (
                GROK_AUTHORIZE_URL,
                GROK_CLIENT_ID.to_owned(),
                String::new(),
                GROK_REDIRECT_URI.to_owned(),
                GROK_SCOPE.to_owned(),
            ),
            _ => return Err(AppError::BadRequest("不支持的 OAuth 平台".into())),
        };
        let mut url = Url::parse(authorize_url)
            .map_err(|_| AppError::Internal("OAuth 授权地址无效".into()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &scope)
            .append_pair("state", &state)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256");
        match provider {
            "anthropic" => {
                url.query_pairs_mut().append_pair("code", "true");
            }
            "grok" => {
                url.query_pairs_mut()
                    .append_pair("nonce", &random_hex(16))
                    .append_pair("plan", "generic")
                    .append_pair("referrer", "sub2api");
            }
            _ => {
                url.query_pairs_mut()
                    .append_pair("access_type", "offline")
                    .append_pair("prompt", "consent")
                    .append_pair("include_granted_scopes", "true");
                if !project_id.is_empty() {
                    url.query_pairs_mut().append_pair("project_id", &project_id);
                }
            }
        }
        self.save(
            &session_id,
            OAuthSession {
                owner_id,
                state: state.clone(),
                code_verifier,
                client_id,
                redirect_uri,
                provider: provider.into(),
                client_secret,
                oauth_type,
                project_id,
                created_at: store::now(),
            },
        )
        .await?;
        Ok(OAuthStart {
            auth_url: url.into(),
            session_id,
            state,
        })
    }

    pub async fn exchange_provider(
        &self,
        client: &reqwest::Client,
        owner_id: i64,
        provider: &str,
        session_id: &str,
        raw_code: &str,
        state: &str,
    ) -> AppResult<ProviderTokenResponse> {
        self.remove_expired().await;
        let (code, embedded_state) = raw_code
            .trim()
            .split_once('#')
            .unwrap_or((raw_code.trim(), ""));
        let submitted_state = if embedded_state.is_empty() {
            state
        } else {
            embedded_state
        };
        if code.is_empty() {
            return Err(AppError::BadRequest("OAuth code 不能为空".into()));
        }
        if submitted_state.is_empty() {
            return Err(AppError::BadRequest("OAuth state 无效".into()));
        }
        let session = self
            .consume(owner_id, provider, session_id, submitted_state)
            .await?;
        let token_url = match provider {
            "anthropic" => CLAUDE_TOKEN_URL,
            "grok" => GROK_TOKEN_URL,
            "gemini" | "antigravity" => GOOGLE_TOKEN_URL,
            _ => return Err(AppError::BadRequest("不支持的 OAuth 平台".into())),
        };
        let response = if provider == "anthropic" {
            client.post(token_url).header("Accept", "application/json, text/plain, */*").header("User-Agent", "axios/1.13.6").json(&json!({
                "grant_type":"authorization_code", "client_id":session.client_id,
                "code":code, "redirect_uri":session.redirect_uri, "code_verifier":session.code_verifier
            })).send().await?
        } else {
            let mut form = vec![
                ("grant_type", "authorization_code".to_owned()),
                ("client_id", session.client_id.clone()),
                ("code", code.to_owned()),
                ("redirect_uri", session.redirect_uri.clone()),
                ("code_verifier", session.code_verifier.clone()),
            ];
            if !session.client_secret.is_empty() {
                form.push(("client_secret", session.client_secret.clone()));
            }
            client
                .post(token_url)
                .header("User-Agent", "JunoHub OAuth/1.0")
                .form(&form)
                .send()
                .await?
        };
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::BadRequest(format!(
                "{} OAuth 换取 Token 失败，HTTP {}: {}",
                provider,
                status.as_u16(),
                body.chars().take(300).collect::<String>()
            )));
        }
        let mut token = response.json::<ProviderTokenResponse>().await?;
        if token.access_token.trim().is_empty() {
            return Err(AppError::BadRequest("OAuth 未返回 access_token".into()));
        }
        token.provider = provider.into();
        token.client_id = session.client_id;
        token.client_secret = session.client_secret;
        token.oauth_type = if session.oauth_type.is_empty() {
            "code_assist".into()
        } else {
            session.oauth_type
        };
        token.project_id = session.project_id;
        Ok(token)
    }

    #[cfg(test)]
    pub async fn validate_openai(
        &self,
        owner_id: i64,
        session_id: &str,
        state: &str,
    ) -> AppResult<OAuthExchangeSession> {
        self.remove_expired().await;
        let session = self.load(session_id).await?;
        if session.owner_id != owner_id || session.provider != "openai" {
            return Err(AppError::Unauthorized);
        }
        if state.is_empty() || !constant_time_eq(state.as_bytes(), session.state.as_bytes()) {
            return Err(AppError::BadRequest("OAuth state 无效".into()));
        }
        Ok(OAuthExchangeSession {
            code_verifier: session.code_verifier,
            client_id: session.client_id,
            redirect_uri: session.redirect_uri,
        })
    }

    #[cfg(test)]
    pub async fn finish(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
        if let Some(persistence) = &self.persistence {
            if let Err(error) = sqlx::query("DELETE FROM oauth_sessions WHERE session_hash=?")
                .bind(store::hash_key(session_id))
                .execute(&persistence.pool)
                .await
            {
                tracing::warn!(%error, "failed to delete OAuth session");
            }
        }
    }

    pub async fn exchange_openai(
        &self,
        client: &reqwest::Client,
        owner_id: i64,
        session_id: &str,
        code: &str,
        state: &str,
    ) -> AppResult<OpenAITokenResponse> {
        if code.trim().is_empty() {
            return Err(AppError::BadRequest("OAuth code 不能为空".into()));
        }
        let session = self.consume(owner_id, "openai", session_id, state).await?;
        let response = client
            .post(OPENAI_TOKEN_URL)
            .header(
                "User-Agent",
                "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color",
            )
            .header("originator", "codex-tui")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", session.client_id.as_str()),
                ("code", code.trim()),
                ("redirect_uri", session.redirect_uri.as_str()),
                ("code_verifier", session.code_verifier.as_str()),
            ])
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(AppError::BadRequest(format!(
                "OpenAI OAuth 换取 Token 失败，HTTP {}: {}",
                status.as_u16(),
                message.chars().take(300).collect::<String>()
            )));
        }
        let mut token = response.json::<OpenAITokenResponse>().await?;
        if token.access_token.trim().is_empty() {
            return Err(AppError::BadRequest("OpenAI 未返回 access_token".into()));
        }
        token.client_id = session.client_id;
        Ok(token)
    }

    async fn remove_expired(&self) {
        let now = store::now();
        self.sessions.write().await.retain(|_, session| {
            now.saturating_sub(session.created_at) <= SESSION_TTL.as_secs() as i64
        });
        if let Some(persistence) = &self.persistence {
            if let Err(error) = sqlx::query("DELETE FROM oauth_sessions WHERE expires_at<?")
                .bind(now)
                .execute(&persistence.pool)
                .await
            {
                tracing::warn!(%error, "failed to remove expired OAuth sessions");
            }
        }
    }
}

fn decode_id_token(value: &str) -> Option<OpenAIIDTokenClaims> {
    let payload = value.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn validate_redirect_uri(value: &str) -> AppResult<()> {
    let url = Url::parse(value).map_err(|_| AppError::BadRequest("redirect_uri 无效".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AppError::BadRequest(
            "redirect_uri 必须是 HTTP(S) 地址".into(),
        ));
    }
    Ok(())
}

fn random_hex(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn random_bytes(size: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn option_required(options: &Value, name: &str, message: &str) -> AppResult<String> {
    options
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::BadRequest(message.into()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn oauth_retry_serialization_preserves_local_configuration() {
        let mut token: ProviderTokenResponse =
            serde_json::from_value(json!({"access_token":"test","expires_in":3600})).unwrap();
        token.provider = "gemini".into();
        token.oauth_type = "ai_studio".into();
        token.project_id = "configured-project".into();
        token.client_id = "configured-client".into();
        token.client_secret = "configured-secret".into();
        let restored: ProviderTokenResponse =
            serde_json::from_value(serde_json::to_value(&token).unwrap()).unwrap();
        assert_eq!(restored.provider, token.provider);
        assert_eq!(restored.oauth_type, token.oauth_type);
        assert_eq!(restored.project_id, token.project_id);
        assert_eq!(restored.client_id, token.client_id);
        assert_eq!(restored.client_secret, token.client_secret);
        let token: OpenAITokenResponse = serde_json::from_value(json!({"access_token":"test","token_type":"Bearer","expires_in":3600,"client_id":"configured-client"})).unwrap();
        let restored: OpenAITokenResponse =
            serde_json::from_value(serde_json::to_value(&token).unwrap()).unwrap();
        assert_eq!(restored.client_id, "configured-client");
    }

    #[tokio::test]
    async fn antigravity_location_rejection_stops_before_onboarding() {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route("/v1internal:loadCodeAssist", post(|| async {
            Json(json!({"allowedTiers":[{"id":"standard-tier","isDefault":true}],"ineligibleTiers":[{"reasonCode":"UNSUPPORTED_LOCATION"}]}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = initialize_google_account_with_bases(
            &reqwest::Client::new(),
            "test-token",
            "antigravity",
            "manual-project",
            &[&base],
        )
        .await;
        server.abort();
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("UNSUPPORTED_LOCATION"));
        assert!(validate_antigravity_eligibility(&json!({"currentTier":{"id":"paid"},"ineligibleTiers":[{"reasonCode":"UNSUPPORTED_LOCATION"}]})).is_ok());
    }

    #[tokio::test]
    async fn gemini_initialization_calls_onboarding_for_new_users() {
        use axum::{routing::post, Json, Router};
        let app = Router::new()
            .route("/v1internal:loadCodeAssist", post(|| async {
                Json(json!({"allowedTiers":[{"id":"standard-tier","isDefault":true}]}))
            }))
            .route("/v1internal:onboardUser", post(|Json(body): Json<Value>| async move {
                assert_eq!(body["tierId"], "standard-tier");
                Json(json!({"done":true,"response":{"cloudaicompanionProject":{"id":"assigned-project"}}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = initialize_google_account_with_bases(
            &reqwest::Client::new(),
            "test-token",
            "gemini",
            "",
            &[&base],
        )
        .await
        .unwrap();
        assert_eq!(result.project_id, "assigned-project");
        server.abort();
    }

    #[test]
    fn google_initialization_distinguishes_registration_and_eligibility() {
        let new_user = json!({"allowedTiers":[{"id":"standard-tier","isDefault":true}]});
        assert_eq!(
            google_onboarding_tier(&new_user, false).as_deref(),
            Some("standard-tier")
        );
        assert_eq!(
            google_onboarding_tier(&new_user, true).as_deref(),
            Some("standard-tier")
        );
        let registered = json!({"currentTier":{"id":"standard-tier"},"allowedTiers":[{"id":"standard-tier","isDefault":true}]});
        assert!(google_onboarding_tier(&registered, false).is_none());
        assert!(google_onboarding_tier(&json!({}), true).is_none());
        let rejected = json!({"ineligibleTiers":[{"reasonCode":"UNSUPPORTED_CLIENT"}],"allowedTiers":[{"id":"standard-tier","isDefault":true}]});
        assert!(validate_gemini_eligibility(&rejected)
            .unwrap_err()
            .to_string()
            .contains("UNSUPPORTED_CLIENT"));
        let paid = json!({"paidTier":{"id":"paid-tier"},"ineligibleTiers":[{"reasonCode":"UNSUPPORTED_CLIENT"}]});
        assert!(validate_gemini_eligibility(&paid).is_ok());
    }

    #[tokio::test]
    async fn openai_start_matches_codex_pkce_contract() {
        let store = OAuthSessionStore::default();
        let start = store.start_openai(7, None).await.unwrap();
        assert_eq!(start.session_id.len(), 32);
        assert_eq!(start.state.len(), 64);
        let url = Url::parse(&start.auth_url).unwrap();
        let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(
            url.as_str().split('?').next().unwrap(),
            OPENAI_AUTHORIZE_URL
        );
        assert_eq!(params["client_id"], OPENAI_CLIENT_ID);
        assert_eq!(params["redirect_uri"], OPENAI_REDIRECT_URI);
        assert_eq!(params["scope"], OPENAI_SCOPE);
        assert_eq!(params["state"], start.state);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["codex_cli_simplified_flow"], "true");
        assert!(!params["code_challenge"].contains('='));
    }

    #[tokio::test]
    async fn oauth_session_is_owned_state_checked_and_single_use() {
        let store = OAuthSessionStore::default();
        let start = store.start_openai(7, None).await.unwrap();
        assert!(store
            .validate_openai(8, &start.session_id, &start.state)
            .await
            .is_err());
        assert!(store
            .validate_openai(7, &start.session_id, "wrong")
            .await
            .is_err());
        let session = store
            .validate_openai(7, &start.session_id, &start.state)
            .await
            .unwrap();
        assert_eq!(session.code_verifier.len(), 128);
        assert_eq!(session.client_id, OPENAI_CLIENT_ID);
        assert_eq!(session.redirect_uri, OPENAI_REDIRECT_URI);
        assert!(store
            .validate_openai(7, &start.session_id, &start.state)
            .await
            .is_ok());
        store.finish(&start.session_id).await;
        assert!(store
            .validate_openai(7, &start.session_id, &start.state)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn persistent_oauth_session_survives_store_recreation_and_is_atomically_consumed() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE oauth_sessions(session_hash TEXT PRIMARY KEY,owner_id INTEGER NOT NULL,provider TEXT NOT NULL,state_hash TEXT NOT NULL,encrypted_payload TEXT NOT NULL,expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let first = OAuthSessionStore::persistent(pool.clone(), "master-key");
        let started = first.start_openai(7, None).await.unwrap();
        drop(first);

        let recreated = OAuthSessionStore::persistent(pool, "master-key");
        let consumed = recreated
            .consume(7, "openai", &started.session_id, &started.state)
            .await
            .unwrap();
        assert_eq!(consumed.owner_id, 7);
        assert!(recreated
            .consume(7, "openai", &started.session_id, &started.state)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn provider_oauth_urls_match_public_clients_and_callbacks() {
        let store = OAuthSessionStore::default();
        for (provider, expected_client, expected_redirect) in [
            ("anthropic", CLAUDE_CLIENT_ID, CLAUDE_REDIRECT_URI),
            ("gemini", GEMINI_CLIENT_ID, GEMINI_REDIRECT_URI),
            (
                "antigravity",
                ANTIGRAVITY_CLIENT_ID,
                ANTIGRAVITY_REDIRECT_URI,
            ),
            ("grok", GROK_CLIENT_ID, GROK_REDIRECT_URI),
        ] {
            let start = store.start_provider(7, provider, &json!({})).await.unwrap();
            let url = Url::parse(&start.auth_url).unwrap();
            let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
            assert_eq!(params["client_id"], expected_client);
            assert_eq!(params["redirect_uri"], expected_redirect);
            assert_eq!(params["state"], start.state);
            assert_eq!(params["code_challenge_method"], "S256");
        }
    }

    #[tokio::test]
    async fn ai_studio_requires_and_preserves_custom_oauth_client() {
        let store = OAuthSessionStore::default();
        assert!(store
            .start_provider(7, "gemini", &json!({"oauth_type":"ai_studio"}))
            .await
            .is_err());
        let start = store
            .start_provider(
                7,
                "gemini",
                &json!({
                    "oauth_type":"ai_studio", "client_id":"custom.apps.googleusercontent.com",
                    "client_secret":"secret", "project_id":"project-1"
                }),
            )
            .await
            .unwrap();
        let url = Url::parse(&start.auth_url).unwrap();
        let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(params["client_id"], "custom.apps.googleusercontent.com");
        assert_eq!(params["redirect_uri"], GEMINI_LOCAL_REDIRECT_URI);
        assert_eq!(params["project_id"], "project-1");
    }

    #[test]
    fn id_token_claims_map_to_account_credentials() {
        let payload = json!({
            "email":"user@example.com",
            "https://api.openai.com/auth":{
                "chatgpt_account_id":"acct-1",
                "chatgpt_user_id":"user-1",
                "chatgpt_plan_type":"plus",
                "user_id":"internal-1",
                "organizations":[{"id":"org-1","is_default":true}]
            }
        });
        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        );
        let token = OpenAITokenResponse {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            id_token,
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: String::new(),
            client_id: OPENAI_CLIENT_ID.into(),
        };
        let (credentials, extra, name) = token.account_payload();
        assert_eq!(credentials["chatgpt_account_id"], "acct-1");
        assert_eq!(credentials["organization_id"], "org-1");
        assert_eq!(credentials["plan_type"], "plus");
        assert_eq!(extra["privacy_mode"], "training_off");
        assert_eq!(name, "user@example.com");
    }

    #[test]
    fn constant_time_comparison_checks_all_bytes() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"samf"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }
}
