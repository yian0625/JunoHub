use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurrentUser {
    pub sub: String,
    pub uid: i64,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    uid: i64,
    role: String,
    exp: usize,
    #[serde(default)]
    session_version: i64,
}

#[cfg(test)]
pub fn issue_user_token(state: &AppState, uid:i64, subject:String, role:String) -> AppResult<String> {
    issue_user_token_at_version(state,uid,subject,role,0)
}

pub fn issue_user_token_at_version(
    state: &AppState,
    uid: i64,
    subject: String,
    role: String,
    session_version: i64,
) -> AppResult<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    encode(
        &Header::default(),
        &Claims {
            sub: subject,
            uid,
            role,
            exp: (now + 7 * 24 * 3600) as usize,
            session_version,
        },
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(e.to_string()))
}

fn decode_user(state: &AppState, token: &str) -> AppResult<(CurrentUser,i64)> {
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?
    .claims;
    Ok((CurrentUser {
        sub: claims.sub,
        uid: claims.uid,
        role: claims.role,
    },claims.session_version))
}

pub async fn require_account(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer(&request).ok_or(AppError::Unauthorized)?;
    let (mut user,session_version) = decode_user(&state, token)?;
    let row: Option<(String, i64,i64)> = sqlx::query_as("SELECT role,status,session_version FROM users WHERE id=?")
        .bind(user.uid)
        .fetch_optional(&state.pool)
        .await?;
    let Some((role, status,current_version)) = row else {
        return Err(AppError::Unauthorized);
    };
    if session_version != current_version { return Err(AppError::Unauthorized); }
    if status != 1 || !matches!(role.as_str(), "admin" | "user") {
        return Err(AppError::Unauthorized);
    }
    user.role = role;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

pub async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer(&request).ok_or(AppError::Unauthorized)?;
    let (mut user,session_version) = decode_user(&state, token)?;
    let row: Option<(String, i64,i64)> = sqlx::query_as("SELECT role,status,session_version FROM users WHERE id=?")
        .bind(user.uid)
        .fetch_optional(&state.pool)
        .await?;
    let Some((role, status,current_version)) = row else {
        return Err(AppError::Unauthorized);
    };
    if session_version != current_version { return Err(AppError::Unauthorized); }
    if status != 1 {
        return Err(AppError::Unauthorized);
    }
    if role != "admin" {
        return Err(AppError::Forbidden("此操作仅限管理员".into()));
    }
    user.role = role;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

pub async fn require_user(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer(&request).ok_or(AppError::Unauthorized)?;
    let (mut user,session_version) = decode_user(&state, token)?;
    let row: Option<(String, i64,i64)> = sqlx::query_as("SELECT role,status,session_version FROM users WHERE id=?")
        .bind(user.uid)
        .fetch_optional(&state.pool)
        .await?;
    let Some((role, status,current_version)) = row else {
        return Err(AppError::Unauthorized);
    };
    if session_version != current_version || !matches!(role.as_str(), "user" | "admin") || status != 1 {
        return Err(AppError::Unauthorized);
    }
    user.role = role;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

/// RFC 7235 规定认证方案名大小写不敏感，`bearer` / `BEARER` 都合法。
pub(crate) fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

pub(crate) fn bearer(request: &Request) -> Option<&str> {
    bearer_token(request.headers().get(header::AUTHORIZATION)?.to_str().ok()?)
}
