//! Persistent local proxy usage. Stores accounting metadata, never prompts or credentials.
use crate::client_adapters::{
    sse::{append_utf8_safe, take_sse_block},
    usage_calculator::{CostCalculator, ModelPricing},
    usage_parser::TokenUsage,
};
use anyhow::Result;
use axum::{
    extract::{Query, State},
    Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{path::Path, sync::Arc, time::Instant};
use tokio::sync::OnceCell;
#[derive(Clone)]
pub struct UsageStore {
    pool: SqlitePool,
    initialized: Arc<OnceCell<()>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub at: i64,
    pub client: String,
    pub provider_id: String,
    pub provider: String,
    pub request_model: String,
    pub model: String,
    pub status: u16,
    pub duration_ms: u64,
    pub first_byte_ms: Option<u64>,
    pub attempts: u64,
    pub streaming: bool,
    pub completed: bool,
    pub error: Option<String>,
    pub usage: Option<TokenUsage>,
    pub cost: Option<String>,
}
impl Entry {
    pub fn new(
        client: &str,
        provider_id: &str,
        provider: &str,
        model: &str,
        status: u16,
        attempts: u64,
        streaming: bool,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            at: chrono::Utc::now().timestamp_millis(),
            client: client.into(),
            provider_id: provider_id.into(),
            provider: provider.into(),
            request_model: model.into(),
            model: model.into(),
            status,
            duration_ms: 0,
            first_byte_ms: None,
            attempts,
            streaming,
            completed: false,
            error: None,
            usage: None,
            cost: None,
        }
    }
}
impl UsageStore {
    pub fn new(home: &Path) -> Result<Self> {
        let path = home.join(".junohub/local-usage.sqlite");
        if !path.exists() {
            crate::client_connect::atomic_write(&path, &[])?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        Ok(Self {
            pool: SqlitePoolOptions::new()
                .max_connections(2)
                .connect_lazy_with(options),
            initialized: Arc::new(OnceCell::new()),
        })
    }
    async fn ready(&self) -> Result<()> {
        self.initialized.get_or_try_init(||async{sqlx::query("CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY, at INTEGER NOT NULL, client TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, status INTEGER NOT NULL, body TEXT NOT NULL)").execute(&self.pool).await?;sqlx::query("CREATE INDEX IF NOT EXISTS requests_time ON requests(at DESC)").execute(&self.pool).await?;Ok::<(),anyhow::Error>(())}).await?;
        Ok(())
    }
    pub async fn record(&self, entry: &Entry) -> Result<()> {
        self.ready().await?;
        sqlx::query("INSERT OR REPLACE INTO requests(id,at,client,provider,model,status,body) VALUES(?,?,?,?,?,?,?)").bind(&entry.id).bind(entry.at).bind(&entry.client).bind(&entry.provider).bind(&entry.request_model).bind(entry.status as i64).bind(serde_json::to_string(entry)?).execute(&self.pool).await?;
        Ok(())
    }
    async fn history(&self, query: &Filter) -> Result<Value> {
        self.ready().await?;
        let page = query.page.unwrap_or(1).clamp(1, 100000);
        let client = query.client.as_deref().unwrap_or("");
        let model = query.model.as_deref().unwrap_or("");
        let provider = query.provider.as_deref().unwrap_or("");
        let status = query.status.as_deref().unwrap_or("");
        let clause="WHERE (?='' OR client=?) AND model LIKE ? AND provider LIKE ? AND at>=? AND at<=? AND (?='' OR (?='success' AND status<400 AND json_extract(body,'$.error') IS NULL) OR (?='error' AND (status>=400 OR json_extract(body,'$.error') IS NOT NULL)))";
        // Query strings are static; all filter values are bound parameters.
        let select =
            format!("SELECT body FROM requests {clause} ORDER BY at DESC LIMIT 50 OFFSET ?");
        let values = sqlx::query(&select)
            .bind(client)
            .bind(client)
            .bind(format!("%{model}%"))
            .bind(format!("%{provider}%"))
            .bind(query.from.unwrap_or(0))
            .bind(query.to.unwrap_or(i64::MAX))
            .bind(status)
            .bind(status)
            .bind(status)
            .bind((page - 1) * 50)
            .fetch_all(&self.pool)
            .await?;
        let summary=format!("SELECT COUNT(*) total,SUM(COALESCE(json_extract(body,'$.usage.input_tokens'),0)) input,SUM(COALESCE(json_extract(body,'$.usage.output_tokens'),0)) output,SUM(CAST(json_extract(body,'$.cost') AS REAL)) cost,SUM(CASE WHEN json_extract(body,'$.cost') IS NULL THEN 1 ELSE 0 END) unknown FROM requests {clause}");
        let totals = sqlx::query(&summary)
            .bind(client)
            .bind(client)
            .bind(format!("%{model}%"))
            .bind(format!("%{provider}%"))
            .bind(query.from.unwrap_or(0))
            .bind(query.to.unwrap_or(i64::MAX))
            .bind(status)
            .bind(status)
            .bind(status)
            .fetch_one(&self.pool)
            .await?;
        let list = values
            .into_iter()
            .map(|r| serde_json::from_str::<Value>(r.get("body")))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(
            json!({"success":true,"list":list,"page":page,"pageSize":50,"total":totals.get::<i64,_>("total"),"summary":{"input":totals.get::<Option<i64>,_>("input").unwrap_or(0),"output":totals.get::<Option<i64>,_>("output").unwrap_or(0),"cost":totals.get::<Option<f64>,_>("cost"),"unpriced":totals.get::<Option<i64>,_>("unknown").unwrap_or(0)}}),
        )
    }
}
#[derive(Default, Deserialize)]
pub struct Filter {
    client: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    status: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    page: Option<i64>,
}
pub async fn history(
    State(state): State<crate::HelperState>,
    Query(query): Query<Filter>,
) -> Json<Value> {
    Json(
        state
            .proxy
            .usage
            .history(&query)
            .await
            .unwrap_or_else(|e| json!({"success":false,"error":e.to_string()})),
    )
}
/// Observes the upstream stream before conversion and retains only usage envelopes.
pub struct Observer {
    store: Arc<UsageStore>,
    entry: Option<Entry>,
    protocol: String,
    pricing: Option<Value>,
    start: Instant,
    sse: bool,
    buffer: String,
    remainder: Vec<u8>,
    events: Vec<Value>,
    overflow: bool,
}
impl Observer {
    pub fn new(
        store: Arc<UsageStore>,
        entry: Entry,
        protocol: &str,
        pricing: Option<Value>,
        start: Instant,
        sse: bool,
    ) -> Self {
        Self {
            store,
            entry: Some(entry),
            protocol: protocol.into(),
            pricing,
            start,
            sse,
            buffer: String::new(),
            remainder: vec![],
            events: vec![],
            overflow: false,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.entry.as_ref().unwrap().first_byte_ms.is_none() {
            self.entry.as_mut().unwrap().first_byte_ms =
                Some(self.start.elapsed().as_millis() as u64);
        }
        if self.overflow {
            return;
        }
        append_utf8_safe(&mut self.buffer, &mut self.remainder, bytes);
        if self.sse {
            while let Some(block) = take_sse_block(&mut self.buffer) {
                let data = block
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    if v.get("usage").is_some()
                        || v.get("usageMetadata").is_some()
                        || v.pointer("/message/usage").is_some()
                        || v.pointer("/response/usage").is_some()
                    {
                        let value = accounting_envelope(&v);
                        if self.events.len() >= 64 {
                            self.events.remove(1);
                        }
                        self.events.push(value);
                    }
                }
            }
        }
        if self.buffer.len() > 4 * 1024 * 1024 {
            self.buffer.clear();
            self.overflow = true;
        }
    }
    pub fn complete(&mut self) {
        self.entry.as_mut().unwrap().completed = true;
    }
    pub fn fail(&mut self, status: u16, message: &str) {
        let entry = self.entry.as_mut().unwrap();
        entry.status = status;
        entry.error = Some(message.into());
    }
    fn usage(&self) -> Option<TokenUsage> {
        if self.sse {
            match self.protocol.as_str() {
                "anthropic-messages" => TokenUsage::from_claude_stream_events(&self.events),
                "openai-completions" => TokenUsage::from_openai_stream_events(&self.events),
                "google-generative-ai" => TokenUsage::from_gemini_stream_chunks(&self.events),
                _ => TokenUsage::from_codex_stream_events_auto(&self.events),
            }
            .or_else(|| {
                self.events.iter().rev().find_map(|event| {
                    let usage = event
                        .get("usage")
                        .or_else(|| event.get("usageMetadata"))
                        .or_else(|| event.pointer("/response/usage"))
                        .or_else(|| event.pointer("/message/usage"))?;
                    let pairs = [
                        ("input_tokens", "output_tokens"),
                        ("prompt_tokens", "completion_tokens"),
                        ("promptTokenCount", "candidatesTokenCount"),
                    ];
                    pairs
                        .iter()
                        .any(|(a, b)| {
                            usage.get(a).and_then(Value::as_u64) == Some(0)
                                && usage.get(b).and_then(Value::as_u64) == Some(0)
                        })
                        .then(TokenUsage::default)
                })
            })
        } else {
            let raw: Value = serde_json::from_str(&self.buffer).ok()?;
            let value = accounting_envelope(&raw);
            match self.protocol.as_str() {
                "anthropic-messages" => TokenUsage::from_claude_response(&value),
                "openai-completions" => TokenUsage::from_openai_response(&value),
                "google-generative-ai" => TokenUsage::from_gemini_response(&value),
                _ => TokenUsage::from_codex_response_auto(&value),
            }
        }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        let usage = self.usage();
        let mut entry = self.entry.take().unwrap();
        entry.duration_ms = self.start.elapsed().as_millis() as u64;
        if !entry.completed && entry.error.is_none() {
            entry.error = Some("响应未完整读取".into());
        }
        if let Some(usage) = usage {
            if let Some(model) = &usage.model {
                entry.model = model.clone();
            }
            entry.cost = self
                .pricing
                .as_ref()
                .and_then(|p| price(p, &usage, &self.protocol));
            entry.usage = Some(usage);
        }
        let store = self.store.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = store.record(&entry).await {
                    tracing::warn!("本地用量记录失败: {e}");
                }
            });
        }
    }
}
fn price(value: &Value, usage: &TokenUsage, protocol: &str) -> Option<String> {
    let mut value = value.clone();
    if let Some(tiers) = value.get("tiers").and_then(Value::as_array) {
        if let Some(tier) = tiers
            .iter()
            .filter(|t| {
                t.get("inputTokensAbove")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| (usage.input_tokens as u64) > n)
            })
            .max_by_key(|t| t.get("inputTokensAbove").and_then(Value::as_u64))
        {
            let mut merged = value.clone();
            for (k, v) in tier.as_object()? {
                merged[k] = v.clone();
            }
            value = merged;
        }
    }
    let rate = |key: &str, tokens: u32| -> Option<String> {
        match value.get(key) {
            Some(v) if v.as_f64().is_some_and(|n| n >= 0.0) => Some(v.to_string()),
            _ if tokens == 0 => Some("0".into()),
            _ => None,
        }
    };
    let pricing = ModelPricing::from_strings(
        &rate("input", usage.input_tokens)?,
        &rate("output", usage.output_tokens)?,
        &rate("cacheRead", usage.cache_read_tokens)?,
        &rate("cacheWrite", usage.cache_creation_tokens)?,
    )
    .ok()?;
    Some(
        CostCalculator::calculate_for_app(protocol, usage, &pricing, Decimal::ONE)
            .total_cost
            .to_string(),
    )
}

// Keep only typed accounting fields, including inside usage objects: upstream
// extensions must never smuggle response content or credentials into debug logs.
fn accounting_envelope(source: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ["type", "id", "model", "modelVersion", "responseId"] {
        if let Some(value) = source.get(key).filter(|v| v.is_string()) {
            out.insert(key.into(), value.clone());
        }
    }
    for key in ["usage", "usageMetadata"] {
        if let Some(value) = source.get(key).filter(|v| v.is_object()) {
            out.insert(key.into(), numeric_usage(value));
        }
    }
    for key in ["message", "response"] {
        if let Some(value) = source.get(key).filter(|v| v.is_object()) {
            out.insert(key.into(), accounting_envelope(value));
        }
    }
    Value::Object(out)
}
fn numeric_usage(source: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(source) = source.as_object() {
        for key in [
            "input_tokens",
            "output_tokens",
            "prompt_tokens",
            "completion_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "prompt_cache_hit_tokens",
            "promptTokenCount",
            "candidatesTokenCount",
            "thoughtsTokenCount",
            "cachedContentTokenCount",
            "totalTokenCount",
            "cached_tokens",
            "cache_write_tokens",
            "input_tokens_details",
            "prompt_tokens_details",
            "output_tokens_details",
            "completion_tokens_details",
            "reasoning_tokens",
        ] {
            if let Some(value) = source.get(key) {
                if value.is_u64() {
                    out.insert(key.into(), value.clone());
                } else if value.is_object() {
                    out.insert(key.into(), numeric_usage(value));
                }
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn home() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("juno-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn pricing_preserves_unknown_zero_cache_and_tiers() {
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 100,
            cache_read_tokens: 400,
            ..Default::default()
        };
        let rates = json!({"input":2,"output":4,"cacheRead":0.5});
        assert_eq!(
            price(&rates, &usage, "openai-responses")
                .unwrap()
                .parse::<Decimal>()
                .unwrap(),
            Decimal::new(18, 4)
        );
        assert_eq!(
            price(&rates, &usage, "anthropic-messages")
                .unwrap()
                .parse::<Decimal>()
                .unwrap(),
            Decimal::new(26, 4)
        );
        assert!(price(&json!({"input":2}), &usage, "openai-responses").is_none());
        assert_eq!(
            price(
                &json!({"input":0,"output":0,"cacheRead":0}),
                &usage,
                "openai-responses"
            )
            .unwrap(),
            "0"
        );
        let tier = json!({"input":2,"output":4,"cacheRead":0.5,"tiers":[{"inputTokensAbove":500,"input":4}]});
        assert_eq!(
            price(&tier, &usage, "openai-responses")
                .unwrap()
                .parse::<Decimal>()
                .unwrap(),
            Decimal::new(30, 4)
        );
    }
    #[tokio::test]
    async fn explicit_zero_differs_from_missing_usage() {
        let home = home();
        let store = Arc::new(UsageStore::new(&home).unwrap());
        let mut observer = Observer::new(
            store.clone(),
            Entry::new("claude", "p", "P", "m", 200, 1, true),
            "anthropic-messages",
            None,
            Instant::now(),
            true,
        );
        observer.feed(b"data: {\"type\":\"message_delta\",\"usage\":null}\n\n");
        assert!(observer.usage().is_none());
        observer.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}\n\n");
        assert_eq!(observer.usage().unwrap().input_tokens, 0);
        observer.complete();
        drop(observer);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        store.pool.close().await;
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn envelopes_drop_content_and_untrusted_usage_extensions() {
        let v = accounting_envelope(
            &json!({"output":[{"text":"SECRET_OUTPUT"}],"usage":{"input_tokens":2,"output_tokens":1,"secret":"SECRET_KEY","input_tokens_details":{"cached_tokens":1,"text":"SECRET_PROMPT"}}}),
        );
        let serialized = v.to_string();
        assert!(!serialized.contains("SECRET"));
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 1);
        assert!(TokenUsage::from_codex_response_auto(&accounting_envelope(
            &json!({"error":"no usage","usage":null})
        ))
        .is_none());
    }
    #[tokio::test]
    async fn fragmented_stream_interruption_persistence_and_filtering() {
        let home = home();
        let store = Arc::new(UsageStore::new(&home).unwrap());
        let mut observer = Observer::new(
            store.clone(),
            Entry::new("codex", "p", "Provider", "requested", 200, 2, true),
            "openai-responses",
            Some(json!({"input":2,"output":4})),
            Instant::now(),
            true,
        );
        let event = format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"model":"actual","output":[{"text":"SECRET_OUTPUT"}],"usage":{"input_tokens":10,"output_tokens":2}}})
        );
        for part in event.as_bytes().chunks(7) {
            observer.feed(part);
        }
        assert_eq!(observer.usage().unwrap().input_tokens, 10);
        drop(observer);
        for _ in 0..100 {
            if store.history(&Filter::default()).await.unwrap()["total"] == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let reopened = UsageStore::new(&home).unwrap();
        let history = reopened
            .history(&Filter {
                provider: Some("rovid".into()),
                model: Some("quest".into()),
                status: Some("error".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(history["total"], 1);
        assert_eq!(history["list"][0]["model"], "actual");
        assert_eq!(history["list"][0]["completed"], false);
        assert_eq!(history["list"][0]["error"], "响应未完整读取");
        assert_eq!(history["summary"]["input"], 10);
        assert!(!history.to_string().contains("SECRET"));
        assert_eq!(
            reopened
                .history(&Filter {
                    status: Some("success".into()),
                    ..Default::default()
                })
                .await
                .unwrap()["total"],
            0
        );
        store.pool.close().await;
        reopened.pool.close().await;
        std::fs::remove_dir_all(home).unwrap();
    }
}
