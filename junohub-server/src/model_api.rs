//! A route declares a wire protocol as well as an upstream destination.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ApiType {
    #[default]
    Auto,
    ChatCompletions,
    Responses,
    Messages,
    ImagesGenerations,
    ImagesEdits,
    VideosGenerations,
    AudioGenerations,
    AudioSpeech,
    AudioTranscriptions,
    AudioTranslations,
    Embeddings,
    Search,
    Rerank,
}
impl ApiType {
    pub fn endpoint(&self) -> Option<&'static str> {
        Some(match self {
            Self::Auto => return None,
            Self::ChatCompletions => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::Messages => "/v1/messages",
            Self::ImagesGenerations => "/v1/images/generations",
            Self::ImagesEdits => "/v1/images/edits",
            Self::VideosGenerations => "/v1/videos/generations",
            Self::AudioGenerations => "/v1/audio/generations",
            Self::AudioSpeech => "/v1/audio/speech",
            Self::AudioTranscriptions => "/v1/audio/transcriptions",
            Self::AudioTranslations => "/v1/audio/translations",
            Self::Embeddings => "/v1/embeddings",
            Self::Search => "/v1/search",
            Self::Rerank => "/v1/rerank",
        })
    }
    pub fn is_dialogue(&self) -> bool {
        matches!(
            self,
            Self::Auto | Self::ChatCompletions | Self::Responses | Self::Messages
        )
    }
}
// Only the stateless follow-up actions implemented by this gateway are public.
// Resource retrieval/cancellation needs a separate ownership-aware endpoint.
pub fn responses_suffix(path: &str) -> Option<&str> {
    for prefix in ["/v1/responses", "/responses", "/backend-api/codex/responses"] {
        if let Some(suffix) = path.strip_prefix(prefix) {
            if matches!(suffix, "" | "/compact" | "/input_tokens") { return Some(suffix); }
        }
    }
    None
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    #[default]
    Bearer,
    XApiKey,
}
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    #[default]
    Direct,
    Task,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamMode {
    Preset,
    Custom,
}
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct ApiConfig {
    /// The public JunoHub contract. Legacy routes used this for both sides.
    pub api_type: ApiType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_api_type: Option<ApiType>,
    /// Missing on legacy routes; do not reinterpret their saved destinations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_mode: Option<UpstreamMode>,
    pub auth_type: AuthType,
    /// Optional supplier compatibility marker; never infer support from a model name.
    #[serde(skip_serializing_if = "is_false")]
    pub context_1m: bool,
    pub response_mode: ResponseMode,
    pub task_path: String,
}
impl ApiConfig {
    pub fn upstream_type(&self) -> &ApiType {
        self.upstream_api_type
            .as_ref()
            .filter(|kind| **kind != ApiType::Auto)
            .unwrap_or(&self.api_type)
    }
    pub fn converts_messages_to_chat(&self) -> bool {
        self.api_type == ApiType::Messages && *self.upstream_type() == ApiType::ChatCompletions
    }
    pub fn validate_protocols(&self) -> AppResult<()> {
        if self.context_1m && *self.upstream_type() != ApiType::Messages {
            return Err(AppError::BadRequest("1M 上下文标记仅用于 Messages 上游".into()));
        }
        if self.api_type != *self.upstream_type() && !self.converts_messages_to_chat() {
            return Err(AppError::BadRequest("此上游协议与 JunoHub 接口的转换尚未实现。请选择相同协议，或使用 Messages → Chat Completions 适配".into()));
        }
        Ok(())
    }
    pub fn is_task(&self) -> bool {
        self.response_mode == ResponseMode::Task
    }
    pub fn from_stored(value: &str) -> AppResult<Self> {
        serde_json::from_str(value)
            .map_err(|_| AppError::Internal("模型接口配置无效，请重新保存路由".into()))
    }
    pub fn for_provider(
        provider: &str,
        payload: &Value,
        previous: &str,
        request_path: &str,
    ) -> AppResult<Self> {
        let mut config: Self = match payload.get("api_config") {
            None => Self::from_stored(previous)?,
            Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
                AppError::BadRequest("接口配置无效，请检查接口类型、鉴权和返回方式".into())
            })?,
        };
        config.task_path = config.task_path.trim().to_owned();
        config.validate_protocols()?;
        if provider == "sub2api" {
            if !matches!(config.api_type, ApiType::Auto | ApiType::ChatCompletions | ApiType::Responses)
                || config.upstream_api_type.as_ref().is_some_and(|kind| *kind != ApiType::Auto)
                || config.upstream_mode.is_some()
                || config.auth_type != AuthType::Bearer || config.response_mode != ResponseMode::Direct
                || !config.task_path.is_empty()
            {
                return Err(AppError::BadRequest("平台授权池仅支持 Chat Completions、Responses 入口；上游连接由账户引擎管理".into()));
            }
        } else {
            match config.upstream_mode {
                Some(UpstreamMode::Custom) if request_path.is_empty() || *config.upstream_type() == ApiType::Auto => {
                    return Err(AppError::BadRequest("自定义上游须填写请求路径并指定请求格式".into()));
                }
                Some(UpstreamMode::Preset) if !request_path.is_empty() || !matches!(config.upstream_type(), ApiType::ChatCompletions | ApiType::Responses | ApiType::Messages) => {
                    return Err(AppError::BadRequest("上游预设仅支持三种通用对话格式；覆盖路径请使用自定义连接".into()));
                }
                _ => {}
            }
        }
        if config.is_task() {
            if config.api_type == ApiType::Auto
                || config.api_type.is_dialogue()
                || request_path.is_empty()
            {
                return Err(AppError::BadRequest(
                    "任务接口需要选择扩展接口类型并填写上游提交路径".into(),
                ));
            }
            validate_task_path(&config.task_path)?;
        } else if !config.task_path.is_empty() {
            return Err(AppError::BadRequest(
                "直接返回模式无需配置任务查询路径".into(),
            ));
        }
        Ok(config)
    }
    pub fn check_endpoint(&self, method: &axum::http::Method, incoming: &str) -> AppResult<()> {
        self.validate_protocols()?;
        let path = incoming.split('?').next().unwrap_or(incoming);
        let expected = self.api_type.endpoint().unwrap_or("已配置的模型调用端点");
        let response_path = responses_suffix(path).is_some();
        let matches = if self.api_type == ApiType::Auto {
            [ApiType::ChatCompletions, ApiType::Messages, ApiType::ImagesGenerations,
             ApiType::ImagesEdits, ApiType::VideosGenerations, ApiType::AudioGenerations,
             ApiType::AudioSpeech, ApiType::AudioTranscriptions, ApiType::Embeddings,
             ApiType::AudioTranslations,
             ApiType::Search, ApiType::Rerank].iter().any(|kind| kind.endpoint() == Some(path))
                || path == "/chat/completions" || response_path
        } else {
            path == expected
                || (self.api_type == ApiType::ChatCompletions && path == "/chat/completions")
                || (self.api_type == ApiType::Responses && response_path)
        };
        if !matches {
            return Err(AppError::BadRequest(format!(
                "此模型路由请调用 {expected}，请求格式须与所选接口一致"
            )));
        }
        if *method != axum::http::Method::POST {
            return Err(AppError::BadRequest(
                "模型调用接口需要 POST；任务结果请通过 /v1/tasks/{task_id} 查询".into(),
            ));
        }
        Ok(())
    }
    pub fn upstream_path<'a>(&'a self, configured: &'a str) -> &'a str {
        if configured.is_empty() {
            self.upstream_type().endpoint().unwrap_or("")
        } else {
            configured
        }
    }
    pub fn upstream_target(
        &self,
        base: &str,
        incoming: &str,
        configured: &str,
    ) -> AppResult<reqwest::Url> {
        if !configured.is_empty() || *self.upstream_type() == ApiType::Auto {
            return crate::model_request_path::target(base, incoming, configured);
        }
        let normalized = crate::upstream_probe::compatible_base(base)?;
        let base_url = reqwest::Url::parse(&normalized)
            .map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
        let prefix = base_url.path().trim_end_matches('/');
        let endpoint = self.upstream_type().endpoint().unwrap();
        let suffix = if prefix.ends_with("/v1") {
            endpoint.strip_prefix("/v1").unwrap_or(endpoint)
        } else {
            endpoint
        };
        crate::model_request_path::target(base, incoming, &format!("{prefix}{suffix}"))
    }
}
fn is_false(value: &bool) -> bool { !*value }

pub const CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";

/// Preserve client beta flags, merging the route option into one header value.
/// Called by both generation and diagnostic requests so tests match real forwarding.
pub fn apply_context_1m(
    request: reqwest::RequestBuilder,
    config: &ApiConfig,
    headers: &axum::http::HeaderMap,
) -> reqwest::RequestBuilder {
    if !config.context_1m || *config.upstream_type() != ApiType::Messages { return request; }
    let mut flags = Vec::new();
    for value in headers.get_all("anthropic-beta") {
        if let Ok(value) = value.to_str() {
            for flag in value.split(',').map(str::trim).filter(|flag| !flag.is_empty()) {
                if !flags.contains(&flag) { flags.push(flag); }
            }
        }
    }
    if !flags.contains(&CONTEXT_1M_BETA) { flags.push(CONTEXT_1M_BETA); }
    request.header("anthropic-beta", flags.join(","))
}
pub fn validate_task_path(path: &str) -> AppResult<()> {
    if path.matches("{task_id}").count() != 1
        || path.replace("{task_id}", "task").contains(['{', '}'])
    {
        return Err(AppError::BadRequest(
            "查询路径须包含一个 {task_id}，例如 /api/image/tasks/{task_id}".into(),
        ));
    }
    crate::model_request_path::validate(&path.replace("{task_id}", "task"))?;
    Ok(())
}
pub fn task_target(base: &str, template: &str, task_id: &str) -> AppResult<reqwest::Url> {
    validate_task_path(template)?;
    if task_id.is_empty()
        || task_id.len() > 256
        || !task_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AppError::BadRequest("上游任务标识无效".into()));
    }
    crate::model_request_path::target(base, "/", &template.replace("{task_id}", task_id))
}
pub fn authenticate(
    request: reqwest::RequestBuilder,
    auth: &AuthType,
    secret: &str,
) -> reqwest::RequestBuilder {
    if secret.is_empty() {
        return request;
    }
    match auth {
        AuthType::Bearer => request.bearer_auth(secret),
        AuthType::XApiKey => request.header("x-api-key", secret),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_marker_is_opt_in_and_preserves_client_betas() {
        use axum::http::{HeaderMap, HeaderValue};
        let legacy = ApiConfig::from_stored("{}").unwrap();
        assert!(!legacy.context_1m);
        assert!(serde_json::to_value(&legacy).unwrap().get("context_1m").is_none());
        let mut config = ApiConfig { api_type: ApiType::Messages, context_1m: true, ..Default::default() };
        let saved = serde_json::to_string(&config).unwrap();
        assert_eq!(ApiConfig::from_stored(&saved).unwrap(), config);
        assert!(config.validate_protocols().is_ok());
        let mut headers = HeaderMap::new();
        headers.append("anthropic-beta", HeaderValue::from_static("interleaved-thinking-2025-05-14,context-1m-2025-08-07"));
        headers.append("anthropic-beta", HeaderValue::from_static("prompt-caching-2024-07-31, interleaved-thinking-2025-05-14"));
        let client = reqwest::Client::new();
        let request = apply_context_1m(client.post("https://example.test"), &config, &headers).build().unwrap();
        assert_eq!(request.headers().get_all("anthropic-beta").iter().count(), 1);
        assert_eq!(request.headers()["anthropic-beta"], "interleaved-thinking-2025-05-14,context-1m-2025-08-07,prompt-caching-2024-07-31");
        config.context_1m = false;
        let request = apply_context_1m(client.post("https://example.test"), &config, &headers).build().unwrap();
        assert!(!request.headers().contains_key("anthropic-beta"));
        config.context_1m = true;
        config.upstream_api_type = Some(ApiType::ChatCompletions);
        assert!(config.validate_protocols().is_err());
        assert!(ApiConfig::for_provider("sub2api", &serde_json::json!({"api_config":{"api_type":"messages","context_1m":true}}), "{}", "").is_err());
    }

    #[test]
    fn public_endpoints_never_become_an_arbitrary_supplier_proxy() {
        for config in [ApiConfig::default(), ApiConfig {api_type:ApiType::Responses,..Default::default()}] {
            for path in ["/v1/files", "/v1/admin", "/v1/responses/resp_other", "/v1/responses/../files", "/v1/responses/%2e%2e/admin", "/v1/responses/compact/extra"] {
                assert!(config.check_endpoint(&axum::http::Method::POST,path).is_err(), "{path}");
            }
            for path in ["/v1/responses", "/responses/compact", "/backend-api/codex/responses/input_tokens"] {
                assert!(config.check_endpoint(&axum::http::Method::POST,path).is_ok(), "{path}");
                assert!(config.check_endpoint(&axum::http::Method::GET,path).is_err());
            }
        }
    }
    #[test]
    fn explicit_connection_modes_validate_without_rewriting_either_contract() {
        let custom = serde_json::json!({"api_config":{"api_type":"messages","upstream_api_type":"chat_completions","upstream_mode":"custom"}});
        let config = ApiConfig::for_provider("custom", &custom, "{}", "/vendor/chat").unwrap();
        assert_eq!(config.api_type, ApiType::Messages);
        assert_eq!(*config.upstream_type(), ApiType::ChatCompletions);
        assert!(ApiConfig::for_provider("custom", &custom, "{}", "").is_err());
        let preset = serde_json::json!({"api_config":{"api_type":"messages","upstream_api_type":"messages","upstream_mode":"preset"}});
        assert!(ApiConfig::for_provider("custom", &preset, "{}", "/vendor/messages").is_err());
        let config = ApiConfig::for_provider("custom", &preset, "{}", "").unwrap();
        assert_eq!(config.upstream_target("https://vendor.test/prefix/v1/messages/", "/v1/messages", "").unwrap().as_str(), "https://vendor.test/prefix/v1/messages");
        let media = serde_json::json!({"api_config":{"api_type":"images_generations","upstream_mode":"preset"}});
        assert!(ApiConfig::for_provider("custom", &media, "{}", "").is_err());
        for kind in ["chat_completions", "responses"] {
            let config = ApiConfig::for_provider("sub2api", &serde_json::json!({"api_config":{"api_type":kind}}), "{}", "").unwrap();
            assert_eq!(config.upstream_mode, None);
            assert!(config.check_endpoint(&axum::http::Method::POST, config.api_type.endpoint().unwrap()).is_ok());
        }
    }
    #[test]
    fn api_config_rejects_incompatible_endpoints_and_unsafe_task_paths() {
        let config = ApiConfig::for_provider("custom", &serde_json::json!({"api_config":{"api_type":"images_generations","auth_type":"x_api_key","response_mode":"task","task_path":"/v2/image/tasks/{task_id}"}}), "{}", "/v2/image/generate").unwrap();
        assert!(config
            .check_endpoint(&axum::http::Method::POST, "/v1/images/generations")
            .is_ok());
        assert!(config
            .check_endpoint(&axum::http::Method::POST, "/v1/chat/completions")
            .is_err());
        assert!(config
            .check_endpoint(&axum::http::Method::GET, "/v1/images/generations")
            .is_err());
        for path in [
            "https://evil.test/{task_id}",
            "/tasks",
            "/tasks/{task_id}/{task_id}",
            "/tasks/{other}/{task_id}",
            "/../tasks/{task_id}",
            "/tasks/{task_id}?key=x",
        ] {
            assert!(validate_task_path(path).is_err());
        }
        assert!(task_target("https://example.test/v1", "/tasks/{task_id}", "../other").is_err());
        assert_eq!(
            task_target("https://example.test/v1", "/tasks/{task_id}", "img_123")
                .unwrap()
                .as_str(),
            "https://example.test/tasks/img_123"
        );
        assert!(ApiConfig::for_provider(
            "sub2api",
            &serde_json::json!({"api_config":{"api_type":"images_generations"}}),
            "{}",
            ""
        )
        .is_err());
    }
    #[test]
    fn gateway_protocol_does_not_determine_upstream_destination() {
        let config=ApiConfig::for_provider("openai",&serde_json::json!({"api_config":{"api_type":"messages","upstream_api_type":"chat_completions"}}),"{}","").unwrap();
        assert_eq!(config.upstream_path(""), "/v1/chat/completions");
        assert_eq!(config.api_type.endpoint(), Some("/v1/messages"));
        assert!(config
            .check_endpoint(&axum::http::Method::POST, "/v1/messages")
            .is_ok());
        assert!(config
            .check_endpoint(&axum::http::Method::POST, "/v1/chat/completions")
            .is_err());
        assert_eq!(config.upstream_path("/vendor/chat"), "/vendor/chat");
        assert_eq!(
            config
                .upstream_target("https://example.test/prefix/v1", "/v1/messages?trace=1", "")
                .unwrap()
                .as_str(),
            "https://example.test/prefix/v1/chat/completions?trace=1"
        );
        assert_eq!(
            config
                .upstream_target("https://example.test", "/v1/messages", "")
                .unwrap()
                .as_str(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            config
                .upstream_target(
                    "https://example.test/prefix/v1",
                    "/v1/messages",
                    "/vendor/chat"
                )
                .unwrap()
                .as_str(),
            "https://example.test/vendor/chat"
        );
        assert!(ApiConfig::for_provider("openai",&serde_json::json!({"api_config":{"api_type":"chat_completions","upstream_api_type":"messages"}}),"{}","/vendor/messages").is_err());
        assert!(ApiConfig::for_provider("openai",&serde_json::json!({"api_config":{"api_type":"images_generations","upstream_api_type":"audio_speech"}}),"{}","/vendor/audio").is_err());
        let legacy =
            ApiConfig::from_stored(r#"{"api_type":"messages","auth_type":"x_api_key"}"#).unwrap();
        assert_eq!(*legacy.upstream_type(), ApiType::Messages);
    }
}
