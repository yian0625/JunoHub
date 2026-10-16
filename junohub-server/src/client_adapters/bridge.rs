//! Runtime wiring for the upstream CC Switch protocol adapters.
use super::{error::ProxyError, providers::*, types::CodexChatReasoningConfig};
use axum::body::Body;
use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;
use std::sync::Arc;

enum Mode {
    Pass,
    ClaudeChat,
    ClaudeResponses,
    ClaudeGemini,
    CodexChat,
    CodexAnthropic,
}
pub struct Bridge {
    mode: Mode,
    context: transform_codex_chat::CodexToolContext,
    shadow: Arc<gemini_shadow::GeminiShadowStore>,
    provider: String,
    session: Option<String>,
    hints: transform_gemini::AnthropicToolSchemaHints,
}
pub struct Prepared {
    pub path: String,
    pub body: Value,
    pub gemini_sse: bool,
    pub bridge: Bridge,
}

pub struct PrepareRequest<'a> {
    pub path: &'a str,
    pub protocol: &'a str,
    pub body: Value,
    pub provider: String,
    pub session: Option<String>,
    pub shadow: Arc<gemini_shadow::GeminiShadowStore>,
    pub reasoning: Option<&'a CodexChatReasoningConfig>,
    pub codex_oauth: bool,
}

pub fn prepare(request: PrepareRequest<'_>) -> Result<Prepared, ProxyError> {
    let PrepareRequest {
        path,
        protocol,
        body,
        provider,
        session,
        shadow,
        reasoning,
        codex_oauth,
    } = request;
    let mode = match (path, protocol) {
        ("v1/messages", "openai-completions") => Mode::ClaudeChat,
        ("v1/messages", "openai-responses") => Mode::ClaudeResponses,
        ("v1/messages", "google-generative-ai") => Mode::ClaudeGemini,
        ("v1/responses", "openai-completions") => Mode::CodexChat,
        ("v1/responses", "anthropic-messages") => Mode::CodexAnthropic,
        _ => Mode::Pass,
    };
    let context = transform_codex_chat::build_codex_tool_context_from_request(&body);
    let hints = transform_gemini::extract_anthropic_tool_schema_hints(&body);
    let stream = body.get("stream").and_then(Value::as_bool) == Some(true);
    let (upstream_path, payload) = match mode {
        Mode::Pass => (path.to_string(), body),
        Mode::ClaudeChat => (
            "v1/chat/completions".into(),
            transform::anthropic_to_openai(body)?,
        ),
        Mode::ClaudeResponses => (
            "v1/responses".into(),
            transform_responses::anthropic_to_responses(
                body,
                session.as_deref(),
                codex_oauth,
                false,
            )?,
        ),
        Mode::CodexChat => (
            "v1/chat/completions".into(),
            transform_codex_chat::responses_to_chat_completions_with_reasoning(body, reasoning)?,
        ),
        Mode::CodexAnthropic => (
            "v1/messages".into(),
            transform_codex_anthropic::responses_request_to_anthropic(body, 16384)?,
        ),
        Mode::ClaudeGemini => {
            let model = body
                .get("model")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ProxyError::InvalidRequest("缺少 Gemini 模型名称".into()))?;
            let model = url::form_urlencoded::byte_serialize(
                model.strip_prefix("models/").unwrap_or(model).as_bytes(),
            )
            .collect::<String>();
            let action = if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            };
            (
                format!("v1beta/models/{model}:{action}"),
                transform_gemini::anthropic_to_gemini_with_shadow(
                    body,
                    Some(&shadow),
                    Some(&provider),
                    session.as_deref(),
                )?,
            )
        }
    };
    Ok(Prepared {
        path: upstream_path,
        body: payload,
        gemini_sse: matches!(mode, Mode::ClaudeGemini) && stream,
        bridge: Bridge {
            mode,
            context,
            shadow,
            provider,
            session,
            hints,
        },
    })
}
impl Bridge {
    pub fn converting(&self) -> bool {
        !matches!(self.mode, Mode::Pass)
    }
    pub fn response(&self, body: Value) -> Result<Value, ProxyError> {
        match self.mode {
            Mode::Pass => Ok(body),
            Mode::ClaudeChat => transform::openai_to_anthropic(body),
            Mode::ClaudeResponses => transform_responses::responses_to_anthropic(body),
            Mode::ClaudeGemini => transform_gemini::gemini_to_anthropic_with_shadow_and_hints(
                body,
                Some(&self.shadow),
                Some(&self.provider),
                self.session.as_deref(),
                Some(&self.hints),
            ),
            Mode::CodexChat => {
                transform_codex_chat::chat_completion_to_response_with_context(body, &self.context)
            }
            Mode::CodexAnthropic => {
                transform_codex_anthropic::anthropic_response_to_responses_with_context(
                    body,
                    &self.context,
                )
            }
        }
    }
    pub fn stream(
        self,
        stream: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    ) -> Body {
        match self.mode {
            Mode::Pass => Body::from_stream(stream),
            Mode::ClaudeChat => Body::from_stream(streaming::create_anthropic_sse_stream(stream)),
            Mode::ClaudeResponses => Body::from_stream(
                streaming_responses::create_anthropic_sse_stream_from_responses(stream),
            ),
            Mode::ClaudeGemini => {
                Body::from_stream(streaming_gemini::create_anthropic_sse_stream_from_gemini(
                    stream,
                    Some(self.shadow),
                    Some(self.provider),
                    self.session,
                    Some(self.hints),
                ))
            }
            Mode::CodexChat => Body::from_stream(
                streaming_codex_chat::create_responses_sse_stream_from_chat_with_context(
                    stream,
                    self.context,
                ),
            ),
            Mode::CodexAnthropic => Body::from_stream(
                streaming_codex_anthropic::create_responses_sse_stream_from_anthropic_with_context(
                    stream,
                    self.context,
                ),
            ),
        }
    }
}
