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
}

pub fn issue_user_token(
    state: &AppState,
    uid: i64,
    subject: String,
    role: String,
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
        },
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(e.to_string()))
}

fn decode_user(state: &AppState, token: &str) -> AppResult<CurrentUser> {
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?
    .claims;
    Ok(CurrentUser {
        sub: claims.sub,
        uid: claims.uid,
        role: claims.role,
    })
}

pub async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer(&request).ok_or(AppError::Unauthorized)?;
    let mut user = decode_user(&state, token)?;
    let row: Option<(String, i64)> = sqlx::query_as("SELECT role,status FROM users WHERE id=?")
        .bind(user.uid)
        .fetch_optional(&state.pool)
        .await?;
    let Some((role, status)) = row else {
        return Err(AppError::Unauthorized);
    };
    if role != "admin" || status != 1 {
        return Err(AppError::Unauthorized);
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
    let mut user = decode_user(&state, token)?;
    let row: Option<(String, i64)> = sqlx::query_as("SELECT role,status FROM users WHERE id=?")
        .bind(user.uid)
        .fetch_optional(&state.pool)
        .await?;
    let Some((role, status)) = row else {
        return Err(AppError::Unauthorized);
    };
    if role != "user" || status != 1 {
        return Err(AppError::Unauthorized);
    }
    user.role = role;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
