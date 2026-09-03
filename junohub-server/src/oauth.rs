use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use url::Url;

use crate::error::{AppError, AppResult};

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OPENAI_SCOPE: &str = "openid profile email offline_access";
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug)]
struct OAuthSession {
    owner_id: i64,
    state: String,
    code_verifier: String,
    client_id: String,
    redirect_uri: String,
    created_at: Instant,
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
    #[serde(skip)]
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
        let expires_at = Utc::now().timestamp().saturating_add(self.expires_in);
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

#[derive(Clone, Debug)]
pub struct OAuthExchangeSession {
    pub code_verifier: String,
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Clone, Default)]
pub struct OAuthSessionStore {
    sessions: Arc<RwLock<HashMap<String, OAuthSession>>>,
}

impl OAuthSessionStore {
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
        self.sessions.write().await.insert(
            session_id.clone(),
            OAuthSession {
                owner_id,
                state: state.clone(),
                code_verifier,
                client_id: OPENAI_CLIENT_ID.into(),
                redirect_uri,
                created_at: Instant::now(),
            },
        );
        Ok(OAuthStart {
            auth_url: url.into(),
            session_id,
            state,
        })
    }

    pub async fn validate_openai(
        &self,
        owner_id: i64,
        session_id: &str,
        state: &str,
    ) -> AppResult<OAuthExchangeSession> {
        self.remove_expired().await;
        let session = self
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::BadRequest("OAuth 会话不存在或已过期".into()))?;
        if session.owner_id != owner_id {
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

    pub async fn finish(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
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
        let session = self.validate_openai(owner_id, session_id, state).await?;
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
        self.finish(session_id).await;
        Ok(token)
    }

    async fn remove_expired(&self) {
        self.sessions
            .write()
            .await
            .retain(|_, session| session.created_at.elapsed() <= SESSION_TTL);
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
