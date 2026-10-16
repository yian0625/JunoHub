use crate::error::{AppError, AppResult};

pub fn for_provider(
    provider_type: &str,
    payload: &serde_json::Value,
    previous: &str,
) -> AppResult<String> {
    let value = match payload.get("request_path") {
        None => previous,
        Some(value) => value
            .as_str()
            .ok_or_else(|| AppError::BadRequest("请求路径必须是文本".into()))?,
    };
    let path = validate(value)?;
    if provider_type == "sub2api" && !path.is_empty() {
        return Err(AppError::BadRequest(
            "平台授权池的请求路径由账户引擎自动管理，请使用自动模式".into(),
        ));
    }
    Ok(path)
}

pub fn validate(value: &str) -> AppResult<String> {
    let path = value.trim();
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.len() > 1024
        || !path.starts_with('/')
        || path.starts_with("//")
        || path == "/"
        || path.contains(['?', '#', '\\', '%'])
        || !path.is_ascii()
        || path
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || path.split('/').any(|part| matches!(part, "." | ".."))
        || path.contains("://")
    {
        return Err(AppError::BadRequest(
            "请求路径须以 / 开头，不含域名、查询参数、空格或路径跳转，例如 /v1/chat/completions"
                .into(),
        ));
    }
    Ok(path.to_owned())
}

pub fn target(base: &str, incoming: &str, configured: &str) -> AppResult<reqwest::Url> {
    let (path, query) = incoming
        .split_once('?')
        .map(|(p, q)| (p, Some(q)))
        .unwrap_or((incoming, None));
    // Task URL construction uses / as an internal placeholder with an explicit path.
    if path != "/" || configured.is_empty() { validate(path)?; }
    if ["/v1/responses/", "/responses/", "/backend-api/codex/responses/"].iter().any(|prefix| path.starts_with(prefix))
        && crate::model_api::responses_suffix(path).is_none() {
        return Err(AppError::BadRequest("不支持的 Responses 操作".into()));
    }
    let mut url = if configured.is_empty() {
        crate::upstream_probe::endpoint(
            base,
            path.strip_prefix("/v1/")
                .map(|p| format!("/{p}"))
                .as_deref()
                .unwrap_or(path),
        )?
    } else {
        let configured = validate(configured)?;
        let base = crate::console::validate_base_url(base)?;
        let mut url =
            reqwest::Url::parse(&base).map_err(|_| AppError::BadRequest("连接地址无效".into()))?;
        // Responses follow-up endpoints retain their suffix on a configured generation path.
        let suffix = [
            "/v1/responses/",
            "/responses/",
            "/backend-api/codex/responses/",
        ]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix));
        url.set_path(&match suffix {
            Some(suffix) => format!("{}/{suffix}", configured.trim_end_matches('/')),
            None => configured,
        });
        url.set_query(None);
        url.set_fragment(None);
        url
    };
    if let Some(query) = query {
        url.set_query(Some(query));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incoming_response_suffix_cannot_escape_the_configured_destination() {
        for path in ["/v1/responses/../admin", "/v1/responses/%2e%2e/admin", "/v1/responses/compact/extra", "/v1/responses/resp_other/cancel"] {
            assert!(target("https://supplier.test/v1",path,"/api/responses").is_err());
        }
    }
    #[test]
    fn model_request_path_preserves_defaults_and_overrides_without_duplicate_versions() {
        assert_eq!(
            target("https://example.test/v1", "/v1/chat/completions", "")
                .unwrap()
                .as_str(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            target(
                "https://example.test/v1",
                "/v1/chat/completions?trace=1",
                "/custom/chat"
            )
            .unwrap()
            .as_str(),
            "https://example.test/custom/chat?trace=1"
        );
        assert_eq!(
            target(
                "https://example.test/v1",
                "/v1/responses/compact",
                "/api/responses"
            )
            .unwrap()
            .as_str(),
            "https://example.test/api/responses/compact"
        );
        assert_eq!(
            target(
                "https://example.test/v1",
                "/v1/chat/completions",
                "/v1/chat/completions"
            )
            .unwrap()
            .as_str(),
            "https://example.test/v1/chat/completions"
        );
        for path in [
            "https://other.test/chat",
            "//other.test/chat",
            "/../admin",
            "/%2e%2e/admin",
            "/chat?key=secret",
            "/chat#x",
            "/chat\\x",
            "/chat x",
        ] {
            assert!(validate(path).is_err(), "{path}");
        }
        assert!(for_provider(
            "sub2api",
            &serde_json::json!({"request_path":"/v1/chat/completions"}),
            ""
        )
        .is_err());
        assert!(for_provider("openai", &serde_json::json!({"request_path":123}), "").is_err());
    }
}
