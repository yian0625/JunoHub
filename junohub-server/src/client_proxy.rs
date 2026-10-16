//! Native loopback proxy: takeover, streaming relay, ordered failover and restore.
//! Behavior follows CC Switch's proxy/takeover/failover commands; no Tauri runtime.
use crate::client_connect::{self, Client, ConnectRequest};
use anyhow::{bail, Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{Path as RoutePath, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct LocalProxy {
    home: PathBuf,
    port: u16,
    state: Arc<Mutex<Stored>>,
    stats: Arc<Mutex<BTreeMap<String, Stats>>>,
    pub accounts: Arc<crate::client_accounts::Accounts>,
    pub usage: Arc<crate::client_usage::UsageStore>,
    http: reqwest::Client,
    shadow: Arc<crate::client_adapters::providers::gemini_shadow::GeminiShadowStore>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Stored {
    routes: BTreeMap<String, Route>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Route {
    client: String,
    token: String,
    sources: Vec<Upstream>,
    #[serde(default)]
    failover_queue: Vec<String>,
    backup_id: String,
    #[serde(default)]
    previous_active: BTreeMap<String, crate::ActiveProvider>,
    written: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Value,
    timeout_seconds: u64,
    failure_threshold: u32,
    cooldown_seconds: u64,
    #[serde(default = "legacy_failover")]
    auto_failover_enabled: bool,
}
impl Route {
    fn queue_sources(&self) -> Vec<&Upstream> {
        if self.failover_queue.is_empty() {
            return self.sources.iter().collect();
        }
        self.failover_queue
            .iter()
            .filter_map(|id| self.sources.iter().find(|s| &s.id == id))
            .collect()
    }
    fn request_sources(&self) -> Vec<&Upstream> {
        if self.auto_failover_enabled {
            self.queue_sources()
        } else {
            self.sources.iter().take(1).collect()
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Upstream {
    id: String,
    name: String,
    base_url: String,
    headers: BTreeMap<String, String>,
    model: String,
    protocol: String,
    #[serde(default)]
    models: BTreeMap<String, String>,
    #[serde(default)]
    reasoning: Option<crate::client_adapters::types::CodexChatReasoningConfig>,
    #[serde(default)]
    catalog: Vec<Value>,
    #[serde(default)]
    binding: Option<crate::client_accounts::Binding>,
    #[serde(default)]
    pricing: Option<Value>,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    requests: u64,
    failovers: u64,
    errors: u64,
    last_provider: Option<String>,
    last_error: Option<String>,
    last_latency_ms: u64,
    #[serde(skip)]
    failures: BTreeMap<String, (u32, Option<Instant>)>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configure {
    pub client: String,
    pub pair_urls: Vec<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u64,
    #[serde(default)]
    pub auto_failover_enabled: Option<bool>,
}
fn legacy_failover() -> bool {
    true
}
fn empty_object() -> Value {
    json!({})
}
fn default_timeout() -> u64 {
    30
}
fn default_threshold() -> u32 {
    3
}
fn default_cooldown() -> u64 {
    30
}
#[derive(Deserialize)]
pub struct ClientRequest {
    pub client: String,
}
fn state_path(home: &Path) -> PathBuf {
    home.join(".junohub/local-proxy.json")
}
pub fn is_taken_over(home: &Path, client: &str) -> Result<bool> {
    Ok(read_state(home)?.routes.contains_key(client))
}
pub fn check_restore(home: &Path, backup: &str) -> Result<()> {
    if backup.is_empty()
        || !backup
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        bail!("备份编号无效");
    }
    let state = read_state(home)?;
    if state.routes.is_empty() {
        return Ok(());
    }
    let manifest: Value = serde_json::from_slice(&std::fs::read(
        home.join(".junohub/backups")
            .join(backup)
            .join("manifest.json"),
    )?)?;
    if manifest
        .get("files")
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files.iter().any(|file| {
                file.get("relative_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| {
                        state
                            .routes
                            .values()
                            .any(|route| route.written.contains_key(path))
                    })
            })
        })
    {
        bail!("备份包含正在由本地代理接管的配置，请在本地路由中停用并恢复");
    }
    Ok(())
}
fn read_state(home: &Path) -> Result<Stored> {
    match std::fs::read(state_path(home)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).context("本地代理配置损坏，未覆盖")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
        Err(e) => Err(e.into()),
    }
}
fn persist(home: &Path, state: &Stored) -> Result<()> {
    client_connect::atomic_write(&state_path(home), &serde_json::to_vec_pretty(state)?)
}
fn hash_file(path: &Path) -> Result<String> {
    Ok(hex::encode(Sha256::digest(std::fs::read(path)?)))
}
fn single_client(value: &str) -> Result<Client> {
    let clients = Client::parse(value)?;
    if clients.len() != 1 {
        bail!("请选择一个客户端");
    }
    Ok(clients[0])
}
impl LocalProxy {
    pub fn new(home: PathBuf, port: u16) -> Result<Self> {
        Ok(Self {
            state: Arc::new(Mutex::new(read_state(&home)?)),
            accounts: Arc::new(crate::client_accounts::Accounts::new(home.clone())),
            usage: Arc::new(crate::client_usage::UsageStore::new(&home)?),
            home,
            port,
            stats: Arc::default(),
            shadow: Arc::default(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()?,
        })
    }
    pub async fn status(&self) -> Value {
        let state = self.state.lock().await;
        let stats = self.stats.lock().await;
        json!({"success":true,"routes":state.routes.iter().map(|(client,route)| json!({"client":client,"enabled":true,"autoFailoverEnabled":route.auto_failover_enabled,"sources":route.queue_sources().into_iter().map(|s|json!({"id":s.id,"name":s.name})).collect::<Vec<_>>(),"timeoutSeconds":route.timeout_seconds,"failureThreshold":route.failure_threshold,"cooldownSeconds":route.cooldown_seconds,"hasHeaderOverrides":!route.headers.is_empty(),"hasBodyOverrides":route.body.as_object().is_some_and(|v|!v.is_empty()),"stats":stats.get(client).cloned().unwrap_or_default()})).collect::<Vec<_>>()})
    }
    pub async fn configure(&self, request: Configure) -> Result<()> {
        let client = single_client(&request.client)?;
        if request.pair_urls.is_empty() || request.pair_urls.len() > 10 {
            bail!("请选择 1 至 10 个供应商");
        }
        if !(1..=120).contains(&request.timeout_seconds)
            || !(1..=10).contains(&request.failure_threshold)
            || !(1..=600).contains(&request.cooldown_seconds)
        {
            bail!("超时、失败阈值或冷却时间超出范围");
        }
        if request.body.as_ref().is_some_and(|v| !v.is_object()) {
            bail!("请求体覆盖必须是 JSON 对象");
        }
        if let Some(headers) = &request.headers {
            validate_headers(headers)?;
        }
        let mut pairs = Vec::new();
        for url in &request.pair_urls {
            let pair = crate::redeem_pairing(url).await?;
            if Client::parse(&pair.client)? != vec![client] {
                bail!("供应商凭证不属于当前客户端");
            }
            pairs.push(pair);
        }
        self.configure_pairs(request, pairs).await.map(|_| ())
    }
    pub async fn configure_pairs(
        &self,
        request: Configure,
        mut pairs: Vec<crate::PairData>,
    ) -> Result<client_connect::ConnectResult> {
        let client = single_client(&request.client)?;
        for pair in &mut pairs {
            crate::client_common::apply_pair(&self.home, pair)?;
        }
        let sources = pairs
            .iter()
            .map(|pair| extract_upstream(pair, client))
            .collect::<Result<Vec<_>>>()?;
        let primary = pairs.remove(0);
        self.configure_sources(request, primary, sources).await
    }
    async fn configure_sources(
        &self,
        request: Configure,
        mut primary: crate::PairData,
        sources: Vec<Upstream>,
    ) -> Result<client_connect::ConnectResult> {
        let client = single_client(&request.client)?;
        let supported = |protocol: &str| match client {
            Client::Claude => matches!(
                protocol,
                "anthropic-messages"
                    | "openai-completions"
                    | "openai-responses"
                    | "google-generative-ai"
            ),
            Client::Codex => matches!(
                protocol,
                "anthropic-messages" | "openai-completions" | "openai-responses"
            ),
            _ => protocol == sources[0].protocol,
        };
        if sources.iter().any(|source| !supported(&source.protocol)) {
            bail!("所选供应商的协议不能用于此客户端的主备路由");
        }
        for source in &sources {
            if let Some(binding) = &source.binding {
                self.accounts.credential(binding).await?;
            }
        }
        let mut identities = std::collections::HashSet::new();
        for source in &sources {
            if !identities.insert(&source.id) {
                bail!("不能重复添加供应商");
            }
        }
        let mut state = self.state.lock().await;
        let previous = state.routes.get(client.key()).cloned();
        if let Some(route) = &previous {
            self.check_files(route)?;
        }
        let token = previous
            .as_ref()
            .map(|r| r.token.clone())
            .unwrap_or_else(|| format!("juno-local-{}", uuid::Uuid::new_v4().simple()));
        let endpoint = format!(
            "http://127.0.0.1:{}/local-proxy/{}",
            self.port,
            client.key()
        );
        replace_endpoint(&mut primary, client, &endpoint, &token)?;
        let connected = client_connect::apply(ConnectRequest {
            clients: vec![client],
            provider_id: primary.provider_id,
            provider_name: primary.name,
            base_url: primary.base_url,
            api_key: primary.api_key,
            model: primary.model,
            models: primary.models,
            home: self.home.clone(),
        })?;
        if let Some(route) = &previous {
            if let Err(e) =
                client_connect::extend_backup(&self.home, &route.backup_id, &connected.backup_id)
            {
                client_connect::restore(&self.home, &connected.backup_id)?;
                return Err(e.context("无法补全原始配置备份"));
            }
        }
        let mut written = previous
            .as_ref()
            .map(|r| r.written.clone())
            .unwrap_or_default();
        for file in &connected.files {
            let path = Path::new(file);
            let relative = path.strip_prefix(&self.home)?.to_string_lossy().to_string();
            written.insert(relative, hash_file(path)?);
        }
        let route = Route {
            client: client.key().into(),
            token,
            failover_queue: sources.iter().map(|s| s.id.clone()).collect(),
            sources,
            backup_id: previous
                .as_ref()
                .map(|r| r.backup_id.clone())
                .unwrap_or_else(|| connected.backup_id.clone()),
            previous_active: previous
                .as_ref()
                .map(|r| r.previous_active.clone())
                .unwrap_or_else(|| {
                    crate::read_management_state(&self.home)
                        .active_providers
                        .into_iter()
                        .filter(|(key, _)| {
                            key == client.key() || key.starts_with(&format!("{}:", client.key()))
                        })
                        .collect()
                }),
            written,
            headers: request.headers.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|r| r.headers.clone())
                    .unwrap_or_default()
            }),
            body: request.body.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|r| r.body.clone())
                    .unwrap_or_else(empty_object)
            }),
            timeout_seconds: request.timeout_seconds,
            failure_threshold: request.failure_threshold,
            cooldown_seconds: request.cooldown_seconds,
            auto_failover_enabled: request.auto_failover_enabled.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|r| r.auto_failover_enabled)
                    .unwrap_or(true)
            }),
        };
        let mut next = state.clone();
        next.routes.insert(client.key().into(), route);
        if let Err(error) = persist(&self.home, &next) {
            client_connect::restore(&self.home, &connected.backup_id)?;
            return Err(error.context("代理配置保存失败，客户端配置已恢复"));
        }
        *state = next;
        crate::record_active_provider(
            &self.home,
            &state.routes[client.key()].sources[0].id,
            &state.routes[client.key()].sources[0].name,
            &connected.backup_id,
            &[client],
        )?;
        self.stats.lock().await.remove(client.key());
        Ok(connected)
    }
    pub async fn switch_pair(
        &self,
        pair: crate::PairData,
    ) -> Result<client_connect::ConnectResult> {
        let client = single_client(&pair.client)?;
        let previous = self
            .state
            .lock()
            .await
            .routes
            .get(client.key())
            .cloned()
            .context("客户端未启用本地代理")?;
        let source = extract_upstream(&pair, client)?;
        let mut sources = vec![source];
        sources.extend(
            previous
                .sources
                .into_iter()
                .filter(|s| s.id != sources[0].id)
                .collect::<Vec<_>>(),
        );
        sources.truncate(10);
        self.configure_sources(
            Configure {
                client: client.key().into(),
                pair_urls: vec![],
                headers: None,
                body: None,
                timeout_seconds: previous.timeout_seconds,
                failure_threshold: previous.failure_threshold,
                cooldown_seconds: previous.cooldown_seconds,
                auto_failover_enabled: Some(previous.auto_failover_enabled),
            },
            pair,
            sources,
        )
        .await
    }
    fn check_files(&self, route: &Route) -> Result<()> {
        for (relative, expected) in &route.written {
            if hash_file(&self.home.join(relative)).ok().as_ref() != Some(expected) {
                bail!(
                    "客户端配置在接管后被外部修改，未覆盖。原配置备份：{}",
                    route.backup_id
                );
            }
        }
        Ok(())
    }
    pub async fn disable(&self, client: &str) -> Result<()> {
        let client = single_client(client)?;
        let mut state = self.state.lock().await;
        let Some(route) = state.routes.get(client.key()).cloned() else {
            return Ok(());
        };
        let detached = client == Client::Pi && pi_configuration_detached(&self.home, &route.token);
        if !detached {
            self.check_files(&route)?;
            client_connect::restore(&self.home, &route.backup_id)?;
        }
        let mut next = state.clone();
        next.routes.remove(client.key());
        persist(&self.home, &next)?;
        *state = next;
        let mut management = crate::read_management_state(&self.home);
        management.active_providers.retain(|key, _| {
            key != client.key() && !key.starts_with(&format!("{}:", client.key()))
        });
        if !detached {
            management.active_providers.extend(route.previous_active);
        }
        client_connect::atomic_write(
            &crate::management_state_path(&self.home),
            &serde_json::to_vec_pretty(&management)?,
        )?;
        Ok(())
    }
}
// A removed/replaced Pi provider must not be resurrected from a takeover backup.
// Fail closed on missing/malformed configuration or any remaining proxy reference.
fn pi_configuration_detached(home: &Path, token: &str) -> bool {
    let Ok(bytes) = std::fs::read(home.join(".pi/agent/models.json")) else {
        return false;
    };
    let Ok(config) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    pi_has_no_proxy_reference(&config, token)
}
fn pi_has_no_proxy_reference(config: &Value, token: &str) -> bool {
    fn references(value: &Value, token: &str) -> bool {
        match value {
            Value::String(s) => {
                s.contains("/local-proxy/pi") || (!token.is_empty() && s.contains(token))
            }
            Value::Array(a) => a.iter().any(|v| references(v, token)),
            Value::Object(o) => o.values().any(|v| references(v, token)),
            _ => false,
        }
    }
    config.get("providers").is_some_and(Value::is_object) && !references(config, token)
}
#[cfg(test)]
mod detached_pi_tests {
    use super::*;
    #[test]
    fn removed_or_direct_providers_allow_detach() {
        assert!(pi_has_no_proxy_reference(
            &serde_json::json!({"providers":{}}),
            "local-token"
        ));
        assert!(pi_has_no_proxy_reference(
            &serde_json::json!({"providers":{"other":{"baseUrl":"https://example.test/v1","apiKey":"keep-me"}}}),
            "local-token"
        ));
    }
    #[test]
    fn partial_edits_and_invalid_configs_keep_protection() {
        for config in [
            serde_json::json!({}),
            serde_json::json!({"providers":{"p":{"baseUrl":"http://localhost:17372/local-proxy/pi/v1"}}}),
            serde_json::json!({"providers":{"p":{"apiKey":"local-token"}}}),
        ] {
            assert!(!pi_has_no_proxy_reference(&config, "local-token"));
        }
    }
}

fn validate_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    if headers.len() > 64 {
        bail!("请求头不能超过 64 项");
    }
    for (key, value) in headers {
        HeaderName::from_bytes(key.as_bytes()).context("请求头名称无效")?;
        HeaderValue::from_str(value).context("请求头值无效")?;
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            bail!("不能覆盖传输层请求头");
        }
    }
    Ok(())
}
fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_string()
}
fn str_headers(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|v| {
            v.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
                .collect()
        })
        .unwrap_or_default()
}
#[derive(Debug)]
struct UnavailableCredential;
impl std::fmt::Display for UnavailableCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"当前配置使用客户端登录，或尚未提供可供检测的凭证。可绑定托管账号、填写 API Key，或在客户端中验证。")
    }
}
impl std::error::Error for UnavailableCredential {}
fn extract_upstream(pair: &crate::PairData, client: Client) -> Result<Upstream> {
    let mut base = pair.base_url.clone();
    let mut key = pair.api_key.clone();
    let mut model = pair.model.clone();
    let mut auth = "authorization";
    let mut protocol = match client {
        Client::Claude => "anthropic-messages",
        Client::Gemini => "google-generative-ai",
        _ => "openai-responses",
    }
    .to_string();
    if let Some(m) = &pair.models {
        let simple = match client {
            Client::Grok => m.grok.as_ref(),
            Client::OpenCode => m.opencode.as_ref(),
            Client::OpenClaw => m.openclaw.as_ref(),
            Client::Hermes => m.hermes.as_ref(),
            Client::Pi => m.pi.as_ref(),
            _ => None,
        };
        if let Some(simple) = simple {
            if let Some(format) = &simple.protocol {
                protocol = format.clone();
            }
            if !simple.model.is_empty() {
                model = simple.model.clone();
            }
        }
    }
    let mut headers = BTreeMap::new();
    let mut models = BTreeMap::new();
    let mut binding = None;
    let mut reasoning = None;
    let mut catalog = Vec::new();
    let mut pricing = None;
    if let Some(native) = pair.models.as_ref().and_then(|m| m.native.as_ref()) {
        let s = &native.settings_config;
        pricing = native.pricing.clone();
        binding = native
            .auth_binding
            .clone()
            .map(serde_json::from_value::<crate::client_accounts::Binding>)
            .transpose()?;
        reasoning = native
            .codex_chat_reasoning
            .clone()
            .map(serde_json::from_value)
            .transpose()?;
        catalog = s
            .get("modelCatalog")
            .and_then(|c| {
                c.as_array()
                    .or_else(|| c.get("models").and_then(Value::as_array))
            })
            .cloned()
            .unwrap_or_default();
        if catalog.is_empty() {
            catalog = match s.get("models") {
                Some(Value::Array(rows)) => rows.clone(),
                Some(Value::Object(rows)) => rows
                    .iter()
                    .map(|(id, row)| {
                        let mut value = row.clone();
                        if let Some(map) = value.as_object_mut() {
                            map.entry("id").or_insert_with(|| json!(id));
                        }
                        value
                    })
                    .collect(),
                _ => Vec::new(),
            };
        }
        match client {
            Client::Claude => {
                protocol = match native.api_format.as_deref().unwrap_or("anthropic") {
                    "anthropic" => "anthropic-messages",
                    "openai_chat" => "openai-completions",
                    "openai_responses" => "openai-responses",
                    other => other,
                }
                .into();
                base = text(s.pointer("/env/ANTHROPIC_BASE_URL"));
                key = text(s.pointer("/env/ANTHROPIC_AUTH_TOKEN"));
                model = text(s.pointer("/env/ANTHROPIC_MODEL"));
                if let Some(env) = s.get("env").and_then(Value::as_object) {
                    for (key, value) in env {
                        if key.starts_with("ANTHROPIC_") && key.ends_with("_MODEL") {
                            if let Some(value) = value.as_str().filter(|v| !v.is_empty()) {
                                models.insert(key.clone(), value.into());
                            }
                        }
                    }
                }
                if key.is_empty() {
                    key = text(s.pointer("/env/ANTHROPIC_API_KEY"));
                    auth = "x-api-key";
                }
            }
            Client::Gemini => {
                base = text(s.pointer("/env/GOOGLE_GEMINI_BASE_URL"));
                if base.is_empty() {
                    base = "https://generativelanguage.googleapis.com".into();
                }
                key = text(s.pointer("/env/GEMINI_API_KEY"));
                model = text(s.pointer("/env/GEMINI_MODEL"));
                auth = "x-goog-api-key";
            }
            Client::Codex | Client::Grok => {
                let doc = text(s.get("config")).parse::<toml_edit::DocumentMut>()?;
                let id = doc
                    .get("model_provider")
                    .and_then(toml_edit::Item::as_str)
                    .unwrap_or("openai");
                let entry = doc.get("model_providers").and_then(|p| p.get(id));
                if let Some(entry) = entry {
                    base = entry
                        .get("base_url")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("")
                        .into();
                    key = entry
                        .get("experimental_bearer_token")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("")
                        .into();
                    if key.is_empty() {
                        key = text(s.pointer("/auth/OPENAI_API_KEY"));
                    }
                    if key.is_empty() {
                        if let Some(env_key) =
                            entry.get("env_key").and_then(toml_edit::Item::as_str)
                        {
                            key = std::env::var(env_key).unwrap_or_default();
                        }
                    }
                    model = doc
                        .get("model")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("")
                        .into();
                } else if client == Client::Grok {
                    let id = doc
                        .get("models")
                        .and_then(|v| v.get("default"))
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("");
                    let entry = doc
                        .get("model")
                        .and_then(|v| v.get(id))
                        .context("Grok 模型配置缺失")?;
                    base = entry
                        .get("base_url")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("")
                        .into();
                    key = entry
                        .get("api_key")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or("")
                        .into();
                    model = entry
                        .get("model")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or(id)
                        .into();
                }
            }
            Client::OpenCode => {
                base = text(s.pointer("/options/baseURL"));
                key = text(s.pointer("/options/apiKey"));
                headers = str_headers(s.pointer("/options/headers"));
                protocol = match text(s.get("npm")).as_str() {
                    "@ai-sdk/anthropic" => "anthropic-messages",
                    "@ai-sdk/google" | "gemini_native" => "google-generative-ai",
                    "@ai-sdk/openai-compatible" => "openai-completions",
                    _ => "openai-responses",
                }
                .into();
            }
            Client::Hermes => {
                base = text(s.get("base_url"));
                key = text(s.get("api_key"));
                protocol = text(s.get("api_mode"));
                headers = str_headers(s.get("headers"));
            }
            _ => {
                base = text(s.get("baseUrl"));
                key = text(s.get("apiKey"));
                protocol = text(s.get("api"));
                headers = str_headers(s.get("headers"));
            }
        }
    }
    if binding.is_none() && ((key.trim().is_empty() && headers.is_empty()) || key.starts_with('!'))
    {
        return Err(UnavailableCredential.into());
    }
    if let Some(format) = pair
        .models
        .as_ref()
        .and_then(|m| m.native.as_ref())
        .and_then(|n| n.api_format.as_deref())
        .filter(|_| matches!(client, Client::Claude | Client::Codex))
    {
        protocol = format.to_string();
    }
    protocol = match protocol.as_str() {
        "anthropic" | "anthropic_messages" | "@ai-sdk/anthropic" => "anthropic-messages",
        "responses" | "codex_responses" | "openai_responses" | "@ai-sdk/openai" => {
            "openai-responses"
        }
        "chat" | "chat_completions" | "openai_chat" | "@ai-sdk/openai-compatible" => {
            "openai-completions"
        }
        "@ai-sdk/google" | "gemini_native" => "google-generative-ai",
        other => other,
    }
    .to_string();
    if matches!(
        protocol.as_str(),
        "anthropic-messages" | "anthropic_messages"
    ) {
        if client != Client::Claude {
            auth = "x-api-key";
        }
        headers.insert("anthropic-version".into(), "2023-06-01".into());
    }
    if client == Client::Claude && protocol != "anthropic-messages" {
        auth = "authorization";
    }
    if protocol == "google-generative-ai" {
        auth = "x-goog-api-key";
    }
    if !key.is_empty() {
        headers.insert(
            auth.into(),
            if auth == "authorization" {
                format!("Bearer {key}")
            } else {
                key
            },
        );
    }
    if let Some(binding) = &binding {
        base = match binding.auth_provider.as_str() {
            "github_copilot" => "https://api.githubcopilot.com",
            "codex_oauth" => "https://chatgpt.com/backend-api/codex",
            "xai_oauth" => "https://api.x.ai/v1",
            _ => bail!("不支持的托管账号类型"),
        }
        .into();
        if binding.auth_provider != "github_copilot" {
            protocol = "openai-responses".into();
        }
    }
    if headers.is_empty() && binding.is_none() {
        bail!("{} 缺少可用于本地代理的 API 凭证", pair.name);
    }
    let url = url::Url::parse(&base).context("供应商 API 地址无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("供应商 API 地址无效");
    }
    if url.path().contains("/local-proxy/") {
        bail!("不能将本地代理自身作为上游");
    }
    validate_headers(&headers)?;
    Ok(Upstream {
        id: pair.provider_id.clone(),
        name: pair.name.clone(),
        base_url: base,
        headers,
        model,
        protocol,
        models,
        reasoning,
        catalog,
        binding,
        pricing,
    })
}
fn replace_endpoint(
    pair: &mut crate::PairData,
    client: Client,
    endpoint: &str,
    token: &str,
) -> Result<()> {
    pair.base_url = endpoint.into();
    pair.api_key = token.into();
    if let Some(native) = pair.models.as_mut().and_then(|m| m.native.as_mut()) {
        let s = &mut native.settings_config;
        match client {
            Client::Claude => {
                let env = s
                    .get_mut("env")
                    .and_then(Value::as_object_mut)
                    .context("Claude env 缺失")?;
                env.remove("ANTHROPIC_API_KEY");
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(token));
                env.insert("ANTHROPIC_BASE_URL".into(), json!(endpoint));
            }
            Client::Gemini => {
                s["env"]["GEMINI_API_KEY"] = json!(token);
                s["env"]["GOOGLE_GEMINI_BASE_URL"] = json!(endpoint);
            }
            Client::Codex | Client::Grok => {
                let mut doc = text(s.get("config")).parse::<toml_edit::DocumentMut>()?;
                let id = doc
                    .get("model_provider")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_string);
                if let Some(id) = id {
                    doc["model_providers"][&id]["base_url"] =
                        toml_edit::value(format!("{endpoint}/v1"));
                    doc["model_providers"][&id]["experimental_bearer_token"] =
                        toml_edit::value(token);
                    doc["model_providers"][&id]["requires_openai_auth"] = toml_edit::value(false);
                    if client == Client::Codex {
                        doc["model_providers"][&id]["wire_api"] = toml_edit::value("responses");
                    }
                    if let Some(table) = doc["model_providers"][&id].as_table_mut() {
                        table.remove("env_key");
                    }
                    s["auth"] = json!({"OPENAI_API_KEY":token});
                } else if client == Client::Grok {
                    let id = doc
                        .get("models")
                        .and_then(|v| v.get("default"))
                        .and_then(toml_edit::Item::as_str)
                        .context("Grok 默认模型缺失")?
                        .to_string();
                    doc["model"][&id]["base_url"] = toml_edit::value(format!("{endpoint}/v1"));
                    doc["model"][&id]["api_key"] = toml_edit::value(token);
                } else {
                    bail!("官方 OAuth 配置不能作为普通 API 代理接管");
                }
                s["config"] = json!(doc.to_string());
            }
            Client::OpenCode => {
                s["options"]["baseURL"] = json!(format!("{endpoint}/v1"));
                s["options"]["apiKey"] = json!(token);
            }
            Client::Hermes => {
                s["base_url"] = json!(format!("{endpoint}/v1"));
                s["api_key"] = json!(token);
            }
            _ => {
                s["baseUrl"] = json!(format!("{endpoint}/v1"));
                s["apiKey"] = json!(token);
            }
        }
        // The loopback credential must not be overridden by a provider's static header.
        for pointer in ["/headers", "/options/headers"] {
            if let Some(headers) = s.pointer_mut(pointer).and_then(Value::as_object_mut) {
                headers.retain(|key, _| {
                    !matches!(
                        key.to_ascii_lowercase().as_str(),
                        "authorization" | "x-api-key" | "x-goog-api-key"
                    )
                });
            }
        }
    }
    Ok(())
}
fn destination(base: &str, path: &str, query: Option<&str>) -> Result<url::Url> {
    if path.split('/').any(|part| part == ".." || part == ".") {
        bail!("无效请求路径");
    }
    let mut url = url::Url::parse(base)?;
    let prefix = url.path().trim_end_matches('/');
    let path = if prefix.ends_with("/v1") {
        path.strip_prefix("v1/").unwrap_or(path)
    } else if prefix.ends_with("/v1beta") {
        path.strip_prefix("v1beta/").unwrap_or(path)
    } else {
        path
    };
    url.set_path(&format!("{prefix}/{path}"));
    if let Some(query) = query {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if !matches!(key.as_ref(), "key" | "api_key" | "token") {
                url.query_pairs_mut().append_pair(&key, &value);
            }
        }
    }
    Ok(url)
}
fn merge(target: &mut Value, patch: &Value) {
    if patch.is_object() && !target.is_object() {
        *target = empty_object();
    }
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else if value.is_object() {
                merge(target.entry(key).or_insert_with(empty_object), value);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}
fn error(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"error":{"message":message,"type":"local_proxy_error"}}).to_string(),
        ))
        .unwrap()
}
pub async fn status(State(state): State<crate::HelperState>) -> Json<Value> {
    Json(state.proxy.status().await)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    client: String,
    auto_failover_enabled: Option<bool>,
    timeout_seconds: Option<u64>,
    failure_threshold: Option<u32>,
    cooldown_seconds: Option<u64>,
}
pub async fn settings(
    State(state): State<crate::HelperState>,
    Json(request): Json<Settings>,
) -> Json<Value> {
    let result: Result<()> = async {
        if request
            .timeout_seconds
            .is_some_and(|v| !(1..=120).contains(&v))
            || request
                .failure_threshold
                .is_some_and(|v| !(1..=10).contains(&v))
            || request
                .cooldown_seconds
                .is_some_and(|v| !(1..=600).contains(&v))
        {
            bail!("代理设置超出允许范围");
        }
        let mut stored = state.proxy.state.lock().await;
        let mut next = stored.clone();
        let route = next
            .routes
            .get_mut(&request.client)
            .context("请先启用客户端接管")?;
        if let Some(value) = request.auto_failover_enabled {
            route.auto_failover_enabled = value;
        }
        if let Some(value) = request.timeout_seconds {
            route.timeout_seconds = value;
        }
        if let Some(value) = request.failure_threshold {
            route.failure_threshold = value;
        }
        if let Some(value) = request.cooldown_seconds {
            route.cooldown_seconds = value;
        }
        persist(&state.proxy.home, &next)?;
        *stored = next;
        if let Some(stats) = state.proxy.stats.lock().await.get_mut(&request.client) {
            stats.failures.clear();
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Json(state.proxy.status().await),
        Err(e) => Json(json!({"success":false,"error":e.to_string()})),
    }
}
pub async fn configure(
    State(state): State<crate::HelperState>,
    Json(request): Json<Configure>,
) -> Json<Value> {
    match state.proxy.configure(request).await {
        Ok(()) => Json(state.proxy.status().await),
        Err(e) => Json(json!({"success":false,"error":e.to_string()})),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRequest {
    client: String,
    pair_urls: Vec<String>,
}
pub async fn queue(
    State(state): State<crate::HelperState>,
    Json(request): Json<ProbeRequest>,
) -> Json<Value> {
    let result: Result<()> = async {
        let client = single_client(&request.client)?;
        if request.pair_urls.is_empty() || request.pair_urls.len() > 10 {
            bail!("请选择 1 至 10 个供应商");
        }
        let mut sources = Vec::new();
        for url in request.pair_urls {
            let mut pair = crate::redeem_pairing(&url).await?;
            if Client::parse(&pair.client)? != vec![client] {
                bail!("供应商凭证不属于当前客户端");
            }
            crate::client_common::apply_pair(&state.proxy.home, &mut pair)?;
            let source = extract_upstream(&pair, client)?;
            if sources.iter().any(|s: &Upstream| s.id == source.id) {
                bail!("不能重复添加供应商");
            }
            sources.push(source);
        }
        let mut stored = state.proxy.state.lock().await;
        let mut next = stored.clone();
        let route = next
            .routes
            .get_mut(client.key())
            .context("请先启用客户端接管")?;
        state.proxy.check_files(route)?;
        if !matches!(client, Client::Claude | Client::Codex)
            && sources
                .iter()
                .any(|s| s.protocol != route.sources[0].protocol)
        {
            bail!("当前客户端的队列须使用相同 API 协议");
        }
        route.failover_queue = sources.iter().map(|s| s.id.clone()).collect();
        let active = sources
            .iter()
            .find(|s| s.id == route.sources[0].id)
            .cloned()
            .unwrap_or_else(|| route.sources[0].clone());
        sources.retain(|s| s.id != active.id);
        sources.insert(0, active);
        route.sources = sources;
        persist(&state.proxy.home, &next)?;
        *stored = next;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Json(state.proxy.status().await),
        Err(e) => Json(json!({"success":false,"error":e.to_string()})),
    }
}
pub async fn probe(
    State(state): State<crate::HelperState>,
    Json(request): Json<ProbeRequest>,
) -> Json<Value> {
    let result:Result<Value> = async {
        let client=single_client(&request.client)?;
        if request.pair_urls.is_empty() || request.pair_urls.len()>10 { bail!("请选择 1 至 10 个供应商"); }
        let mut sources=Vec::new();
        let mut unavailable=Vec::new();
        for url in &request.pair_urls {
            let mut pair=crate::redeem_pairing(url).await?;
            crate::client_common::apply_pair(&state.home,&mut pair)?;
            if Client::parse(&pair.client)? != vec![client] { bail!("供应商不属于当前客户端"); }
            match extract_upstream(&pair,client) {
                Ok(source)=>sources.push(source),
                Err(error) if error.is::<UnavailableCredential>()=>unavailable.push(json!({"id":pair.provider_id,"success":false,"outcome":"unsupported","error":error.to_string()})),
                Err(error)=>return Err(error),
            }
        }
        let checks=sources.into_iter().map(|source| {
            let http=state.proxy.http.clone();
            let accounts=state.proxy.accounts.clone();
            async move {
                let started=Instant::now();
                if let Some(binding)=&source.binding {
                    return match accounts.models(binding).await {
                        Ok(models)=>json!({"id":source.id,"success":true,"status":200,"modelCount":models.len(),"latencyMs":started.elapsed().as_millis() as u64}),
                        Err(e)=>json!({"id":source.id,"success":false,"error":e.to_string()}),
                    };
                }
                let path=if source.protocol=="google-generative-ai" {"v1beta/models"} else {"v1/models"};
                let mut req=http.get(destination(&source.base_url,path,None).unwrap()).timeout(Duration::from_secs(10));
                for (k,v) in &source.headers { req=req.header(k,v); }
                match req.send().await {
                    Ok(response)=>json!({"id":source.id,"success":response.status().is_success(),"status":response.status().as_u16(),"latencyMs":started.elapsed().as_millis() as u64}),
                    Err(_)=>json!({"id":source.id,"success":false,"error":"连接失败或超时"}),
                }
            }
        });
        unavailable.extend(futures_util::future::join_all(checks).await);
        Ok(json!({"success":true,"results":unavailable}))
    }.await;
    Json(result.unwrap_or_else(|e| json!({"success":false,"error":e.to_string()})))
}
pub async fn disable(
    State(state): State<crate::HelperState>,
    Json(request): Json<ClientRequest>,
) -> Json<Value> {
    match state.proxy.disable(&request.client).await {
        Ok(()) => Json(state.proxy.status().await),
        Err(e) => Json(json!({"success":false,"error":e.to_string()})),
    }
}
pub async fn forward(
    State(state): State<crate::HelperState>,
    RoutePath((client, path)): RoutePath<(String, String)>,
    request: Request,
) -> Response {
    let service = &state.proxy;
    let route = { service.state.lock().await.routes.get(&client).cloned() };
    let Some(route) = route else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "当前客户端未启用本地代理");
    };
    let incoming = request.headers();
    let supplied = incoming
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            incoming
                .get("x-api-key")
                .or_else(|| incoming.get("x-goog-api-key"))
                .and_then(|v| v.to_str().ok())
        })
        .map(str::to_string)
        .or_else(|| {
            request.uri().query().and_then(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .find(|(k, _)| k == "key")
                    .map(|(_, v)| v.into_owned())
            })
        });
    if supplied.as_deref() != Some(route.token.as_str()) {
        return error(StatusCode::UNAUTHORIZED, "本地代理凭证无效");
    };
    if !(path == "v1/messages"
        || path == "v1/responses"
        || path == "v1/chat/completions"
        || path == "v1/models"
        || path == "v1beta/models"
        || path.starts_with("v1beta/models/"))
    {
        return error(StatusCode::NOT_FOUND, "不支持的模型接口");
    };
    let method = request.method().clone();
    let query = request.uri().query().map(str::to_string);
    let original_headers = request.headers().clone();
    let bytes = match to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "请求体过大"),
    };
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => return error(StatusCode::BAD_REQUEST, "请求体必须是 JSON"),
        }
    };

    let started = Instant::now();
    let mut last_status = StatusCode::BAD_GATEWAY;
    let mut attempts: u64 = 0;
    service
        .stats
        .lock()
        .await
        .entry(client.clone())
        .or_default()
        .requests += 1;
    for stored_upstream in route.request_sources() {
        let mut upstream = stored_upstream.clone();
        if let Some(binding) = &upstream.binding {
            if method == axum::http::Method::GET && matches!(path.as_str(), "models" | "v1/models")
            {
                return match service.accounts.models(binding).await {
                    Ok(models)=>Json(json!({"object":"list","data":models.into_iter().map(|m|json!({"id":m.id,"object":"model","owned_by":m.owned_by})).collect::<Vec<_>>()})).into_response(),
                    Err(e)=>error(StatusCode::BAD_GATEWAY,&e.to_string()),
                };
            }
            match service.accounts.credential(binding).await {
                Ok(auth) => {
                    upstream.base_url = auth.base;
                    upstream.headers = auth.headers;
                }
                Err(e) => {
                    last_status = StatusCode::UNAUTHORIZED;
                    service
                        .stats
                        .lock()
                        .await
                        .entry(client.clone())
                        .or_default()
                        .last_error = Some(e.to_string());
                    continue;
                }
            }
        }
        {
            let stats = service.stats.lock().await;
            if route.auto_failover_enabled
                && stats
                    .get(&client)
                    .and_then(|s| s.failures.get(&upstream.id))
                    .and_then(|(_, until)| *until)
                    .is_some_and(|until| until > Instant::now())
            {
                continue;
            }
        }
        let mut headers = HeaderMap::new();
        for (name, value) in &original_headers {
            if !matches!(
                name.as_str(),
                "host"
                    | "authorization"
                    | "x-api-key"
                    | "x-goog-api-key"
                    | "cookie"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
                    | "accept-encoding"
            ) {
                headers.insert(name.clone(), value.clone());
            }
        }
        for (name, value) in upstream.headers.iter().chain(route.headers.iter()) {
            let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                return error(StatusCode::BAD_GATEWAY, "上游认证或请求头格式无效");
            };
            headers.insert(name, value);
        }
        let mut payload = body.clone();
        if upstream.id != route.sources[0].id
            && !upstream.model.is_empty()
            && payload.get("model").and_then(Value::as_str) == Some(route.sources[0].model.as_str())
        {
            payload["model"] = json!(upstream.model);
        }
        if upstream.id != route.sources[0].id {
            if let Some((key, _)) = route.sources[0].models.iter().find(|(_, model)| {
                body.get("model").and_then(Value::as_str) == Some(model.as_str())
            }) {
                if let Some(model) = upstream.models.get(key) {
                    payload["model"] = json!(model);
                }
            }
        }
        let session = original_headers
            .get("session_id")
            .or_else(|| original_headers.get("x-session-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                body.pointer("/metadata/session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                body.pointer("/metadata/user_id")
                    .and_then(Value::as_str)
                    .map(|id| {
                        serde_json::from_str::<Value>(id)
                            .ok()
                            .and_then(|v| {
                                v.get("session_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| {
                                id.rsplit_once("_session_")
                                    .map(|(_, session)| session)
                                    .unwrap_or(id)
                                    .to_string()
                            })
                    })
            });
        let mut reasoning = upstream.reasoning.clone();
        if let Some(config) = reasoning.as_mut() {
            config.effort_levels = upstream
                .catalog
                .iter()
                .find(|m| m.get("id").or_else(|| m.get("model")) == payload.get("model"))
                .and_then(|m| m.get("reasoningLevels"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                });
        }
        let prepared = match crate::client_adapters::bridge::prepare(
            crate::client_adapters::bridge::PrepareRequest {
                path: &path,
                protocol: &upstream.protocol,
                body: payload,
                provider: upstream.id.clone(),
                session,
                shadow: service.shadow.clone(),
                reasoning: reasoning.as_ref(),
                codex_oauth: upstream
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.auth_provider == "codex_oauth"),
            },
        ) {
            Ok(prepared) => prepared,
            Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
        let mut payload = prepared.body;
        if !payload.is_null() {
            merge(&mut payload, &route.body);
        }
        if payload.is_object()
            && upstream
                .binding
                .as_ref()
                .is_some_and(|b| b.auth_provider == "codex_oauth")
        {
            payload["stream"] = json!(true);
            payload["store"] = json!(false);
            if payload.get("tools").is_none() {
                payload["tools"] = json!([]);
            }
            if payload.get("parallel_tool_calls").is_none() {
                payload["parallel_tool_calls"] = json!(true);
            }
            if payload.get("instructions").is_none() {
                payload["instructions"] = json!("");
            }
            for key in ["max_output_tokens", "temperature", "top_p"] {
                payload.as_object_mut().map(|o| o.remove(key));
            }
            let include = payload
                .as_object_mut()
                .unwrap()
                .entry("include")
                .or_insert_with(|| json!([]));
            if let Some(values) = include.as_array_mut() {
                if !values.iter().any(|v| v == "reasoning.encrypted_content") {
                    values.push(json!("reasoning.encrypted_content"));
                }
            }
        }
        if upstream
            .binding
            .as_ref()
            .is_some_and(|b| b.auth_provider == "github_copilot")
        {
            let initiator = if path == "v1/messages" {
                crate::client_adapters::copilot_optimizer::classify_request(
                    &body,
                    original_headers.contains_key("anthropic-beta"),
                    true,
                    true,
                )
                .initiator
            } else if body
                .get("input")
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .is_some_and(|v| {
                    v.get("type").and_then(Value::as_str) == Some("function_call_output")
                })
            {
                "agent"
            } else {
                "user"
            };
            headers.insert(
                HeaderName::from_static("x-initiator"),
                HeaderValue::from_static(initiator),
            );
        }
        let bridge = prepared.bridge;
        let converting = bridge.converting();
        let codex_oauth = upstream
            .binding
            .as_ref()
            .is_some_and(|b| b.auth_provider == "codex_oauth");
        let copilot = upstream
            .binding
            .as_ref()
            .is_some_and(|b| b.auth_provider == "github_copilot");
        let upstream_path = if codex_oauth || (copilot && prepared.path != "v1/messages") {
            prepared.path.strip_prefix("v1/").unwrap_or(&prepared.path)
        } else {
            &prepared.path
        };
        let mut url = match destination(&upstream.base_url, upstream_path, query.as_deref()) {
            Ok(url) => url,
            Err(_) => return error(StatusCode::BAD_REQUEST, "上游请求地址无效"),
        };
        if prepared.gemini_sse {
            url.query_pairs_mut().append_pair("alt", "sse");
        }
        if upstream.protocol != "anthropic-messages" {
            headers.remove("anthropic-version");
            headers.remove("anthropic-beta");
        }
        let mut req = service.http.request(method.clone(), url).headers(headers);
        if !payload.is_null() {
            req = req.json(&payload);
        }
        attempts += 1;
        let result =
            tokio::time::timeout(Duration::from_secs(route.timeout_seconds), req.send()).await;
        match result {
            Ok(Ok(response)) => {
                let status = response.status();
                last_status = status;
                if !matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504) {
                    {
                        let mut stats = service.stats.lock().await;
                        let stats = stats.entry(client.clone()).or_default();
                        stats.last_provider = Some(upstream.name.clone());
                        stats.last_latency_ms = started.elapsed().as_millis() as u64;
                        stats.failovers += u64::from(upstream.id != route.sources[0].id);
                        stats.last_error = if status.is_success() {
                            None
                        } else {
                            Some(format!("HTTP {status}"))
                        };
                        stats.failures.remove(&upstream.id);
                        if !status.is_success() {
                            stats.errors += 1;
                        }
                    }
                    let request_model = body
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or(&upstream.model);
                    let actual_model = payload
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or(request_model);
                    let specific = upstream
                        .catalog
                        .iter()
                        .find(|m| {
                            m.get("model")
                                .or_else(|| m.get("id"))
                                .and_then(Value::as_str)
                                == Some(actual_model)
                        })
                        .and_then(|m| m.get("cost"))
                        .cloned();
                    let pricing = match (upstream.pricing.clone(), specific) {
                        (Some(mut default), Some(specific)) => {
                            merge(&mut default, &specific);
                            Some(default)
                        }
                        (default, specific) => specific.or(default),
                    };
                    let mut entry = crate::client_usage::Entry::new(
                        &client,
                        &upstream.id,
                        &upstream.name,
                        request_model,
                        status.as_u16(),
                        attempts,
                        body.get("stream").and_then(Value::as_bool) == Some(true),
                    );
                    entry.model = actual_model.into();
                    if !status.is_success() {
                        entry.error = Some(format!("HTTP {}", status.as_u16()));
                    }
                    let upstream_sse = response
                        .headers()
                        .get("content-type")
                        .and_then(|h| h.to_str().ok())
                        .is_some_and(|h| h.contains("text/event-stream"));
                    let mut observer = crate::client_usage::Observer::new(
                        service.usage.clone(),
                        entry,
                        &upstream.protocol,
                        pricing,
                        started,
                        upstream_sse,
                    );
                    if (converting || codex_oauth)
                        && status.is_success()
                        && body.get("stream").and_then(Value::as_bool) != Some(true)
                    {
                        let bytes = match tokio::time::timeout(
                            Duration::from_secs(route.timeout_seconds),
                            to_bytes(Body::from_stream(response.bytes_stream()), 32 * 1024 * 1024),
                        )
                        .await
                        {
                            Ok(Ok(bytes)) => bytes,
                            _ => {
                                observer.fail(502, "上游响应读取失败或超出限制");
                                return error(
                                    StatusCode::BAD_GATEWAY,
                                    "上游响应读取失败或超出限制",
                                );
                            }
                        };
                        observer.feed(&bytes);
                        observer.complete();
                        let result = if codex_oauth {
                            codex_completed_response(&bytes)
                        } else {
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())
                        }
                        .and_then(|value| bridge.response(value).map_err(|e| e.to_string()));
                        return match result {
                            Ok(value) => Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Body::from(value.to_string()))
                                .unwrap(),
                            Err(e) => {
                                observer.fail(502, "协议转换失败");
                                error(StatusCode::BAD_GATEWAY, &e)
                            }
                        };
                    }
                    let mut reply = Response::builder().status(status);
                    for name in ["content-type", "cache-control", "x-request-id"] {
                        if let Some(value) = response.headers().get(name) {
                            reply = reply.header(name, value);
                        }
                    }
                    let mut source = response.bytes_stream();
                    let idle_seconds = route.timeout_seconds;
                    let stream = async_stream::stream! {
                        loop {
                            match tokio::time::timeout(Duration::from_secs(idle_seconds),source.next()).await {
                                Ok(Some(Ok(bytes)))=>{observer.feed(&bytes);yield Ok::<_,std::io::Error>(bytes)},
                                Ok(Some(Err(error)))=>{yield Err(std::io::Error::other(error));break;},
                                Ok(None)=>{observer.complete();break},
                                Err(_)=>{yield Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"本地代理上游流空闲超时"));break;},
                            }
                        }
                    };
                    // Do not retry after any response bytes have been sent to the client.
                    if converting && status.is_success() {
                        reply = reply.header("content-type", "text/event-stream");
                    }
                    let response_body = if status.is_success() {
                        bridge.stream(stream)
                    } else {
                        Body::from_stream(stream)
                    };
                    return reply.body(response_body).unwrap();
                }
            }
            _ => {
                last_status = StatusCode::GATEWAY_TIMEOUT;
            }
        }
        let mut stats = service.stats.lock().await;
        let stats = stats.entry(client.clone()).or_default();
        let failure = stats.failures.entry(upstream.id.clone()).or_default();
        failure.0 += 1;
        if failure.0 >= route.failure_threshold {
            failure.1 = Some(Instant::now() + Duration::from_secs(route.cooldown_seconds));
        }
    }
    let mut stats = service.stats.lock().await;
    let stats = stats.entry(client.clone()).or_default();
    stats.errors += 1;
    stats.failovers += attempts.saturating_sub(1);
    stats.last_error = Some(format!("全部上游不可用（HTTP {last_status}）"));
    let mut entry = crate::client_usage::Entry::new(
        &client,
        &route.sources[0].id,
        &route.sources[0].name,
        body.get("model").and_then(Value::as_str).unwrap_or(""),
        last_status.as_u16(),
        attempts,
        body.get("stream").and_then(Value::as_bool) == Some(true),
    );
    entry.duration_ms = started.elapsed().as_millis() as u64;
    entry.completed = true;
    entry.error = Some("所有供应商暂时不可用".into());
    let store = service.usage.clone();
    tokio::spawn(async move {
        // 记账失败不能反过来影响已经失败的请求，但也绝不能静默吞掉：
        // 丢的是用量与费用明细，事后无法补，只能靠日志发现。
        if let Err(error) = store.record(&entry).await {
            tracing::warn!(%error, "本地用量记录失败，该次请求不会出现在用量明细中");
        }
    });
    error(last_status, "所有供应商暂时不可用，请稍后重试")
}

fn codex_completed_response(bytes: &[u8]) -> std::result::Result<Value, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "上游响应编码无效".to_string())?;
    for event in text.replace("\r\n", "\n").split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.completed") => {
                return value
                    .get("response")
                    .cloned()
                    .ok_or_else(|| "上游缺少完整响应".into())
            }
            Some("response.failed" | "error") => {
                return Err(value
                    .pointer("/response/error/message")
                    .or_else(|| value.pointer("/error/message"))
                    .and_then(Value::as_str)
                    .unwrap_or("上游生成失败")
                    .to_string())
            }
            _ => {}
        }
    }
    Err("上游流未返回完整响应".into())
}

#[cfg(test)]
mod oauth_response_tests {
    use super::*;
    #[test]
    fn native_detection_uses_actual_credentials_not_the_summary_key() {
        let pair:crate::PairData=serde_json::from_value(json!({"client":"claude","name":"Official","provider_id":"p","base_url":"","api_key":"","model":"","models":{"native":{"client":"claude","settingsConfig":{"env":{"ANTHROPIC_BASE_URL":"https://example.test","ANTHROPIC_API_KEY":"VALID_UPPER_CASE_KEY"}}}}})).unwrap();
        let source = extract_upstream(&pair, Client::Claude).unwrap();
        assert_eq!(
            source.headers.get("x-api-key").unwrap(),
            "VALID_UPPER_CASE_KEY"
        );
        let mut missing = pair;
        missing
            .models
            .as_mut()
            .unwrap()
            .native
            .as_mut()
            .unwrap()
            .settings_config = json!({"env":{}});
        assert!(extract_upstream(&missing, Client::Claude)
            .err()
            .unwrap()
            .is::<UnavailableCredential>());
    }
    #[test]
    fn codex_nonstream_uses_final_response_and_reports_truncation() {
        let bytes=b"event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\r\n\r\nevent: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-qa\",\"output\":[]}}\r\n\r\n";
        assert_eq!(codex_completed_response(bytes).unwrap()["id"], "resp-qa");
        assert!(codex_completed_response(b"data: [DONE]\n\n").is_err());
        assert_eq!(codex_completed_response(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n\n").unwrap_err(),"quota");
    }
}
