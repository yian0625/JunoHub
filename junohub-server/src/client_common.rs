//! Compatibility cleanup for previously applied common configuration.
//! Common configuration editing and merging have been removed.
use crate::client_connect::native::NativeProviderConfig;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;
use toml_edit::{DocumentMut, Item, TableLike};
pub fn marker(client: &str) -> Option<&'static str> {
    match client {
        "claude" => Some(".junohub/common-applied-claude.json"),
        "codex" => Some(".junohub/common-applied-codex.json"),
        "gemini" => Some(".junohub/common-applied-gemini.json"),
        _ => None,
    }
}
pub fn applied(home: &Path, client: &str) -> Result<String> {
    let Some(path) = marker(client).map(|p| home.join(p)) else {
        return Ok(String::new());
    };
    if !path.exists() {
        return Ok(String::new());
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(value
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or("")
        .into())
}
pub fn json_remove(target: &mut Value, source: &Value) {
    let (Some(t), Some(s)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    for (k, v) in s {
        if let Some(old) = t.get_mut(k) {
            if old == v {
                t.remove(k);
            } else if old.is_object() && v.is_object() {
                json_remove(old, v);
                if old.as_object().is_some_and(|o| o.is_empty()) {
                    t.remove(k);
                }
            }
        }
    }
}
pub fn remove_table(target: &mut dyn TableLike, source: &dyn TableLike) {
    for (k, v) in source.iter() {
        if let Some(old) = target.get_mut(k) {
            if let (Some(t), Some(s)) = (old.as_table_like_mut(), v.as_table_like()) {
                remove_table(t, s);
                if t.is_empty() {
                    target.remove(k);
                }
            } else if same_item(old, v) {
                target.remove(k);
            }
        }
    }
}
fn same_item(a: &Item, b: &Item) -> bool {
    match (a.as_value(), b.as_value()) {
        (Some(a), Some(b)) => same_value(a, b),
        _ => false,
    }
}
fn same_value(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
    use toml_edit::Value::*;
    match (a, b) {
        (String(a), String(b)) => a.value() == b.value(),
        (Integer(a), Integer(b)) => a.value() == b.value(),
        (Float(a), Float(b)) => a.value() == b.value(),
        (Boolean(a), Boolean(b)) => a.value() == b.value(),
        (Datetime(a), Datetime(b)) => a.value() == b.value(),
        (Array(a), Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| same_value(a, b))
        }
        (InlineTable(a), InlineTable(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|b| same_value(v, b)))
        }
        _ => false,
    }
}
pub fn effective(_home: &Path, native: &NativeProviderConfig) -> Result<NativeProviderConfig> {
    let mut result = native.clone();
    result.common_config_enabled = Some(false);
    result.effective_common_snippet = Some(String::new());
    Ok(result)
}
pub fn apply_pair(home: &Path, pair: &mut crate::PairData) -> Result<()> {
    if let Some(native) = pair.models.as_mut().and_then(|m| m.native.as_mut()) {
        *native = effective(home, native)?;
    }
    Ok(())
}
pub fn strip(home: &Path, client: &str, settings: &mut Value) -> Result<()> {
    let snippet = applied(home, client)?;
    if snippet.trim().is_empty() {
        return Ok(());
    }
    match client {
        "codex" => {
            let mut target = settings
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or("")
                .parse::<DocumentMut>()?;
            remove_table(
                target.as_table_mut(),
                snippet.parse::<DocumentMut>()?.as_table(),
            );
            settings["config"] = json!(target.to_string());
        }
        "claude" => json_remove(settings, &serde_json::from_str(&snippet)?),
        "gemini" => {
            if let Some(env) = settings.get_mut("env") {
                json_remove(env, &serde_json::from_str(&snippet)?);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_common_settings_are_never_applied() {
        let home =
            std::env::temp_dir().join(format!("juno-removed-common-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".junohub")).unwrap();
        std::fs::write(
            home.join(".junohub/common-config.json"),
            r#"{"claude":"{\"env\":{\"MODEL\":\"override\"}}"}"#,
        )
        .unwrap();
        let native: NativeProviderConfig = serde_json::from_value(json!({"client":"claude","commonConfigEnabled":true,"settingsConfig":{"env":{"MODEL":"provider"}}})).unwrap();
        assert_eq!(
            effective(&home, &native).unwrap().settings_config,
            native.settings_config
        );
        std::fs::write(
            home.join(".junohub/common-config.json"),
            "invalid obsolete file",
        )
        .unwrap();
        assert_eq!(
            effective(&home, &native).unwrap().settings_config,
            native.settings_config
        );
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn legacy_cleanup_preserves_external_changes() {
        let mut value = json!({"env":{"KEEP":"own","OLD":"common","EDITED":"new"}});
        json_remove(
            &mut value,
            &json!({"env":{"OLD":"common","EDITED":"common"}}),
        );
        assert_eq!(value, json!({"env":{"KEEP":"own","EDITED":"new"}}));
    }
}
