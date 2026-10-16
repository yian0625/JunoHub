//! Managed device-code accounts. Credentials stay in the local component.
use crate::client_adapters::providers::{codex_oauth_auth::*, copilot_auth::*, xai_oauth_auth::*};
use anyhow::{bail, Context, Result};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub auth_provider: String,
    #[serde(default)]
    pub account_id: Option<String>,
}
pub struct Accounts {
    pub copilot: CopilotAuthManager,
    pub codex: CodexOAuthManager,
    pub xai: XaiOAuthManager,
    pending: Mutex<BTreeMap<String, Pending>>,
}
#[derive(Clone)]
struct Pending {
    provider: String,
    domain: Option<String>,
    device: String,
    expires: Instant,
    next: Instant,
    interval: u64,
}
pub struct Credential {
    pub base: String,
    pub headers: BTreeMap<String, String>,
}
impl Accounts {
    pub fn new(home: PathBuf) -> Self {
        let directory = home.join(".junohub/managed-accounts");
        Self {
            copilot: CopilotAuthManager::new(directory.clone()),
            codex: CodexOAuthManager::new(directory.clone()),
            xai: XaiOAuthManager::new(directory),
            pending: Mutex::default(),
        }
    }
    pub async fn status(&self) -> Value {
        json!({"success":true,"providers":{"github_copilot":self.copilot.get_status().await,"codex_oauth":self.codex.get_status().await,"xai_oauth":self.xai.get_status().await}})
    }
    pub async fn start(
        &self,
        provider: &str,
        domain: Option<&str>,
        target: Option<&str>,
    ) -> Result<Value> {
        let code = match provider {
            "github_copilot" => self.copilot.start_device_flow(domain).await?,
            "codex_oauth" => self.codex.start_device_flow(target).await?,
            "xai_oauth" => self.xai.start_device_flow().await?,
            _ => bail!("不支持的托管账号类型"),
        };
        let session = uuid::Uuid::new_v4().to_string();
        let interval = code.interval.max(5);
        let mut pending = self.pending.lock().await;
        pending.retain(|_, p| p.expires > Instant::now());
        if pending.len() >= 16 {
            bail!("进行中的登录过多，请先取消之前的登录");
        }
        pending.insert(
            session.clone(),
            Pending {
                provider: provider.into(),
                domain: domain.map(str::to_string),
                device: code.device_code,
                expires: Instant::now() + Duration::from_secs(code.expires_in),
                next: Instant::now() + Duration::from_secs(interval),
                interval,
            },
        );
        Ok(
            json!({"success":true,"sessionId":session,"userCode":code.user_code,"verificationUri":code.verification_uri,"expiresIn":code.expires_in,"interval":interval}),
        )
    }
    pub async fn poll(&self, session: &str) -> Result<Value> {
        let entry = {
            let mut pending = self.pending.lock().await;
            let entry = pending
                .get_mut(session)
                .context("登录会话不存在，请重新登录")?;
            if entry.expires <= Instant::now() {
                pending.remove(session);
                bail!("登录会话已过期");
            }
            if entry.next > Instant::now() {
                return Ok(json!({"success":true,"pending":true,"interval":entry.interval}));
            }
            entry.next = Instant::now() + Duration::from_secs(entry.interval);
            entry.clone()
        };
        let result: Result<Option<Value>> = match entry.provider.as_str() {
            "github_copilot" => match self
                .copilot
                .poll_for_token(&entry.device, entry.domain.as_deref())
                .await
            {
                Ok(account) => Ok(account.map(|a| json!(a))),
                Err(CopilotAuthError::AuthorizationPending) => Ok(None),
                Err(e) => Err(e.into()),
            },
            "codex_oauth" => match self.codex.poll_for_token(&entry.device, || async {}).await {
                Ok(account) => Ok(account.map(|a| json!(a))),
                Err(CodexOAuthError::AuthorizationPending) => Ok(None),
                Err(e) => Err(e.into()),
            },
            "xai_oauth" => match self.xai.poll_for_token(&entry.device).await {
                Ok(account) => Ok(account.map(|a| json!(a))),
                Err(XaiOAuthError::AuthorizationPending) => Ok(None),
                Err(e) => Err(e.into()),
            },
            _ => unreachable!(),
        };
        match result {
            Ok(Some(account)) => {
                self.pending.lock().await.remove(session);
                Ok(json!({"success":true,"pending":false,"account":account}))
            }
            Ok(None) => Ok(json!({"success":true,"pending":true,"interval":entry.interval})),
            Err(e) => {
                self.pending.lock().await.remove(session);
                Err(e)
            }
        }
    }
    pub async fn cancel(&self, session: &str) {
        if let Some(entry) = self.pending.lock().await.remove(session) {
            if entry.provider == "codex_oauth" {
                self.codex.cancel_device_flow(&entry.device).await;
            }
        }
    }
    pub async fn account_id(&self, binding: &Binding) -> Result<String> {
        if let Some(id) = binding.account_id.as_ref().filter(|id| !id.is_empty()) {
            return Ok(id.clone());
        }
        match binding.auth_provider.as_str() {
            "github_copilot" => self.copilot.get_status().await.default_account_id,
            "codex_oauth" => self.codex.default_account_id().await,
            "xai_oauth" => self.xai.default_account_id().await,
            _ => bail!("不支持的托管账号类型"),
        }
        .context("尚未登录托管账号")
    }
    pub async fn credential(&self, binding: &Binding) -> Result<Credential> {
        let id = self.account_id(binding).await?;
        let mut headers = BTreeMap::new();
        let (token, base) = match binding.auth_provider.as_str() {
            "github_copilot" => {
                for (key, value) in [
                    ("editor-version", COPILOT_EDITOR_VERSION),
                    ("editor-plugin-version", COPILOT_PLUGIN_VERSION),
                    ("user-agent", COPILOT_USER_AGENT),
                    ("copilot-integration-id", COPILOT_INTEGRATION_ID),
                    ("x-github-api-version", COPILOT_API_VERSION),
                    ("openai-intent", "conversation-agent"),
                    ("x-interaction-type", "conversation-agent"),
                    ("x-vscode-user-agent-library-version", "electron-fetch"),
                ] {
                    headers.insert(key.into(), value.into());
                }
                (
                    self.copilot.get_valid_token_for_account(&id).await?,
                    self.copilot.get_api_endpoint(&id).await,
                )
            }
            "codex_oauth" => {
                headers.insert(
                    "chatgpt-account-id".into(),
                    self.codex.chatgpt_account_id_for_account(&id).await?,
                );
                headers.insert("originator".into(), CODEX_OAUTH_ORIGINATOR.into());
                headers.insert("version".into(), CODEX_OAUTH_CLIENT_VERSION.into());
                headers.insert(
                    "user-agent".into(),
                    format!("codex_cli_rs/{CODEX_OAUTH_CLIENT_VERSION}"),
                );
                (
                    self.codex.get_valid_token_for_account(&id).await?,
                    "https://chatgpt.com/backend-api/codex".into(),
                )
            }
            "xai_oauth" => (
                self.xai.get_valid_token_for_account(&id).await?,
                "https://api.x.ai/v1".into(),
            ),
            _ => bail!("不支持的托管账号类型"),
        };
        headers.insert("authorization".into(), format!("Bearer {token}"));
        Ok(Credential { base, headers })
    }
    pub async fn models(
        &self,
        binding: &Binding,
    ) -> Result<Vec<crate::client_model_fetch::FetchedModel>> {
        let id = self.account_id(binding).await?;
        match binding.auth_provider.as_str() {
            "github_copilot" => Ok(self
                .copilot
                .fetch_models_for_account(&id)
                .await?
                .into_iter()
                .map(|m| crate::client_model_fetch::FetchedModel {
                    id: m.id,
                    owned_by: Some(m.vendor),
                })
                .collect()),
            "codex_oauth" => {
                crate::client_adapters::providers::codex_oauth_models::fetch_models_with_token(
                    &self.codex.get_valid_token_for_account(&id).await?,
                    &self.codex.chatgpt_account_id_for_account(&id).await?,
                )
                .await
                .map_err(anyhow::Error::msg)
            }
            "xai_oauth" => {
                let auth = self.credential(binding).await?;
                crate::client_model_fetch::fetch_models(
                    &auth.base,
                    "",
                    false,
                    None,
                    None,
                    None,
                    Some(&auth.headers),
                )
                .await
                .map_err(anyhow::Error::msg)
            }
            _ => bail!("不支持的托管账号类型"),
        }
    }
    pub async fn manage(&self, provider: &str, id: &str, remove: bool) -> Result<()> {
        match (provider, remove) {
            ("github_copilot", true) => self.copilot.remove_account(id).await?,
            ("github_copilot", false) => self.copilot.set_default_account(id).await?,
            ("codex_oauth", true) => self.codex.remove_account(id).await?,
            ("codex_oauth", false) => self.codex.set_default_account(id).await?,
            ("xai_oauth", true) => self.xai.remove_account(id).await?,
            ("xai_oauth", false) => self.xai.set_default_account(id).await?,
            _ => bail!("不支持的托管账号类型"),
        };
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    action: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    domain: Option<String>,
}
pub async fn status(State(state): State<crate::HelperState>) -> Json<Value> {
    Json(state.proxy.accounts.status().await)
}
pub async fn action(
    State(state): State<crate::HelperState>,
    Json(request): Json<Action>,
) -> Json<Value> {
    let result: Result<Value> = async {
        let accounts = &state.proxy.accounts;
        match request.action.as_str() {
            "start" => {
                accounts
                    .start(
                        &request.provider,
                        request.domain.as_deref(),
                        Some(&request.account_id)
                            .filter(|s| !s.is_empty())
                            .map(String::as_str),
                    )
                    .await
            }
            "poll" => accounts.poll(&request.session_id).await,
            "cancel" => {
                accounts.cancel(&request.session_id).await;
                Ok(json!({"success":true}))
            }
            "remove" | "default" => {
                accounts
                    .manage(
                        &request.provider,
                        &request.account_id,
                        request.action == "remove",
                    )
                    .await?;
                Ok(accounts.status().await)
            }
            "check" => {
                accounts
                    .credential(&Binding {
                        auth_provider: request.provider,
                        account_id: Some(request.account_id),
                    })
                    .await?;
                Ok(json!({"success":true}))
            }
            _ => bail!("不支持的账号操作"),
        }
    }
    .await;
    Json(result.unwrap_or_else(|e| json!({"success":false,"error":e.to_string()})))
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use axum::{body::Bytes, extract::State, routing::post, Router};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    fn jwt() -> String {
        format!("eyJhbGciOiJub25lIn0.{}.",URL_SAFE_NO_PAD.encode(json!({"sub":"qa-user","email":"qa@example.test","https://api.openai.com/auth":{"chatgpt_account_id":"qa-workspace"}}).to_string()))
    }
    async fn device() -> Json<Value> {
        Json(
            json!({"device_auth_id":"device-qa","user_code":"TEST-CODE","interval":5,"expires_in":600}),
        )
    }
    async fn poll(State(count): State<Arc<AtomicUsize>>) -> (axum::http::StatusCode, Json<Value>) {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(json!({"pending":true})),
            )
        } else {
            (
                axum::http::StatusCode::OK,
                Json(json!({"authorization_code":"code-qa","code_verifier":"verifier-qa"})),
            )
        }
    }
    async fn tokens(body: Bytes) -> Json<Value> {
        let form: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        let refresh = form.get("grant_type").unwrap() == "refresh_token";
        if refresh {
            assert_eq!(form.get("refresh_token").unwrap(), "refresh-qa");
        } else {
            assert_eq!(form.get("code").unwrap(), "code-qa");
            assert_eq!(form.get("code_verifier").unwrap(), "verifier-qa");
        }
        Json(
            json!({"access_token":jwt(),"id_token":jwt(),"refresh_token":if refresh{"refresh-rotated"}else{"refresh-qa"},"expires_in":if refresh{3600}else{1}}),
        )
    }
    #[tokio::test]
    async fn device_login_refresh_restart_delete_preserve_official_login() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let original = b"official login fixture";
        std::fs::write(home.join(".codex/auth.json"), original).unwrap();
        let router = Router::new()
            .route("/api/accounts/deviceauth/usercode", post(device))
            .route("/api/accounts/deviceauth/token", post(poll))
            .route("/oauth/token", post(tokens))
            .with_state(Arc::new(AtomicUsize::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        crate::client_adapters::http_client::TEST_ENDPOINT
            .scope(base, async {
                let accounts = Accounts::new(home.into());
                let login = accounts.start("codex_oauth", None, None).await.unwrap();
                assert!(login.get("device_code").is_none());
                let session = login["sessionId"].as_str().unwrap();
                accounts.pending.lock().await.get_mut(session).unwrap().next = Instant::now();
                assert_eq!(accounts.poll(session).await.unwrap()["pending"], true);
                accounts.pending.lock().await.get_mut(session).unwrap().next = Instant::now();
                let completed = accounts.poll(session).await.unwrap();
                assert_eq!(completed["pending"], false);
                let id = completed["account"]["id"].as_str().unwrap();
                let binding = Binding {
                    auth_provider: "codex_oauth".into(),
                    account_id: Some(id.into()),
                };
                let credential = accounts.credential(&binding).await.unwrap();
                assert_eq!(credential.headers["chatgpt-account-id"], "qa-workspace");
                let status = accounts.status().await.to_string();
                assert!(!status.contains("refresh-qa"));
                assert!(!status.contains("refresh-rotated"));
                assert!(!status.contains(&jwt()));
                let restarted = Accounts::new(home.into());
                assert_eq!(
                    restarted
                        .account_id(&Binding {
                            auth_provider: "codex_oauth".into(),
                            account_id: None
                        })
                        .await
                        .unwrap(),
                    id
                );
                let files = std::fs::read_dir(home.join(".junohub/managed-accounts")).unwrap();
                let mut found = false;
                for file in files {
                    let file = file.unwrap();
                    let data = std::fs::read_to_string(file.path()).unwrap();
                    if data.contains("refresh-rotated") {
                        found = true;
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            assert_eq!(
                                file.metadata().unwrap().permissions().mode() & 0o777,
                                0o600
                            );
                        }
                    }
                }
                assert!(found, "refresh rotation not persisted");
                restarted.manage("codex_oauth", id, true).await.unwrap();
                assert!(restarted.credential(&binding).await.is_err());
                assert_eq!(
                    std::fs::read(home.join(".codex/auth.json")).unwrap(),
                    original
                );
                let login = accounts.start("codex_oauth", None, None).await.unwrap();
                let session = login["sessionId"].as_str().unwrap();
                accounts.cancel(session).await;
                assert!(accounts.poll(session).await.is_err());
            })
            .await;
        server.abort();
    }
}
