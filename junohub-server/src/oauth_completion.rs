//! Durable continuation after a single-use authorization code has been exchanged.
//! Only an explicitly failed provisioning attempt is retryable. Interrupted work
//! is never automatically replayed, since its external side effects are unknown.
use std::future::Future;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::{
    error::{AppError, AppResult},
    secrets,
    state::AppState,
    store,
};

enum Claim {
    Exchange,
    Resume(String),
    Done(String),
}

async fn claim(pool: &SqlitePool, session_hash: &str, binding: &str) -> AppResult<Claim> {
    let now = store::now();
    sqlx::query("DELETE FROM oauth_completions WHERE expires_at<?")
        .bind(now)
        .execute(pool)
        .await?;
    let inserted = sqlx::query("INSERT OR IGNORE INTO oauth_completions(session_hash,binding_hash,phase,expires_at) VALUES(?,?,'exchange',?)")
        .bind(session_hash).bind(binding).bind(now + 1800).execute(pool).await?;
    if inserted.rows_affected() == 1 {
        return Ok(Claim::Exchange);
    }
    let row = sqlx::query("SELECT binding_hash,phase,encrypted_token,encrypted_result FROM oauth_completions WHERE session_hash=?")
        .bind(session_hash).fetch_one(pool).await?;
    if row.get::<String, _>("binding_hash") != binding {
        return Err(AppError::BadRequest(
            "授权结果与当前会话或目标账户不匹配，请使用本次生成的授权链接。".into(),
        ));
    }
    match row.get::<String, _>("phase").as_str() {
        "done" => Ok(Claim::Done(row.get("encrypted_result"))),
        "retry" => {
            let claimed = sqlx::query("UPDATE oauth_completions SET phase='provision' WHERE session_hash=? AND binding_hash=? AND phase='retry'")
                .bind(session_hash).bind(binding).execute(pool).await?;
            if claimed.rows_affected() != 1 { return Err(busy()); }
            Ok(Claim::Resume(row.get("encrypted_token")))
        }
        "failed" => Err(AppError::BadRequest("本次授权码已尝试兑换，不能再次使用。请重新生成授权链接；重新打开旧授权页不能恢复会话。".into())),
        _ => Err(busy()),
    }
}

fn busy() -> AppError {
    AppError::BadRequest("本次授权正在处理，请勿重复提交。如果服务曾中断，请先检查账户是否已添加，再重新生成授权链接。".into())
}

pub async fn run<T, E, P, EF, PF>(
    state: &AppState,
    uid: i64,
    provider: &str,
    payload: &Value,
    exchange: E,
    provision: P,
) -> AppResult<Value>
where
    T: Serialize + DeserializeOwned,
    E: FnOnce() -> EF,
    P: FnOnce(T) -> PF,
    EF: Future<Output = AppResult<T>>,
    PF: Future<Output = AppResult<Value>>,
{
    let session = payload["session_id"].as_str().unwrap_or_default();
    let oauth_state = payload["state"].as_str().unwrap_or_default();
    let code = payload["code"].as_str().unwrap_or_default();
    if session.is_empty() || oauth_state.is_empty() || code.trim().is_empty() {
        return Err(AppError::BadRequest(
            "缺少本次授权的会话、state 或授权码，请重新生成授权链接。".into(),
        ));
    }
    // Validate ownership and state before reserving a receipt. A forged request
    // must not consume or reserve another user's authorization session.
    let session_hash = store::hash_key(session);
    let binding = store::hash_key(
        &json!([
            uid,
            provider,
            oauth_state,
            code.trim(),
            payload["provider_id"],
            payload["account_id"],
            payload["target_provider_id"]
        ])
        .to_string(),
    );
    let existing: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oauth_completions WHERE session_hash=?)")
            .bind(&session_hash)
            .fetch_one(&state.pool)
            .await?;
    if !existing {
        state
            .oauth_sessions
            .validate_session(uid, provider, session, oauth_state)
            .await?;
    }
    let claimed = claim(&state.pool, &session_hash, &binding).await?;
    let token = match claimed {
        Claim::Done(encrypted) => {
            return Ok(serde_json::from_str(&secrets::decrypt(
                &state.config.encryption_key,
                &encrypted,
            )?)?)
        }
        Claim::Resume(encrypted) => {
            let mut staged: Value =
                serde_json::from_str(&secrets::decrypt(&state.config.encryption_key, &encrypted)?)?;
            let elapsed = store::now().saturating_sub(staged["received_at"].as_i64().unwrap_or(0));
            if let Some(ttl) = staged["token"]["expires_in"].as_i64() {
                let remaining = ttl.saturating_sub(elapsed);
                if remaining <= 60 {
                    sqlx::query("UPDATE oauth_completions SET phase='failed',encrypted_token=NULL WHERE session_hash=?")
                        .bind(&session_hash).execute(&state.pool).await?;
                    return Err(AppError::BadRequest(
                        "暂存的授权凭证已过期，请重新生成授权链接。".into(),
                    ));
                }
                staged["token"]["expires_in"] = json!(remaining);
            }
            serde_json::from_value(staged["token"].take())?
        }
        Claim::Exchange => {
            let token = match exchange().await {
                Ok(token) => token,
                Err(error) => {
                    sqlx::query("UPDATE oauth_completions SET phase='failed' WHERE session_hash=?")
                        .bind(&session_hash)
                        .execute(&state.pool)
                        .await?;
                    return Err(AppError::BadRequest(format!(
                        "授权码兑换失败：{error}。请重新生成授权链接。"
                    )));
                }
            };
            let encrypted = secrets::encrypt(
                &state.config.encryption_key,
                &json!({"received_at":store::now(),"token":token}).to_string(),
            )?;
            sqlx::query("UPDATE oauth_completions SET phase='provision',encrypted_token=? WHERE session_hash=?")
                .bind(encrypted).bind(&session_hash).execute(&state.pool).await?;
            token
        }
    };
    match provision(token).await {
        Ok(result) => {
            let encrypted = secrets::encrypt(&state.config.encryption_key, &result.to_string())?;
            sqlx::query("UPDATE oauth_completions SET phase='done',encrypted_result=?,encrypted_token=NULL WHERE session_hash=?")
                .bind(encrypted).bind(&session_hash).execute(&state.pool).await?;
            Ok(result)
        }
        Err(error) => {
            sqlx::query("UPDATE oauth_completions SET phase='retry' WHERE session_hash=?")
                .bind(&session_hash)
                .execute(&state.pool)
                .await?;
            Err(AppError::BadRequest(format!("授权凭证已取得，但账户初始化或保存失败：{error}。本次凭证暂存 30 分钟，可重试完成授权。")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use reqwest::Client;
    use sqlx::{postgres::PgPoolOptions, sqlite::SqlitePoolOptions};
    use std::{
        net::SocketAddr,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };
    async fn state() -> AppState {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect test database");
        sqlx::migrate!().run(&pool).await.expect("run migrations");
        let timestamp = store::now();
        for email in ["one@example.com", "two@example.com"] {
            sqlx::query("INSERT INTO users(email,password_hash,display_name,role,status,created_at,updated_at) VALUES(?,?,'test','user',1,?,?)")
                .bind(email)
                .bind("unused")
                .bind(timestamp)
                .bind(timestamp)
                .execute(&pool)
                .await
                .expect("insert test user");
        }
        AppState {
            pool,
            sub2_pool: PgPoolOptions::new()
                .connect_lazy("postgres://sub2api:sub2api@127.0.0.1/sub2api")
                .unwrap(),
            sub2_redis: None,
            oauth_sessions: crate::oauth::OAuthSessionStore::default(),
            config: Arc::new(Config {
                bind: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                database_url: "sqlite::memory:".into(),
                jwt_secret: "test-jwt-secret".into(),
                bootstrap_admin_email: String::new(),
                bootstrap_admin_password: String::new(),
                encryption_key: "test-encryption-key".into(),
                email: Default::default(),
                oss: Default::default(),
                sub2api_admin_email: String::new(),
                sub2api_database_url: "postgres://sub2api:sub2api@127.0.0.1/sub2api".into(),
                sub2api_redis_url: "redis://127.0.0.1:6379/0".into(),
                sub2api_slot_ttl_seconds: 1800,
                sub2api_wait_ttl_seconds: 120,
                sub2api_sticky_ttl_seconds: 3600,
                sub2api_response_sticky_ttl_seconds: 3600,
                sub2api_session_hash_read_old_fallback: true,
                sub2api_session_hash_dual_write_old: true,
                scheduler_worker_enabled: false,
                account_engine_gateway_enabled: false,
                allowed_origins: Vec::new(),
                tokenhub_image_hosts: Vec::new(),
                tokenhub_image_models: Vec::new(),
                log_retention_days: 0,
            }),
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn token_survives_provisioning_failure_and_success_is_idempotent() {
        let mut state = state().await;
        state.oauth_sessions = crate::oauth::OAuthSessionStore::persistent(
            state.pool.clone(),
            state.config.encryption_key.clone(),
        );
        let started = state.oauth_sessions.start_openai(7, None).await.unwrap();
        let payload =
            json!({"session_id":started.session_id,"state":started.state,"code":"test-code"});
        let exchanges = AtomicUsize::new(0);
        let provision_count = AtomicUsize::new(0);
        let provisions = &provision_count;
        let bad = run(
            &state,
            8,
            "openai",
            &payload,
            || async { Ok(json!({})) },
            |_| async { Ok(json!({})) },
        )
        .await;
        assert!(bad.is_err());
        let first = run(
            &state,
            7,
            "openai",
            &payload,
            || async {
                exchanges.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"access_token":"private-test-token"}))
            },
            |_| async {
                provisions.fetch_add(1, Ordering::SeqCst);
                Err(AppError::BadRequest("mock onboarding failed".into()))
            },
        )
        .await;
        assert!(first
            .unwrap_err()
            .to_string()
            .contains("mock onboarding failed"));
        let encrypted: String = sqlx::query_scalar("SELECT encrypted_token FROM oauth_completions")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert!(!encrypted.contains("private-test-token"));
        // Recreate the store as after a server restart. Retry must not exchange the code.
        state.oauth_sessions = crate::oauth::OAuthSessionStore::persistent(
            state.pool.clone(),
            state.config.encryption_key.clone(),
        );
        let result = run::<Value, _, _, _, _>(
            &state,
            7,
            "openai",
            &payload,
            || async { panic!("authorization code must not be exchanged twice") },
            |token| async move {
                assert_eq!(token["access_token"], "private-test-token");
                provisions.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"account_id":42}))
            },
        )
        .await
        .unwrap();
        assert_eq!(result["account_id"], 42);
        let repeated = run::<Value, _, _, _, _>(
            &state,
            7,
            "openai",
            &payload,
            || async { panic!("must not exchange") },
            |_| async { panic!("must not create a second account") },
        )
        .await
        .unwrap();
        assert_eq!(repeated, result);
        assert_eq!(exchanges.load(Ordering::SeqCst), 1);
        assert_eq!(provisions.load(Ordering::SeqCst), 2);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM oauth_completions WHERE encrypted_token IS NOT NULL",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
    }
    #[tokio::test]
    async fn receipt_retries_only_failed_provisioning_and_binds_identity() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/0014_oauth_completions.sql"))
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            claim(&pool, "session", "owner-state-target").await.unwrap(),
            Claim::Exchange
        ));
        assert!(claim(&pool, "session", "other-owner").await.is_err());
        assert!(claim(&pool, "session", "owner-state-target").await.is_err());
        sqlx::query("UPDATE oauth_completions SET phase='retry',encrypted_token='ciphertext'")
            .execute(&pool)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            claim(&pool, "session", "owner-state-target"),
            claim(&pool, "session", "owner-state-target")
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let successful = a.or(b).unwrap();
        assert!(matches!(successful, Claim::Resume(token) if token == "ciphertext"));
        sqlx::query("UPDATE oauth_completions SET phase='done',encrypted_result='saved-result',encrypted_token=NULL").execute(&pool).await.unwrap();
        assert!(
            matches!(claim(&pool, "session", "owner-state-target").await.unwrap(), Claim::Done(result) if result == "saved-result")
        );
        assert!(claim(&pool, "session", "other-target").await.is_err());
    }
}
