use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use toml_edit::{value, DocumentMut, Item, Table};
use url::Url;

#[path = "client_native.rs"]
pub mod native;
pub use native::{read_native_providers, NativeProviderConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Client {
    Claude,
    Codex,
    Gemini,
    Grok,
    OpenCode,
    OpenClaw,
    Hermes,
    Pi,
}

impl Client {
    pub fn parse(value: &str) -> Result<Vec<Self>> {
        if value.contains(',') {
            let mut clients = Vec::new();
            for item in value.split(',') {
                clients.extend(Self::parse(item)?);
            }
            clients.dedup();
            return Ok(clients);
        }
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Ok(vec![Self::Claude]),
            "codex" => Ok(vec![Self::Codex]),
            "gemini" | "gemini-cli" => Ok(vec![Self::Gemini]),
            "grok" | "grokbuild" | "grok-build" => Ok(vec![Self::Grok]),
            "opencode" => Ok(vec![Self::OpenCode]),
            "openclaw" => Ok(vec![Self::OpenClaw]),
            "hermes" => Ok(vec![Self::Hermes]),
            "pi" => Ok(vec![Self::Pi]),
            "all" => Ok(vec![
                Self::Claude,
                Self::Codex,
                Self::Gemini,
                Self::Grok,
                Self::OpenCode,
                Self::OpenClaw,
                Self::Hermes,
                Self::Pi,
            ]),
            _ => bail!("不支持的客户端：{value}"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::Grok => "Grok Build",
            Self::OpenCode => "OpenCode",
            Self::OpenClaw => "OpenClaw",
            Self::Hermes => "Hermes",
            Self::Pi => "Pi",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::OpenClaw => "openclaw",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::OpenClaw => "openclaw",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
        }
    }

    pub fn npm_package(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("@anthropic-ai/claude-code@latest"),
            Self::Codex => Some("@openai/codex@latest"),
            Self::Gemini => Some("@google/gemini-cli@latest"),
            Self::Grok => Some("@xai-official/grok@latest"),
            Self::OpenCode => Some("opencode-ai@latest"),
            Self::OpenClaw => Some("openclaw@latest"),
            Self::Hermes => None,
            Self::Pi => Some("@earendil-works/pi-coding-agent@latest"),
        }
    }
}

#[derive(Debug)]
pub struct ConnectRequest {
    pub clients: Vec<Client>,
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub models: Option<UniversalModels>,
    pub home: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UniversalModels {
    pub native: Option<NativeProviderConfig>,
    pub claude: Option<ClaudeModels>,
    pub codex: Option<CodexModels>,
    pub gemini: Option<GeminiModels>,
    pub grok: Option<SimpleModels>,
    pub opencode: Option<SimpleModels>,
    pub openclaw: Option<SimpleModels>,
    pub hermes: Option<SimpleModels>,
    pub pi: Option<SimpleModels>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModels {
    pub model: String,
    pub haiku_model: Option<String>,
    pub sonnet_model: Option<String>,
    pub opus_model: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModels {
    pub model: String,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GeminiModels {
    pub model: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleModels {
    pub model: String,
    pub protocol: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConnectResult {
    pub backup_id: String,
    pub clients: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupManifest {
    id: String,
    created_at: u64,
    files: Vec<BackupEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupEntry {
    relative_path: String,
    existed: bool,
}

struct PlannedWrite {
    relative_path: &'static str,
    contents: Vec<u8>,
}

pub fn default_home() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("无法确定用户主目录，请使用 --home 指定"))
}

fn read_env_map(path: &Path) -> Result<std::collections::HashMap<String, String>> {
    let source = read_optional_text(path)?;
    Ok(source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let raw = value.trim();
            let decoded = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
                serde_json::from_str::<String>(raw)
                    .unwrap_or_else(|_| raw[1..raw.len() - 1].to_string())
            } else if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
                raw[1..raw.len() - 1].to_string()
            } else {
                raw.to_string()
            };
            Some((key.trim().to_string(), decoded))
        })
        .collect())
}

pub fn apply(request: ConnectRequest) -> Result<ConnectResult> {
    let mut writes = Vec::new();
    if let Some(config) = request
        .models
        .as_ref()
        .and_then(|models| models.native.as_ref())
    {
        let clients = Client::parse(&config.client)?;
        if clients.len() != 1 || request.clients != clients {
            bail!("原生供应商配置只能应用到其所属客户端");
        }
        writes = native::plan_native(&request, config)?;
    } else {
        validate_request(&request)?;
        for client in &request.clients {
            match client {
                Client::Claude => writes.push(plan_claude(&request)?),
                Client::Codex => writes.push(plan_codex(&request)?),
                Client::Gemini => writes.extend(plan_gemini(&request)?),
                Client::Grok => writes.push(plan_grok(&request)?),
                Client::OpenCode => writes.push(plan_opencode(&request)?),
                Client::OpenClaw => writes.push(plan_openclaw(&request)?),
                Client::Hermes => writes.push(plan_hermes(&request)?),
                Client::Pi => writes.push(plan_pi(&request)?),
            }
        }
    }
    commit_plan(&request, writes)
}

fn commit_plan(request: &ConnectRequest, writes: Vec<PlannedWrite>) -> Result<ConnectResult> {
    let backup = create_backup(&request.home, &writes)?;
    if let Err(error) = commit_writes(&request.home, &writes) {
        let rollback = restore_manifest(&request.home, &backup);
        return match rollback {
            Ok(()) => Err(error.context("写入失败，已恢复原配置")),
            Err(rollback_error) => Err(error.context(format!(
                "写入失败，自动恢复也失败：{rollback_error}；备份位于 {}",
                backup_dir(&request.home, &backup.id).display()
            ))),
        };
    }
    Ok(ConnectResult {
        backup_id: backup.id,
        clients: request
            .clients
            .iter()
            .map(|client| client.label().to_string())
            .collect(),
        files: writes
            .iter()
            .map(|write| request.home.join(write.relative_path).display().to_string())
            .collect(),
    })
}

pub fn restore(home: &Path, backup_id: &str) -> Result<Vec<String>> {
    if backup_id.is_empty()
        || !backup_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("备份编号无效");
    }
    let manifest_path = backup_dir(home, backup_id).join("manifest.json");
    let manifest: BackupManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("无法读取备份清单 {}", manifest_path.display()))?,
    )
    .context("备份清单格式无效")?;
    restore_manifest(home, &manifest)?;
    Ok(manifest
        .files
        .iter()
        .map(|entry| home.join(&entry.relative_path).display().to_string())
        .collect())
}

fn validate_request(request: &ConnectRequest) -> Result<()> {
    if request.clients.is_empty() {
        bail!("至少选择一个客户端");
    }
    if request.api_key.trim().is_empty() {
        bail!("访问密钥不能为空");
    }
    if request.model.trim().is_empty() {
        bail!("模型别名不能为空");
    }
    reject_multiline("访问密钥", &request.api_key)?;
    reject_multiline("模型别名", &request.model)?;
    let url = Url::parse(request.base_url.trim()).context("JunoHub 地址无效")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("JunoHub 地址必须是完整的 HTTP 或 HTTPS URL");
    }
    Ok(())
}

fn reject_multiline(label: &str, value: &str) -> Result<()> {
    if value.contains(['\n', '\r', '\0']) {
        bail!("{label}不能包含换行或空字符");
    }
    Ok(())
}

fn normalized_openai_url(base_url: &str) -> String {
    let value = base_url.trim().trim_end_matches('/');
    let origin_only = value
        .split_once("://")
        .is_some_and(|(_, remainder)| !remainder.contains('/'));
    if value.ends_with("/v1") || !origin_only {
        value.to_string()
    } else {
        format!("{value}/v1")
    }
}

fn normalized_origin_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn plan_claude(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".claude/settings.json";
    let path = request.home.join(relative_path);
    let mut settings = read_json_object(&path)?;
    let env = object_entry(
        &mut settings,
        "env",
        "Claude Code settings.json 的 env 必须是对象",
    )?;
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        Value::String(normalized_origin_url(&request.base_url)),
    );
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String(request.api_key.clone()),
    );
    let models = request
        .models
        .as_ref()
        .and_then(|models| models.claude.as_ref());
    let primary = models
        .map(|models| models.model.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(&request.model);
    env.insert("ANTHROPIC_MODEL".into(), Value::String(primary.into()));
    for (key, configured) in [
        (
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            models.and_then(|models| models.haiku_model.as_deref()),
        ),
        (
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            models.and_then(|models| models.sonnet_model.as_deref()),
        ),
        (
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            models.and_then(|models| models.opus_model.as_deref()),
        ),
    ] {
        env.insert(
            key.into(),
            Value::String(
                configured
                    .filter(|value| !value.is_empty())
                    .unwrap_or(primary)
                    .into(),
            ),
        );
    }
    Ok(PlannedWrite {
        relative_path,
        contents: serde_json::to_vec_pretty(&settings)?,
    })
}

fn plan_codex(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".codex/config.toml";
    let path = request.home.join(relative_path);
    let source = read_optional_text(&path)?;
    let mut document = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source
            .parse::<DocumentMut>()
            .with_context(|| format!("{} 不是有效的 TOML，未覆盖原文件", path.display()))?
    };
    let models = request
        .models
        .as_ref()
        .and_then(|models| models.codex.as_ref());
    let model = models
        .map(|models| models.model.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(&request.model);
    document["model"] = value(model.trim());
    document["model_provider"] = value("junohub_managed");
    document["model_reasoning_effort"] = value(
        models
            .and_then(|models| models.reasoning_effort.as_deref())
            .unwrap_or("high"),
    );
    document["disable_response_storage"] = value(true);
    ensure_table(&mut document, "model_providers")?;
    let providers = document["model_providers"]
        .as_table_mut()
        .ok_or_else(|| anyhow!("model_providers 必须是 TOML 表"))?;
    if !providers.contains_key("junohub_managed") {
        providers.insert("junohub_managed", Item::Table(Table::new()));
    }
    let provider = providers
        .get_mut("junohub_managed")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow!("model_providers.junohub_managed 必须是 TOML 表"))?;
    provider["name"] = value(request.provider_name.trim());
    provider["base_url"] = value(normalized_openai_url(&request.base_url));
    provider["wire_api"] = value("responses");
    provider["experimental_bearer_token"] = value(request.api_key.trim());
    provider["requires_openai_auth"] = value(false);
    Ok(PlannedWrite {
        relative_path,
        contents: document.to_string().into_bytes(),
    })
}

fn plan_gemini(request: &ConnectRequest) -> Result<Vec<PlannedWrite>> {
    let model = request
        .models
        .as_ref()
        .and_then(|models| models.gemini.as_ref())
        .map(|models| models.model.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(&request.model);
    let env_path = request.home.join(".gemini/.env");
    let env_source = read_optional_text(&env_path)?;
    let env_contents = update_env(
        &env_source,
        &[
            (
                "GOOGLE_GEMINI_BASE_URL",
                normalized_origin_url(&request.base_url),
            ),
            ("GEMINI_API_KEY", request.api_key.trim().to_string()),
            ("GEMINI_MODEL", model.trim().to_string()),
        ],
    );
    let settings_path = request.home.join(".gemini/settings.json");
    let mut settings = read_json_object(&settings_path)?;
    let security = object_entry(
        &mut settings,
        "security",
        "Gemini settings.json 的 security 必须是对象",
    )?;
    let auth = security
        .entry("auth")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("Gemini settings.json 的 security.auth 必须是对象"))?;
    auth.insert(
        "selectedType".into(),
        Value::String("gemini-api-key".into()),
    );
    Ok(vec![
        PlannedWrite {
            relative_path: ".gemini/.env",
            contents: env_contents.into_bytes(),
        },
        PlannedWrite {
            relative_path: ".gemini/settings.json",
            contents: serde_json::to_vec_pretty(&settings)?,
        },
    ])
}

fn client_model(request: &ConnectRequest, client: Client) -> &str {
    let configured = request.models.as_ref().and_then(|models| match client {
        Client::Grok => models.grok.as_ref().map(|value| value.model.as_str()),
        Client::OpenCode => models.opencode.as_ref().map(|value| value.model.as_str()),
        Client::OpenClaw => models.openclaw.as_ref().map(|value| value.model.as_str()),
        Client::Hermes => models.hermes.as_ref().map(|value| value.model.as_str()),
        Client::Pi => models.pi.as_ref().map(|value| value.model.as_str()),
        _ => None,
    });
    configured
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&request.model)
}

fn client_protocol<'a>(request: &'a ConnectRequest, client: Client, fallback: &'a str) -> &'a str {
    request
        .models
        .as_ref()
        .and_then(|models| match client {
            Client::Grok => models.grok.as_ref(),
            Client::OpenCode => models.opencode.as_ref(),
            Client::OpenClaw => models.openclaw.as_ref(),
            Client::Hermes => models.hermes.as_ref(),
            Client::Pi => models.pi.as_ref(),
            _ => None,
        })
        .and_then(|models| models.protocol.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn provider_key(request: &ConnectRequest) -> String {
    let suffix = request
        .provider_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    if suffix.is_empty() {
        "junohub-managed".into()
    } else {
        format!("junohub-{suffix}")
    }
}

fn plan_grok(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".grok/config.toml";
    let path = request.home.join(relative_path);
    let source = read_optional_text(&path)?;
    let mut document = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source
            .parse::<DocumentMut>()
            .with_context(|| format!("{} 不是有效的 TOML，未覆盖原文件", path.display()))?
    };
    let key = provider_key(request);
    let model = client_model(request, Client::Grok).trim();
    ensure_table(&mut document, "models")?;
    document["models"]["default"] = value(&key);
    ensure_table(&mut document, "model")?;
    if !document["model"]
        .as_table()
        .is_some_and(|table| table.contains_key(&key))
    {
        document["model"][&key] = Item::Table(Table::new());
    }
    let profile = document["model"][&key]
        .as_table_mut()
        .ok_or_else(|| anyhow!("model.{key} 必须是 TOML 表"))?;
    profile["model"] = value(model);
    profile["base_url"] = value(normalized_openai_url(&request.base_url));
    profile["name"] = value(request.provider_name.trim());
    profile["api_key"] = value(request.api_key.trim());
    profile["api_backend"] = value(client_protocol(request, Client::Grok, "responses"));
    Ok(PlannedWrite {
        relative_path,
        contents: document.to_string().into_bytes(),
    })
}

fn plan_opencode(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".config/opencode/opencode.json";
    let path = request.home.join(relative_path);
    let mut config = read_json_object(&path)?;
    let providers = object_entry(&mut config, "provider", "OpenCode 的 provider 必须是对象")?;
    let key = provider_key(request);
    let model = client_model(request, Client::OpenCode).trim();
    let npm = client_protocol(request, Client::OpenCode, "@ai-sdk/openai");
    providers.insert(key, json!({
        "name": request.provider_name.trim(),
        "npm": npm,
        "options": {"baseURL": normalized_openai_url(&request.base_url), "apiKey": request.api_key.trim()},
        "models": {model: {"name": model}}
    }));
    Ok(PlannedWrite {
        relative_path,
        contents: serde_json::to_vec_pretty(&config)?,
    })
}

fn plan_openclaw(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".openclaw/openclaw.json";
    let path = request.home.join(relative_path);
    let source = read_optional_text(&path)?;
    let mut config: Value = if source.trim().is_empty() {
        json!({})
    } else {
        json5::from_str(&source)
            .with_context(|| format!("{} 不是有效的 JSON/JSON5，未覆盖原文件", path.display()))?
    };
    let models = object_entry(&mut config, "models", "OpenClaw 的 models 必须是对象")?;
    let providers = models
        .entry("providers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenClaw 的 models.providers 必须是对象"))?;
    let key = provider_key(request);
    let model = client_model(request, Client::OpenClaw).trim();
    let protocol = client_protocol(request, Client::OpenClaw, "openai-responses");
    providers.insert(
        key.clone(),
        json!({
            "baseUrl": normalized_openai_url(&request.base_url),
            "apiKey": request.api_key.trim(),
            "api": protocol,
            "models": [{"id": model, "name": model}]
        }),
    );
    let agents = object_entry(&mut config, "agents", "OpenClaw 的 agents 必须是对象")?;
    let defaults = agents
        .entry("defaults")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenClaw 的 agents.defaults 必须是对象"))?;
    defaults.insert("model".into(), json!({"primary": format!("{key}/{model}")}));
    Ok(PlannedWrite {
        relative_path,
        contents: serde_json::to_vec_pretty(&config)?,
    })
}

fn plan_hermes(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".hermes/config.yaml";
    let path = request.home.join(relative_path);
    let source = read_optional_text(&path)?;
    let mut config: serde_yaml::Value = if source.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(&source)
            .with_context(|| format!("{} 不是有效的 YAML，未覆盖原文件", path.display()))?
    };
    let root = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("Hermes config.yaml 根节点必须是对象"))?;
    let providers_key = serde_yaml::Value::String("custom_providers".into());
    let providers = root
        .entry(providers_key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("Hermes custom_providers 必须是数组"))?;
    let key = provider_key(request);
    let model = client_model(request, Client::Hermes).trim();
    let protocol = client_protocol(request, Client::Hermes, "codex_responses");
    let entry = serde_yaml::to_value(json!({
        "name": key,
        "base_url": normalized_openai_url(&request.base_url),
        "api_key": request.api_key.trim(),
        "api_mode": protocol,
        "models": {model: {}}
    }))?;
    providers.retain(|provider| {
        provider.get("name").and_then(serde_yaml::Value::as_str) != Some(key.as_str())
    });
    providers.push(entry);
    root.insert(
        serde_yaml::Value::String("model".into()),
        serde_yaml::to_value(json!({"default": model, "provider": key}))?,
    );
    Ok(PlannedWrite {
        relative_path,
        contents: serde_yaml::to_string(&config)?.into_bytes(),
    })
}

fn plan_pi(request: &ConnectRequest) -> Result<PlannedWrite> {
    let relative_path = ".pi/agent/models.json";
    let path = request.home.join(relative_path);
    let source = read_optional_text(&path)?;
    let mut config: Value = if source.trim().is_empty() {
        json!({})
    } else {
        json5::from_str(&source)
            .with_context(|| format!("{} 不是有效的 JSON/JSON5，未覆盖原文件", path.display()))?
    };
    let providers = object_entry(
        &mut config,
        "providers",
        "Pi models.json 的 providers 必须是对象",
    )?;
    let key = provider_key(request);
    let model = client_model(request, Client::Pi).trim();
    let protocol = client_protocol(request, Client::Pi, "openai-responses");
    providers.insert(
        key,
        json!({
            "name": request.provider_name.trim(),
            "baseUrl": normalized_openai_url(&request.base_url),
            "api": protocol,
            "apiKey": request.api_key.trim(),
            "models": [{"id": model, "name": model}]
        }),
    );
    Ok(PlannedWrite {
        relative_path,
        contents: serde_json::to_vec_pretty(&config)?,
    })
}

fn read_optional_text(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("无法读取 {}", path.display())),
    }
}

fn read_json_object(path: &Path) -> Result<Value> {
    let source = read_optional_text(path)?;
    if source.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_str(&source)
        .with_context(|| format!("{} 不是有效的 JSON，未覆盖原文件", path.display()))?;
    if !value.is_object() {
        bail!("{} 的根节点必须是 JSON 对象", path.display());
    }
    Ok(value)
}

fn object_entry<'a>(
    value: &'a mut Value,
    key: &str,
    error: &str,
) -> Result<&'a mut Map<String, Value>> {
    let root = value.as_object_mut().ok_or_else(|| anyhow!("{error}"))?;
    let child = root.entry(key).or_insert_with(|| json!({}));
    child.as_object_mut().ok_or_else(|| anyhow!("{error}"))
}

fn ensure_table(document: &mut DocumentMut, key: &str) -> Result<()> {
    if !document.as_table().contains_key(key) {
        document[key] = Item::Table(Table::new());
    }
    if !document[key].is_table() {
        bail!("{key} 必须是 TOML 表，未覆盖原文件");
    }
    Ok(())
}

fn update_env(source: &str, entries: &[(&str, String)]) -> String {
    let managed = entries.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    let mut lines = source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !managed.iter().any(|key| {
                trimmed
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            })
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push("# JunoHub managed connection".into());
    lines.extend(entries.iter().map(|(key, value)| format!("{key}={value}")));
    lines.push(String::new());
    lines.join("\n")
}

fn create_backup(home: &Path, writes: &[PlannedWrite]) -> Result<BackupManifest> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let id = format!("{created_at}-{}", uuid::Uuid::new_v4().simple());
    let directory = backup_dir(home, &id);
    fs::create_dir_all(&directory)
        .with_context(|| format!("无法创建备份目录 {}", directory.display()))?;
    let mut files = Vec::new();
    for write in writes {
        let source = home.join(write.relative_path);
        let existed = source.exists();
        if existed {
            let destination = directory.join(write.relative_path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &destination).with_context(|| {
                format!("无法备份 {} 到 {}", source.display(), destination.display())
            })?;
        }
        files.push(BackupEntry {
            relative_path: write.relative_path.into(),
            existed,
        });
    }
    let manifest = BackupManifest {
        id,
        created_at,
        files,
    };
    atomic_write(
        &directory.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(manifest)
}

fn backup_dir(home: &Path, backup_id: &str) -> PathBuf {
    home.join(".junohub/backups").join(backup_id)
}

fn commit_writes(home: &Path, writes: &[PlannedWrite]) -> Result<()> {
    for write in writes {
        atomic_write(&home.join(write.relative_path), &write.contents)?;
    }
    Ok(())
}

fn restore_manifest(home: &Path, manifest: &BackupManifest) -> Result<()> {
    let directory = backup_dir(home, &manifest.id);
    for entry in &manifest.files {
        let destination = home.join(&entry.relative_path);
        if entry.existed {
            let source = directory.join(&entry.relative_path);
            atomic_write(
                &destination,
                &fs::read(&source)
                    .with_context(|| format!("无法读取备份文件 {}", source.display()))?,
            )?;
        } else if destination.exists() {
            fs::remove_file(&destination)
                .with_context(|| format!("无法删除 {}", destination.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("配置路径无效"))?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.junohub-{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let write_result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("无法创建临时文件 {}", temporary.display()))?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
            .with_context(|| format!("无法替换配置文件 {}", path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

/// Preserve originals for newly managed files when a route changes suppliers.
pub fn extend_backup(home: &Path, original: &str, current: &str) -> Result<()> {
    let target = backup_dir(home, original);
    let source = backup_dir(home, current);
    let mut first: BackupManifest =
        serde_json::from_slice(&fs::read(target.join("manifest.json"))?)?;
    let next: BackupManifest = serde_json::from_slice(&fs::read(source.join("manifest.json"))?)?;
    for file in next.files {
        if first
            .files
            .iter()
            .any(|existing| existing.relative_path == file.relative_path)
        {
            continue;
        }
        if file.existed {
            let destination = target.join(&file.relative_path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            atomic_write(&destination, &fs::read(source.join(&file.relative_path))?)?;
        }
        first.files.push(file);
    }
    atomic_write(
        &target.join("manifest.json"),
        &serde_json::to_vec_pretty(&first)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(home: &Path) -> ConnectRequest {
        ConnectRequest {
            clients: Client::parse("all").unwrap(),
            provider_id: "profile-test".into(),
            provider_name: "JunoHub".into(),
            base_url: "https://hub.example.com/".into(),
            api_key: "sk-juno-test".into(),
            model: "route-model".into(),
            models: None,
            home: home.to_path_buf(),
        }
    }

    #[test]
    fn applies_all_clients_without_overwriting_unrelated_settings_or_codex_login() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Read"]},"env":{"KEEP":"yes"}}"#,
        )
        .unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            "approval_policy = \"on-request\"\n[mcp_servers.demo]\ncommand = \"demo\"\n",
        )
        .unwrap();
        fs::write(home.join(".codex/auth.json"), r#"{"auth_mode":"chatgpt"}"#).unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();
        fs::write(home.join(".gemini/.env"), "KEEP=yes\nGEMINI_MODEL=old\n").unwrap();
        fs::write(
            home.join(".gemini/settings.json"),
            r#"{"mcpServers":{"demo":{"command":"demo"}}}"#,
        )
        .unwrap();

        let result = apply(request(home)).unwrap();
        assert_eq!(result.files.len(), 9);
        let claude: Value =
            serde_json::from_slice(&fs::read(home.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(claude["permissions"]["allow"][0], "Read");
        assert_eq!(claude["env"]["KEEP"], "yes");
        assert_eq!(claude["env"]["ANTHROPIC_MODEL"], "route-model");
        let codex = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert!(codex.contains("approval_policy = \"on-request\""));
        assert!(codex.contains("[mcp_servers.demo]"));
        assert!(codex.contains("[model_providers.junohub_managed]"));
        assert!(codex.contains("experimental_bearer_token = \"sk-juno-test\""));
        assert_eq!(
            fs::read_to_string(home.join(".codex/auth.json")).unwrap(),
            r#"{"auth_mode":"chatgpt"}"#
        );
        let gemini_env = fs::read_to_string(home.join(".gemini/.env")).unwrap();
        assert!(gemini_env.contains("KEEP=yes"));
        let pi: Value =
            json5::from_str(&fs::read_to_string(home.join(".pi/agent/models.json")).unwrap())
                .unwrap();
        assert_eq!(
            pi["providers"]["junohub-profile-test"]["api"],
            "openai-responses"
        );
        assert_eq!(
            pi["providers"]["junohub-profile-test"]["models"][0]["id"],
            "route-model"
        );
        let opencode: Value =
            serde_json::from_slice(&fs::read(home.join(".config/opencode/opencode.json")).unwrap())
                .unwrap();
        assert_eq!(
            opencode["provider"]["junohub-profile-test"]["options"]["apiKey"],
            "sk-juno-test"
        );
        assert_eq!(
            opencode["provider"]["junohub-profile-test"]["models"]["route-model"]["name"],
            "route-model"
        );
        let grok = fs::read_to_string(home.join(".grok/config.toml")).unwrap();
        assert!(grok.contains("default = \"junohub-profile-test\""));
        assert!(grok.contains("api_backend = \"responses\""));
        let openclaw: Value =
            json5::from_str(&fs::read_to_string(home.join(".openclaw/openclaw.json")).unwrap())
                .unwrap();
        assert_eq!(
            openclaw["models"]["providers"]["junohub-profile-test"]["api"],
            "openai-responses"
        );
        assert_eq!(
            openclaw["agents"]["defaults"]["model"]["primary"],
            "junohub-profile-test/route-model"
        );
        let hermes: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(home.join(".hermes/config.yaml")).unwrap())
                .unwrap();
        assert_eq!(
            hermes["model"]["provider"].as_str(),
            Some("junohub-profile-test")
        );
        assert_eq!(
            hermes["custom_providers"][0]["api_mode"].as_str(),
            Some("codex_responses")
        );
        for client in Client::parse("all").unwrap() {
            let imported = read_native_providers(home, client).unwrap();
            assert_eq!(
                imported.len(),
                1,
                "{} 应能重新导入刚写入的供应商",
                client.label()
            );
            let config = serde_json::to_string(&imported[0].settings_config).unwrap();
            assert!(config.contains("route-model"));
            assert!(config.contains("sk-juno-test"));
        }
        assert_eq!(gemini_env.matches("GEMINI_MODEL=").count(), 1);
        let gemini: Value =
            serde_json::from_slice(&fs::read(home.join(".gemini/settings.json")).unwrap()).unwrap();
        assert_eq!(gemini["mcpServers"]["demo"]["command"], "demo");
        assert_eq!(gemini["security"]["auth"]["selectedType"], "gemini-api-key");

        restore(home, &result.backup_id).unwrap();
        assert!(!fs::read_to_string(home.join(".codex/config.toml"))
            .unwrap()
            .contains("junohub"));
        assert_eq!(
            fs::read_to_string(home.join(".gemini/.env")).unwrap(),
            "KEEP=yes\nGEMINI_MODEL=old\n"
        );
    }

    #[test]
    fn invalid_existing_config_fails_before_any_write() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/settings.json"), "{broken").unwrap();
        let error = apply(request(home)).unwrap_err().to_string();
        assert!(error.contains("不是有效的 JSON"));
        assert_eq!(
            fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
            "{broken"
        );
        assert!(!home.join(".codex/config.toml").exists());
    }

    #[test]
    fn applies_distinct_universal_models_for_each_client() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = request(temp.path());
        request.models = Some(UniversalModels {
            claude: Some(ClaudeModels {
                model: "claude-main".into(),
                haiku_model: Some("claude-haiku".into()),
                sonnet_model: Some("claude-sonnet".into()),
                opus_model: Some("claude-opus".into()),
            }),
            codex: Some(CodexModels {
                model: "codex-main".into(),
                reasoning_effort: Some("xhigh".into()),
            }),
            gemini: Some(GeminiModels {
                model: "gemini-main".into(),
            }),
            ..UniversalModels::default()
        });
        apply(request).unwrap();
        let claude: Value =
            serde_json::from_slice(&fs::read(temp.path().join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(claude["env"]["ANTHROPIC_MODEL"], "claude-main");
        assert_eq!(
            claude["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "claude-haiku"
        );
        assert_eq!(claude["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claude-opus");
        let codex = fs::read_to_string(temp.path().join(".codex/config.toml")).unwrap();
        assert!(codex.contains("model = \"codex-main\""));
        assert!(codex.contains("model_reasoning_effort = \"xhigh\""));
        let gemini = fs::read_to_string(temp.path().join(".gemini/.env")).unwrap();
        assert!(gemini.contains("GEMINI_MODEL=gemini-main"));
    }

    #[test]
    fn applies_custom_provider_identity_and_keeps_url_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = request(temp.path());
        request.clients = vec![Client::Claude, Client::Codex, Client::Gemini];
        request.provider_name = "Team Gateway".into();
        request.base_url = "https://gateway.example.com/custom-prefix/".into();
        apply(request).unwrap();

        let claude: Value =
            serde_json::from_slice(&fs::read(temp.path().join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            claude["env"]["ANTHROPIC_BASE_URL"],
            "https://gateway.example.com/custom-prefix"
        );
        let codex = fs::read_to_string(temp.path().join(".codex/config.toml")).unwrap();
        assert!(codex.contains("name = \"Team Gateway\""));
        assert!(codex.contains("base_url = \"https://gateway.example.com/custom-prefix\""));
        let gemini = fs::read_to_string(temp.path().join(".gemini/.env")).unwrap();
        assert!(gemini.contains("GOOGLE_GEMINI_BASE_URL=https://gateway.example.com/custom-prefix"));
    }
}
