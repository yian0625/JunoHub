use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("{message}")]
    FormFailed {
        reason: &'static str,
        message: &'static str,
        field: &'static str,
    },
    #[error("{0}")]
    ServiceUnavailable(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    GatewayTimeout(String),
    #[error("{0}")]
    BadGateway(String),
    #[error("未授权")]
    Unauthorized,
    #[error("{message}")]
    LoginFailed {
        reason: &'static str,
        message: &'static str,
        field: Option<&'static str>,
    },
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::GatewayTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::FormFailed { .. } => StatusCode::BAD_REQUEST,
            Self::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::LoginFailed { reason: "account_disabled", .. } => StatusCode::FORBIDDEN,
            Self::LoginFailed { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match &self {
            Self::Sql(_) | Self::Http(_) | Self::Json(_) => {
                tracing::error!(kind = match &self {Self::Sql(_) => "database", Self::Http(_) => "http", _ => "json"}, "internal request failure");
                "服务暂时不可用，请稍后重试".to_owned()
            }
            _ => self.to_string(),
        };
        let error_type = match status {
            StatusCode::UNAUTHORIZED => "authentication_error",
            StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
            status if status.is_client_error() => "invalid_request_error",
            _ => "server_error",
        };
        let mut body = json!({
            "code": status.as_u16(), "message": message, "data": null,
            "error": {"message": message, "type": error_type, "code": status.as_u16().to_string()}
        });
        if let Self::LoginFailed { reason, field, .. } = self {
            body["reason"] = json!(reason);
            if let Some(field) = field {
                body["field"] = json!(field);
            }
        }
        if let Self::FormFailed { reason, field, .. } = self {
            body["reason"] = json!(reason);
            body["field"] = json!(field);
        }
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn database_errors_do_not_expose_internal_details() {
        let response = AppError::Sql(sqlx::Error::Protocol("private table and credential details".into())).into_response();
        let bytes = axum::body::to_bytes(response.into_body(),4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private table"));
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["code"],500);
    }

    #[tokio::test]
    async fn model_not_found_is_readable_by_clients_and_console() {
        let response = AppError::NotFound("模型 gpt 5.6 terra 不存在".into()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 404);
        assert_eq!(body["error"]["message"], body["message"]);
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "404");
    }
}
