use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Pi's optional cost object requires all four rates when present.
/// Fill its native defaults while preserving extensions and explicit prices.
pub fn normalize(client: &str, config: &Value) -> Value {
    let mut result = config.clone();
    if client == "pi" {
        if let Some(models) = result.get_mut("models").and_then(Value::as_array_mut) {
            for model in models {
                for key in ["name", "api", "baseUrl"] {
                    if model.get(key).and_then(Value::as_str) == Some("") {
                        model.as_object_mut().unwrap().remove(key);
                    }
                }
                if let Some(cost) = model.get_mut("cost").and_then(Value::as_object_mut) {
                    for key in ["input", "output", "cacheRead", "cacheWrite"] {
                        cost.entry(key).or_insert(Value::from(0));
                    }
                    let defaults = cost.clone();
                    if let Some(tiers) = cost.get_mut("tiers").and_then(Value::as_array_mut) {
                        for tier in tiers.iter_mut().filter_map(Value::as_object_mut) {
                            for key in ["input", "output", "cacheRead", "cacheWrite"] {
                                tier.entry(key).or_insert_with(|| defaults[key].clone());
                            }
                        }
                    }
                }
            }
        }
    }
    result
}

/// Validate the native document before either persistence or filesystem mutation.
/// Unknown native fields are preserved for forward compatibility.
pub fn validate(client: &str, config: &Value) -> Result<()> {
    if !config.is_object() {
        bail!("供应商配置必须是 JSON 对象");
    }
    for headers in [config.get("headers"), config.pointer("/options/headers")]
        .into_iter()
        .flatten()
    {
        let headers = headers.as_object().context("请求头必须是对象")?;
        for (name, value) in headers {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
                || !value.as_str().is_some_and(|v| !v.contains(['\r', '\n']))
            {
                bail!("请求头名称或值无效");
            }
        }
    }
    if let Some(models) = config.get("models") {
        let values: Vec<&Value> = match models {
            Value::Array(values) => values.iter().collect(),
            Value::Object(values) => values.values().collect(),
            _ => Vec::new(),
        };
        for model in values {
            for path in [
                "/contextWindow",
                "/maxTokens",
                "/limit/context",
                "/limit/output",
            ] {
                if let Some(value) = model.pointer(path) {
                    if !value.as_u64().is_some_and(|v| v > 0) {
                        bail!("模型上下文和输出上限必须是正整数");
                    }
                }
            }
            if let Some(cost) = model.get("cost") {
                validate_pricing(cost)?;
            }
            if client == "pi" {
                if let Some(map) = model.get("thinkingLevelMap") {
                    let map = map.as_object().context("思考等级映射必须是对象")?;
                    if map.iter().any(|(key, value)| {
                        !matches!(
                            key.as_str(),
                            "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                        ) || !(value.is_null() || value.is_string())
                    }) {
                        bail!("思考等级映射需使用有效档位，映射值必须是字符串或 null");
                    }
                }
            }
            if model.get("reasoning").is_some_and(|v| !v.is_boolean()) {
                bail!("推理能力必须是布尔值");
            }
        }
    }
    match client {
        "claude" | "gemini" => {
            if let Some(env) = config.get("env") {
                let env = env.as_object().context("env 必须是对象")?;
                for (key, value) in env {
                    if !value.is_string() {
                        bail!("环境变量 {key} 必须是字符串");
                    }
                }
            }
            if let Some(settings) = config.get("config") {
                if !settings.is_object() {
                    bail!("config 必须是对象");
                }
            }
        }
        "codex" | "grok" => {
            if client == "codex" {
                if let Some(catalog) = config.get("modelCatalog") {
                    let rows = catalog
                        .as_array()
                        .or_else(|| catalog.get("models").and_then(Value::as_array))
                        .context("模型目录需要 models 数组")?;
                    let mut ids = std::collections::HashSet::new();
                    for row in rows {
                        if let Some(cost) = row.get("cost") {
                            validate_pricing(cost)?;
                        }
                        let id = row
                            .get("model")
                            .or_else(|| row.get("id"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                            .context("模型目录的 ID 不能为空")?;
                        if !ids.insert(id) {
                            bail!("模型目录的 ID 不能重复");
                        }
                        if row
                            .get("contextWindow")
                            .is_some_and(|v| !v.as_u64().is_some_and(|n| n > 0))
                        {
                            bail!("模型目录上下文窗口必须是正整数");
                        }
                    }
                }
            }
            config
                .get("config")
                .and_then(Value::as_str)
                .context("缺少 config TOML 字符串")?
                .parse::<toml_edit::DocumentMut>()
                .context("TOML 配置格式无效")?;
            if let Some(auth) = config.get("auth") {
                if !auth.is_object() {
                    bail!("auth 必须是对象");
                }
            }
        }
        "opencode" => {
            if let Some(options) = config.get("options") {
                if !options.is_object() {
                    bail!("options 必须是对象");
                }
            }
            if let Some(models) = config.get("models") {
                let models = models.as_object().context("OpenCode models 必须是对象")?;
                if models
                    .iter()
                    .any(|(id, value)| id.trim().is_empty() || !value.is_object())
                {
                    bail!("模型需要非空 ID 和有效配置对象");
                }
            }
        }
        "pi" | "openclaw" | "hermes" => {
            if let Some(models) = config.get("models") {
                if client == "hermes" && models.is_object() {
                    return Ok(());
                }
                let models = models.as_array().context("models 必须是数组")?;
                let mut ids = std::collections::HashSet::new();
                for model in models {
                    let id = model
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .context("每个模型需要非空 ID")?;
                    if !ids.insert(id) {
                        bail!("模型 ID 不能重复：{id}");
                    }
                }
            }
        }
        _ => bail!("不支持的客户端"),
    }
    Ok(())
}

pub fn validate_pricing(cost: &Value) -> Result<()> {
    let rates = cost.as_object().context("价格必须是对象")?;
    let mut all = vec![rates];
    if let Some(tiers) = rates.get("tiers") {
        for tier in tiers.as_array().context("价格 tiers 必须是数组")? {
            all.push(tier.as_object().context("价格 tier 必须是对象")?);
        }
    }
    for rates in all {
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            if rates.get(key).is_some_and(|value| {
                !value
                    .as_f64()
                    .is_some_and(|rate| rate.is_finite() && rate >= 0.0)
            }) {
                bail!("价格必须是非负数");
            }
        }
        if rates
            .get("inputTokensAbove")
            .is_some_and(|value| value.as_u64().is_none())
        {
            bail!("阶梯价格的 Token 门槛必须是非负整数");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pi_costs_include_required_rates_and_preserve_tiers() {
        let config = json!({"models":[{"id":"a","name":"","cost":{"input":1.25,"tiers":[{"inputTokensAbove":200000,"output":8}]},"custom":{"keep":true}}]});
        validate("pi", &config).unwrap();
        let result = normalize("pi", &config);
        assert_eq!(result["models"][0]["cost"]["cacheRead"], 0);
        assert_eq!(result["models"][0]["cost"]["tiers"][0]["input"], 1.25);
        assert_eq!(result["models"][0]["cost"]["tiers"][0]["output"], 8);
        assert!(result["models"][0].get("name").is_none());
        assert_eq!(result["models"][0]["custom"], config["models"][0]["custom"]);
    }

    #[test]
    fn malformed_native_models_are_rejected_without_dropping_unknown_capabilities() {
        assert!(validate("pi", &json!({"models":"invalid"})).is_err());
        assert!(validate("pi", &json!({"models":[{"id":"a","contextWindow":-1}]})).is_err());
        assert!(validate("pi", &json!({"models":[{"id":"a","cost":{"input":-1}}]})).is_err());
        assert!(validate("pi", &json!({"headers":{"bad header":"token"}})).is_err());

        assert!(validate("pi", &json!({"models":[{"id":"same"},{"id":"same"}]})).is_err());
        assert!(validate("claude", &json!({"env":{"API_KEY":null}})).is_err());
        assert!(validate(
            "pi",
            &json!({"models":[{"id":"model","futureCapability":{"enabled":true}}]})
        )
        .is_ok());
    }
}
