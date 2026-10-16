//! Lossless native provider configuration, matching CC Switch's settingsConfig contract.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeProviderConfig {
    pub client: String,
    #[serde(default)]
    pub provider_key: String,
    pub settings_config: Value,
    #[serde(default)]
    pub api_format: Option<String>,
    #[serde(default)]
    pub codex_chat_reasoning: Option<Value>,
    #[serde(default)]
    pub auth_binding: Option<Value>,
    #[serde(default)]
    pub common_config_enabled: Option<bool>,
    #[serde(default)]
    pub pricing: Option<Value>,
    #[serde(skip)]
    pub effective_common_snippet: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeImport {
    pub name: String,
    pub client: String,
    pub provider_key: String,
    pub settings_config: Value,
    pub current: bool,
}

fn json_file(home: &Path, relative: &str) -> Result<Value> {
    let source = read_optional_text(&home.join(relative))?;
    let config = if source.trim().is_empty() {
        json!({})
    } else {
        json5::from_str::<Value>(&source)
            .with_context(|| format!("{relative} 格式无效，未覆盖原文件"))?
    };
    if !config.is_object() {
        bail!("{relative} 必须是对象");
    }
    Ok(config)
}

fn toml_file(home: &Path, relative: &str) -> Result<DocumentMut> {
    read_optional_text(&home.join(relative))?
        .parse::<DocumentMut>()
        .with_context(|| format!("{relative} 不是有效 TOML，未覆盖原文件"))
}

pub fn read_native_providers(home: &Path, client: Client) -> Result<Vec<NativeImport>> {
    let mut result = Vec::new();
    let mut push = |name: String, provider_key: String, settings_config: Value, current: bool| {
        result.push(NativeImport {
            name,
            client: client.key().into(),
            provider_key,
            settings_config,
            current,
        });
    };
    match client {
        Client::Claude => {
            if home.join(".claude/settings.json").exists() {
                push(
                    "当前 Claude 配置".into(),
                    "default".into(),
                    json_file(home, ".claude/settings.json")?,
                    true,
                );
            }
        }
        Client::Codex => {
            if home.join(".codex/config.toml").exists() {
                let document = toml_file(home, ".codex/config.toml")?;
                let key = document
                    .get("model_provider")
                    .and_then(Item::as_str)
                    .unwrap_or("openai");
                let name = document
                    .get("model_providers")
                    .and_then(|v| v.get(key))
                    .and_then(|v| v.get("name"))
                    .and_then(Item::as_str)
                    .unwrap_or(key);
                // OAuth sessions remain on this computer; only static API credentials belong to providers.
                let auth = json_file(home, ".codex/auth.json")?;
                let api_auth = auth
                    .get("OPENAI_API_KEY")
                    .filter(|v| v.is_string())
                    .map(|key| json!({"OPENAI_API_KEY":key}))
                    .unwrap_or_else(|| json!({}));
                let mut settings = json!({"auth":api_auth,"config":document.to_string()});
                if document
                    .get("model_catalog_json")
                    .and_then(Item::as_str)
                    .is_some_and(|p| {
                        std::path::Path::new(p) == home.join(".codex/junohub-model-catalog.json")
                            || p == "junohub-model-catalog.json"
                    })
                {
                    let catalog = json_file(home, ".codex/junohub-model-catalog.json")?;
                    if let Some(source) = catalog.get("junohub_model_catalog") {
                        settings["modelCatalog"] = source.clone();
                    }
                }
                push(name.into(), key.into(), settings, true);
            }
        }
        Client::Gemini => {
            if home.join(".gemini/.env").exists() || home.join(".gemini/settings.json").exists() {
                push(
                    "当前 Gemini 配置".into(),
                    "default".into(),
                    json!({"env":read_env_map(&home.join(".gemini/.env"))?,"config":json_file(home,".gemini/settings.json")?}),
                    true,
                );
            }
        }
        Client::Grok => {
            if home.join(".grok/config.toml").exists() {
                push(
                    "当前 Grok 配置".into(),
                    "default".into(),
                    json!({"config":toml_file(home,".grok/config.toml")?.to_string()}),
                    true,
                );
            }
        }
        Client::OpenCode | Client::OpenClaw | Client::Pi => {
            let (file, pointer) = match client {
                Client::OpenCode => (".config/opencode/opencode.json", "/provider"),
                Client::OpenClaw => (".openclaw/openclaw.json", "/models/providers"),
                _ => (".pi/agent/models.json", "/providers"),
            };
            let config = json_file(home, file)?;
            let pi_settings = if client == Client::Pi {
                json_file(home, ".pi/agent/settings.json")?
            } else {
                json!({})
            };
            let selected = match client {
                Client::OpenCode => config
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                Client::OpenClaw => config
                    .pointer("/agents/defaults/model/primary")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        config
                            .pointer("/agents/defaults/model")
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default(),
                _ => pi_settings
                    .get("defaultProvider")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            };
            if let Some(providers) = config.pointer(pointer).and_then(Value::as_object) {
                for (key, value) in providers {
                    push(
                        value
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(key)
                            .into(),
                        key.clone(),
                        value.clone(),
                        selected == key || selected.starts_with(&format!("{key}/")),
                    );
                }
            }
        }
        Client::Hermes => {
            let source = read_optional_text(&home.join(".hermes/config.yaml"))?;
            let config: Value = if source.trim().is_empty() {
                json!({})
            } else {
                serde_yaml::from_str(&source)?
            };
            let selected = config
                .pointer("/model/provider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(providers) = config.get("providers").and_then(Value::as_object) {
                for (key, value) in providers {
                    let mut value = value.clone();
                    value["_cc_source"] = json!("providers_dict");
                    push(key.clone(), key.clone(), value, selected == key);
                }
            }
            if let Some(providers) = config.get("custom_providers").and_then(Value::as_array) {
                for value in providers {
                    let key = value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("custom");
                    let mut value = value.clone();
                    value["_cc_source"] = json!("custom_providers");
                    push(key.into(), key.into(), value, selected == key);
                }
            }
        }
    }
    for provider in &mut result {
        crate::client_common::strip(home, client.key(), &mut provider.settings_config)?;
    }
    Ok(result)
}

fn merge(target: &mut Value, source: &Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            if value.is_object() && target.get(key).is_some_and(Value::is_object) {
                merge(target.get_mut(key).unwrap(), value);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn json_write(relative_path: &'static str, config: &Value) -> Result<PlannedWrite> {
    Ok(PlannedWrite {
        relative_path,
        contents: serde_json::to_vec_pretty(config)?,
    })
}

pub(super) fn plan_native(
    request: &ConnectRequest,
    native: &NativeProviderConfig,
) -> Result<Vec<PlannedWrite>> {
    let native = crate::client_common::effective(&request.home, native)?;
    let client = request.clients[0];
    let normalized =
        crate::client_config_validation::normalize(client.key(), &native.settings_config);
    let settings = &normalized;
    crate::client_config_validation::validate(client.key(), settings)?;
    if let Some(pricing) = &native.pricing {
        crate::client_config_validation::validate_pricing(pricing)?;
    }
    if !settings.is_object() {
        bail!("供应商配置必须是 JSON 对象");
    }
    let key = if native.provider_key.trim().is_empty() {
        provider_key(request)
    } else {
        native.provider_key.trim().into()
    };
    if key.contains(['\n', '\r', '\0', '/', '\\']) {
        bail!("供应商标识包含无效字符");
    }
    let planned: Result<Vec<PlannedWrite>> = match client {
        Client::Claude => {
            let mut live = json_file(&request.home, ".claude/settings.json")?;
            crate::client_common::strip(&request.home, "claude", &mut live)?;
            // A replacement provider must not inherit the previous provider's credentials/model routing.
            if let Some(env) = live.get_mut("env").and_then(Value::as_object_mut) {
                env.retain(|key, _| {
                    !key.starts_with("ANTHROPIC_") && key != "CLAUDE_CODE_SUBAGENT_MODEL"
                });
            }
            merge(&mut live, settings);
            Ok(vec![json_write(".claude/settings.json", &live)?])
        }
        Client::Codex => {
            let mut stored =
                json!({"config":toml_file(&request.home,".codex/config.toml")?.to_string()});
            crate::client_common::strip(&request.home, "codex", &mut stored)?;
            let mut live = stored["config"].as_str().unwrap().parse::<DocumentMut>()?;
            let source = settings
                .get("config")
                .and_then(Value::as_str)
                .context("Codex 配置需要 config TOML 字符串")?;
            let mut incoming = source.parse::<DocumentMut>().context("Codex TOML 无效")?;
            if let Some(provider) = incoming
                .get("model_provider")
                .and_then(Item::as_str)
                .map(str::to_string)
            {
                if provider != "openai" {
                    if let Some(token) = settings
                        .pointer("/auth/OPENAI_API_KEY")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        incoming["model_providers"][&provider]["experimental_bearer_token"] =
                            value(token);
                        incoming["model_providers"][&provider]["requires_openai_auth"] =
                            value(false);
                    }
                    let entry = incoming
                        .get("model_providers")
                        .and_then(|v| v.get(&provider));
                    if entry
                        .and_then(|v| v.get("requires_openai_auth"))
                        .and_then(Item::as_bool)
                        == Some(true)
                    {
                        let independent =
                            ["env_key", "experimental_bearer_token"].iter().any(|key| {
                                entry
                                    .and_then(|v| v.get(key))
                                    .and_then(Item::as_str)
                                    .is_some_and(|v| !v.trim().is_empty())
                            });
                        if !independent {
                            bail!("第三方 Codex 供应商缺少独立凭证，不能使用官方 ChatGPT 登录；请配置 API Key 或 env_key");
                        }
                        incoming["model_providers"][&provider]["requires_openai_auth"] =
                            value(false);
                    }
                }
            }
            for key in [
                "model_provider",
                "model",
                "model_reasoning_effort",
                "model_catalog_json",
                "model_context_window",
                "model_auto_compact_token_limit",
            ] {
                live.remove(key);
            }
            for (key, item) in incoming.iter() {
                if key == "model_providers" {
                    ensure_table(&mut live, key)?;
                    if let Some(table) = item.as_table() {
                        for (id, provider) in table.iter() {
                            live["model_providers"][id] = provider.clone();
                        }
                    }
                } else {
                    live[key] = item.clone();
                }
            }
            let mut writes = Vec::new();
            if let Some(mut catalog) =
                crate::client_codex_catalog::build(settings, native.api_format.as_deref())?
            {
                let own = live
                    .get("model_catalog_json")
                    .and_then(Item::as_str)
                    .is_none_or(|p| {
                        std::path::Path::new(p)
                            .file_name()
                            .is_some_and(|n| n == "junohub-model-catalog.json")
                    });
                if own {
                    live["model_catalog_json"] = value(
                        request
                            .home
                            .join(".codex/junohub-model-catalog.json")
                            .to_string_lossy()
                            .as_ref(),
                    );
                    catalog["junohub_model_catalog"] = settings["modelCatalog"].clone();
                    writes.push(json_write(".codex/junohub-model-catalog.json", &catalog)?);
                }
            }
            if native.api_format.as_deref().is_some_and(|f| {
                matches!(f, "anthropic" | "anthropic_messages" | "anthropic-messages")
            }) {
                live["web_search"] = value("disabled");
            }
            // auth.json is never overwritten, preserving the user's official ChatGPT login.
            writes.push(PlannedWrite {
                relative_path: ".codex/config.toml",
                contents: live.to_string().into_bytes(),
            });
            Ok(writes)
        }
        Client::Gemini => {
            let mut stored = json!({"env":read_env_map(&request.home.join(".gemini/.env"))?});
            crate::client_common::strip(&request.home, "gemini", &mut stored)?;
            let mut env: std::collections::HashMap<String, String> =
                serde_json::from_value(stored["env"].clone())?;
            for key in ["GEMINI_API_KEY", "GOOGLE_GEMINI_BASE_URL", "GEMINI_MODEL"] {
                env.remove(key);
            }
            if let Some(incoming) = settings.get("env").and_then(Value::as_object) {
                for (key, value) in incoming {
                    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        bail!("Gemini 环境变量名无效");
                    }
                    let value = value.as_str().context("Gemini 环境变量必须为字符串")?;
                    reject_multiline("环境变量", value)?;
                    env.insert(key.clone(), value.into());
                }
            }
            let mut pairs = env.iter().collect::<Vec<_>>();
            pairs.sort_by_key(|(key, _)| *key);
            let text = pairs
                .into_iter()
                .map(|(key, value)| format!("{key}={}\n", serde_json::to_string(value).unwrap()))
                .collect::<String>();
            let mut live = json_file(&request.home, ".gemini/settings.json")?;
            if let Some(config) = settings.get("config") {
                merge(&mut live, config);
            }
            let security = object_entry(&mut live, "security", "Gemini security 必须是对象")?;
            let auth = security.entry("auth").or_insert_with(|| json!({}));
            if !auth.is_object() {
                bail!("Gemini auth 必须是对象");
            }
            auth["selectedType"] = json!(
                if env.get("GEMINI_API_KEY").is_some_and(|v| !v.is_empty()) {
                    "gemini-api-key"
                } else {
                    "oauth-personal"
                }
            );
            Ok(vec![
                PlannedWrite {
                    relative_path: ".gemini/.env",
                    contents: text.into_bytes(),
                },
                json_write(".gemini/settings.json", &live)?,
            ])
        }
        Client::Grok => {
            let mut live = toml_file(&request.home, ".grok/config.toml")?;
            let source = settings
                .get("config")
                .and_then(Value::as_str)
                .context("Grok 配置需要 config TOML 字符串")?;
            let mut incoming = source.parse::<DocumentMut>().context("Grok TOML 无效")?;
            // CC Switch's Grok presets use the Codex-shaped editor contract.
            // Project it into Grok Build's native [models]/[model.<id>] format.
            if let Some(provider) = incoming
                .get("model_provider")
                .and_then(Item::as_str)
                .map(str::to_string)
            {
                let model = incoming
                    .get("model")
                    .and_then(Item::as_str)
                    .unwrap_or("grok-4.5")
                    .to_string();
                let upstream = incoming
                    .get("model_providers")
                    .and_then(|v| v.get(&provider))
                    .cloned()
                    .unwrap_or(Item::None);
                let token = settings
                    .pointer("/auth/OPENAI_API_KEY")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                incoming = DocumentMut::new();
                incoming["models"]["default"] = value(&model);
                incoming["model"][&model]["model"] = value(&model);
                incoming["model"][&model]["name"] = value(
                    upstream
                        .get("name")
                        .and_then(Item::as_str)
                        .unwrap_or(&request.provider_name),
                );
                incoming["model"][&model]["base_url"] = value(
                    upstream
                        .get("base_url")
                        .and_then(Item::as_str)
                        .unwrap_or(""),
                );
                incoming["model"][&model]["api_key"] = value(token);
                incoming["model"][&model]["api_backend"] = value(
                    upstream
                        .get("wire_api")
                        .and_then(Item::as_str)
                        .unwrap_or("responses"),
                );
            }
            for key in ["models", "model"] {
                live.remove(key);
            }
            for (key, item) in incoming.iter() {
                live[key] = item.clone();
            }
            Ok(vec![PlannedWrite {
                relative_path: ".grok/config.toml",
                contents: live.to_string().into_bytes(),
            }])
        }
        Client::OpenCode | Client::OpenClaw | Client::Pi => {
            let file = match client {
                Client::OpenCode => ".config/opencode/opencode.json",
                Client::OpenClaw => ".openclaw/openclaw.json",
                _ => ".pi/agent/models.json",
            };
            let mut live = json_file(&request.home, file)?;
            let providers = match client {
                Client::OpenCode => {
                    object_entry(&mut live, "provider", "OpenCode provider 必须是对象")?
                }
                Client::Pi => object_entry(&mut live, "providers", "Pi providers 必须是对象")?,
                _ => {
                    let models = object_entry(&mut live, "models", "OpenClaw models 必须是对象")?;
                    models
                        .entry("providers")
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .context("OpenClaw providers 必须是对象")?
                }
            };
            providers.insert(key.clone(), settings.clone());
            // Match additive clients: add the complete catalog without changing their default model.
            Ok(vec![json_write(file, &live)?])
        }
        Client::Hermes => {
            let source = read_optional_text(&request.home.join(".hermes/config.yaml"))?;
            let mut live: Value = if source.trim().is_empty() {
                json!({})
            } else {
                serde_yaml::from_str(&source)?
            };
            let mut entry = settings.clone();
            let source = entry.as_object_mut().unwrap().remove("_cc_source");
            if source.as_ref().and_then(Value::as_str) == Some("providers_dict") {
                object_entry(&mut live, "providers", "Hermes providers 必须是对象")?
                    .insert(key.clone(), entry);
            } else {
                entry["name"] = json!(key);
                let root = live.as_object_mut().context("Hermes 配置必须是对象")?;
                let providers = root
                    .entry("custom_providers")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .context("custom_providers 必须为数组")?;
                providers.retain(|p| p.get("name").and_then(Value::as_str) != Some(&key));
                providers.push(entry);
            }
            Ok(vec![PlannedWrite {
                relative_path: ".hermes/config.yaml",
                contents: serde_yaml::to_string(&live)?.into_bytes(),
            }])
        }
    };
    let mut writes = planned?;
    if let Some(marker) = crate::client_common::marker(client.key()).filter(|p| {
        !native
            .effective_common_snippet
            .as_deref()
            .unwrap_or("")
            .is_empty()
            || request.home.join(p).exists()
    }) {
        writes.push(json_write(
            marker,
            &json!({"snippet":native.effective_common_snippet.as_deref().unwrap_or("")}),
        )?);
    }
    Ok(writes)
}

pub fn remove_native_provider(
    home: &Path,
    client: Client,
    provider_key: &str,
) -> Result<ConnectResult> {
    let (file, pointer) = match client {
        Client::Pi => (".pi/agent/models.json", "/providers"),
        Client::OpenCode => (".config/opencode/opencode.json", "/provider"),
        Client::OpenClaw => (".openclaw/openclaw.json", "/models/providers"),
        Client::Hermes => (".hermes/config.yaml", "/providers"),
        _ => bail!("此客户端不支持从配置中移除单个供应商"),
    };
    let mut config = if client == Client::Hermes {
        let source = read_optional_text(&home.join(file))?;
        if source.trim().is_empty() {
            json!({})
        } else {
            serde_yaml::from_str(&source)?
        }
    } else {
        json_file(home, file)?
    };
    let selected = match client {
        Client::Pi => json_file(home, ".pi/agent/settings.json")?
            .get("defaultProvider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Client::OpenCode => config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Client::OpenClaw => config
            .pointer("/agents/defaults/model/primary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => config
            .pointer("/model/provider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    if selected == provider_key || selected.starts_with(&format!("{provider_key}/")) {
        bail!("该供应商是客户端默认供应商，请先切换默认模型再移除");
    }
    if let Some(items) = config.pointer_mut(pointer).and_then(Value::as_object_mut) {
        items.remove(provider_key);
    }
    if client == Client::Hermes {
        if let Some(items) = config
            .get_mut("custom_providers")
            .and_then(Value::as_array_mut)
        {
            items.retain(|item| item.get("name").and_then(Value::as_str) != Some(provider_key));
        }
    }
    let writes = if client == Client::Hermes {
        vec![PlannedWrite {
            relative_path: file,
            contents: serde_yaml::to_string(&config)?.into_bytes(),
        }]
    } else {
        vec![json_write(file, &config)?]
    };
    commit_plan(
        &ConnectRequest {
            clients: vec![client],
            provider_id: String::new(),
            provider_name: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            models: None,
            home: home.into(),
        },
        writes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn apply_config(
        home: &Path,
        client: Client,
        key: &str,
        settings: Value,
    ) -> Result<ConnectResult> {
        apply(ConnectRequest {
            clients: vec![client],
            provider_id: "test-native".into(),
            provider_name: "Test".into(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            home: home.into(),
            models: Some(UniversalModels {
                native: Some(NativeProviderConfig {
                    client: client.key().into(),
                    provider_key: key.into(),
                    settings_config: settings,
                    api_format: None,
                    codex_chat_reasoning: None,
                    auth_binding: None,
                    common_config_enabled: None,
                    effective_common_snippet: None,
                    pricing: None,
                }),
                ..Default::default()
            }),
        })
    }
    #[test]
    fn pi_import_save_switch_retains_every_model_and_capability() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".pi/agent")).unwrap();
        let entry = json!({"baseUrl":"https://example.test/v1","apiKey":"secret","api":"openai-responses","headers":{"X-Team":"one"},"models":[{"id":"a","name":"A","reasoning":true,"contextWindow":200000,"cost":{"input":2,"output":0,"cacheRead":0,"cacheWrite":0},"compat":{"supportsStore":false}},{"id":"b","name":"B","input":["text","image"]}]});
        fs::write(
            home.join(".pi/agent/models.json"),
            serde_json::to_vec(&json!({"providers":{"original":entry,"keep":{"models":[]}}}))
                .unwrap(),
        )
        .unwrap();
        fs::write(
            home.join(".pi/agent/settings.json"),
            r#"{"defaultProvider":"original","defaultModel":"b","theme":"dark"}"#,
        )
        .unwrap();
        let imported = read_native_providers(home, Client::Pi).unwrap();
        let item = imported
            .iter()
            .find(|p| p.provider_key == "original")
            .unwrap();
        assert!(item.current);
        assert_eq!(item.settings_config, entry);
        let result = apply_config(home, Client::Pi, "copy", item.settings_config.clone()).unwrap();
        let config = json_file(home, ".pi/agent/models.json").unwrap();
        assert_eq!(config["providers"]["copy"], entry);
        assert_eq!(config["providers"]["original"], entry);
        assert!(config["providers"].get("keep").is_some());
        let settings = json_file(home, ".pi/agent/settings.json").unwrap();
        assert_eq!(settings["defaultModel"], "b");
        remove_native_provider(home, Client::Pi, "copy").unwrap();
        assert!(
            json_file(home, ".pi/agent/models.json").unwrap()["providers"]
                .get("copy")
                .is_none()
        );
        assert!(remove_native_provider(home, Client::Pi, "original").is_err());
        restore(home, &result.backup_id).unwrap();
        assert_eq!(
            json_file(home, ".pi/agent/models.json").unwrap()["providers"]["original"],
            entry
        );
    }
    #[test]
    fn official_codex_switch_preserves_login_and_unrelated_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"do-not-export"}}"#;
        fs::write(home.join(".codex/auth.json"), auth).unwrap();
        fs::write(home.join(".codex/config.toml"),"model_provider='old'\nmodel='old'\napproval_policy='on-request'\n[mcp_servers.demo]\ncommand='demo'\n").unwrap();
        let imported = read_native_providers(home, Client::Codex).unwrap();
        assert!(!serde_json::to_string(&imported)
            .unwrap()
            .contains("do-not-export"));
        apply_config(home,Client::Codex,"custom",json!({"auth":{"OPENAI_API_KEY":"new-secret"},"config":"model_provider='custom'\nmodel='new'\n[model_providers.custom]\nbase_url='https://example.test/v1'\nwire_api='responses'\n"})).unwrap();
        let doc = toml_file(home, ".codex/config.toml").unwrap();
        assert_eq!(
            doc["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
            Some("new-secret")
        );
        apply_config(
            home,
            Client::Codex,
            "openai",
            json!({"auth":{},"config":""}),
        )
        .unwrap();
        let doc = toml_file(home, ".codex/config.toml").unwrap();
        assert!(doc.get("model_provider").is_none());
        assert_eq!(doc["approval_policy"].as_str(), Some("on-request"));
        assert!(doc.get("mcp_servers").is_some());
        assert_eq!(
            fs::read_to_string(home.join(".codex/auth.json")).unwrap(),
            auth
        );
    }
    #[test]
    fn gemini_reimport_preserves_quoted_values() {
        let dir = tempfile::tempdir().unwrap();
        let env = json!({"GEMINI_API_KEY":"secret","CUSTOM":r#"value with "quotes" and \slashes"#});
        apply_config(dir.path(), Client::Gemini, "custom", json!({"env":env})).unwrap();
        let imported = read_native_providers(dir.path(), Client::Gemini).unwrap();
        assert_eq!(imported[0].settings_config["env"]["CUSTOM"], env["CUSTOM"]);
    }

    #[test]
    fn malformed_live_file_does_not_get_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".pi/agent")).unwrap();
        fs::write(home.join(".pi/agent/models.json"), "{broken").unwrap();
        assert!(apply_config(home, Client::Pi, "example", json!({"models":[]})).is_err());
        assert_eq!(
            fs::read_to_string(home.join(".pi/agent/models.json")).unwrap(),
            "{broken"
        );
    }
    #[test]
    fn official_claude_clears_old_credentials_but_preserves_user_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/settings.json"),r#"{"env":{"ANTHROPIC_API_KEY":"old","ANTHROPIC_BASE_URL":"https://old.test","KEEP":"yes"},"permissions":{"allow":["Read"]}}"#).unwrap();
        apply_config(home, Client::Claude, "official", json!({"env":{}})).unwrap();
        let config = json_file(home, ".claude/settings.json").unwrap();
        assert!(config["env"].get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(config["env"]["KEEP"], "yes");
        assert_eq!(config["permissions"]["allow"], json!(["Read"]));
    }
    #[test]
    fn codex_catalog_import_and_restore_keep_declared_fields() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let catalog = json!({"models":[{"model":"qa-model","contextWindow":200000,"reasoningLevels":["low","high"],"extra":{"keep":true}}]});
        let applied=apply_config(home,Client::Codex,"qa",json!({"auth":{"OPENAI_API_KEY":"qa-key"},"config":"model_provider='qa'\nmodel='qa-model'\n[model_providers.qa]\nbase_url='https://example.test/v1'\n","modelCatalog":catalog})).unwrap();
        let imported = read_native_providers(home, Client::Codex).unwrap();
        assert_eq!(imported[0].settings_config["modelCatalog"], catalog);
        restore(home, &applied.backup_id).unwrap();
        assert!(!home.join(".codex/junohub-model-catalog.json").exists());
    }
}
