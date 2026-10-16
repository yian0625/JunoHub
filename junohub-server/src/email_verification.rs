use std::time::Duration;

use axum::{extract::State, Json};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, Rng};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::{config::{EmailConfig, EmailProvider}, console::normalize_email, error::{AppError, AppResult}, models::Envelope, state::AppState, store};

const CODE_TTL: i64 = 300;
const RESEND_DELAY: i64 = 60;
const MAX_ATTEMPTS: i64 = 5;

#[derive(Deserialize)]
pub struct SendCodeInput { pub email: String }

pub async fn send_code(State(state): State<AppState>, Json(input): Json<SendCodeInput>) -> AppResult<Json<Envelope<Value>>> {
    let email = normalize_email(&input.email)?;
    if sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE email=?")
        .bind(&email).fetch_optional(&state.pool).await?.is_some() {
        return Err(AppError::FormFailed { reason: "email_registered", message: "该邮箱已注册，请直接登录。", field: "email" });
    }
    validate_delivery_config(&state.config.email)?;
    let code = format!("{:06}", OsRng.gen_range(0..1_000_000_u32));
    let generation = Uuid::new_v4().to_string();
    let timestamp = store::now();
    reserve(&state.pool, &state.config.email, &state.config.encryption_key, &email, &code, &generation, timestamp).await?;
    let result = deliver(&state.config.email, &email, &code, &generation).await;
    if let Err(error) = result {
        // Keep the send attempt for rate limiting; a failed or ambiguous delivery never enables registration.
        sqlx::query("DELETE FROM registration_codes WHERE email=? AND generation=?")
            .bind(&email).bind(&generation).execute(&state.pool).await?;
        return Err(error);
    }
    let updated = sqlx::query("UPDATE registration_codes SET sent=1 WHERE email=? AND generation=? AND expires_at>?")
        .bind(&email).bind(&generation).bind(store::now()).execute(&state.pool).await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::ServiceUnavailable("验证码已失效，请重新获取。".into()));
    }
    Ok(Json(Envelope::ok(json!({"retry_after": RESEND_DELAY, "expires_in": CODE_TTL}))))
}

fn code_mac(secret: &str, email: &str, generation: &str, code: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    for part in ["junohub-registration", email, generation, code] {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    mac
}

fn code_hash(secret: &str, email: &str, generation: &str, code: &str) -> String {
    hex::encode(code_mac(secret, email, generation, code).finalize().into_bytes())
}

async fn reserve(pool: &SqlitePool, config: &EmailConfig, secret: &str, email: &str, code: &str, generation: &str, timestamp: i64) -> AppResult<()> {
    // Serialize reservations across processes and release the database before contacting the mail service.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM registration_email_sends WHERE created_at<?").bind(timestamp - 86400).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM registration_codes WHERE expires_at<=?").bind(timestamp).execute(&mut *tx).await?;
    let (count, latest): (i64, Option<i64>) = sqlx::query_as("SELECT COUNT(*),MAX(created_at) FROM registration_email_sends WHERE email=?")
        .bind(email).fetch_one(&mut *tx).await?;
    if latest.is_some_and(|last| timestamp - last < RESEND_DELAY) {
        return Err(AppError::TooManyRequests("发送过于频繁，请在 60 秒后重试。".into()));
    }
    if count >= 10 {
        return Err(AppError::TooManyRequests("此邮箱 24 小时内获取验证码次数已达上限，请稍后再试。".into()));
    }
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_email_sends").fetch_one(&mut *tx).await?;
    if total >= config.daily_limit {
        return Err(AppError::TooManyRequests("验证码邮件 24 小时发送额度已用完，请稍后再试。".into()));
    }
    let recent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registration_email_sends WHERE created_at>?")
        .bind(timestamp - 60).fetch_one(&mut *tx).await?;
    if recent >= 5 {
        return Err(AppError::TooManyRequests("验证码邮件发送繁忙，请在一分钟后重试。".into()));
    }
    sqlx::query("INSERT INTO registration_email_sends(generation,email,created_at) VALUES(?,?,?)")
        .bind(generation).bind(email).bind(timestamp).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO registration_codes(email,generation,code_hash,expires_at,attempts,sent) VALUES(?,?,?,?,0,0) ON CONFLICT(email) DO UPDATE SET generation=excluded.generation,code_hash=excluded.code_hash,expires_at=excluded.expires_at,attempts=0,sent=0")
        .bind(email).bind(generation).bind(code_hash(secret, email, generation, code)).bind(timestamp + CODE_TTL).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

fn code_error(reason: &'static str, message: &'static str) -> AppError {
    AppError::FormFailed { reason, message, field: "verification_code" }
}

// The caller commits wrong-code attempts; consuming a valid code and creating its user share one transaction.
pub async fn consume(tx: &mut Transaction<'_, Sqlite>, secret: &str, email: &str, code: &str, timestamp: i64) -> AppResult<()> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(code_error("code_invalid", "请输入邮件中的 6 位验证码。"));
    }
    let row = sqlx::query("SELECT generation,code_hash,expires_at,attempts,sent FROM registration_codes WHERE email=?")
        .bind(email).fetch_optional(&mut **tx).await?
        .ok_or_else(|| code_error("code_missing", "请先获取邮箱验证码。"))?;
    if row.get::<i64, _>("expires_at") <= timestamp {
        return Err(code_error("code_expired", "验证码已过期，请重新获取。"));
    }
    if row.get::<i64, _>("attempts") >= MAX_ATTEMPTS {
        return Err(code_error("code_locked", "验证码输错次数过多，请重新获取。"));
    }
    if row.get::<i64, _>("sent") != 1 {
        return Err(code_error("code_not_sent", "验证码邮件尚未发送成功，请稍后重新获取。"));
    }
    let stored: String = row.get("code_hash");
    let matches = hex::decode(stored).ok().is_some_and(|digest| code_mac(secret, email, row.get("generation"), code).verify_slice(&digest).is_ok());
    if !matches {
        sqlx::query("UPDATE registration_codes SET attempts=attempts+1 WHERE email=?").bind(email).execute(&mut **tx).await?;
        return Err(if row.get::<i64, _>("attempts") + 1 >= MAX_ATTEMPTS {
            code_error("code_locked", "验证码输错次数过多，请重新获取。")
        } else { code_error("code_incorrect", "验证码不正确，请检查邮件后重新输入。") });
    }
    sqlx::query("DELETE FROM registration_codes WHERE email=?").bind(email).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) fn email_content(code: &str) -> (String, String) {
    (
        format!("你的 JunoHub 注册验证码是：{code}\n\n验证码 5 分钟内有效，请勿向他人透露。\n如果你没有申请注册，请忽略此邮件。"),
        format!("<div style=\"font-family:Arial,sans-serif;max-width:480px;margin:0 auto;padding:32px;color:#18202e\"><h2>注册 JunoHub</h2><p>你的邮箱验证码：</p><p style=\"font-size:32px;font-weight:700;letter-spacing:8px;color:#2563eb\">{code}</p><p>验证码 5 分钟内有效，请勿向他人透露。</p><p style=\"color:#677284;font-size:13px\">如果你没有申请注册，请忽略此邮件。</p></div>"),
    )
}

fn email_payload(config: &EmailConfig, email: &str, code: &str) -> Value {
    let (text, html) = email_content(code);
    json!({
        "from": config.from,
        "to": [email],
        "subject": "JunoHub 注册验证码",
        "text": text,
        "html": html
    })
}

async fn deliver(config: &EmailConfig, email: &str, code: &str, generation: &str) -> AppResult<()> {
    if config.provider == EmailProvider::Smtp {
        return crate::smtp_email::deliver(&config.smtp, email, code, generation).await;
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build()
        .map_err(|_| AppError::ServiceUnavailable("验证码邮件服务暂时不可用，请稍后重试。".into()))?;
    transmit(&client, "https://api.resend.com/emails", config, email, code, generation).await
}

fn validate_delivery_config(config: &EmailConfig) -> AppResult<()> {
    match config.provider {
        EmailProvider::Smtp => crate::smtp_email::validate(&config.smtp),
        EmailProvider::Resend if config.api_key.is_empty() => Err(AppError::ServiceUnavailable("验证码邮件服务尚未配置，请联系管理员。".into())),
        // 发件地址必须由 RESEND_FROM 显式提供：这里曾经回退到一个写死的域名，
        // 结果是没有配置的部署会拿别人的域名发信，而且失败得很难懂。
        EmailProvider::Resend if config.from.trim().is_empty() => Err(AppError::ServiceUnavailable(
            "验证码邮件服务缺少 RESEND_FROM，请联系管理员配置发件地址。".into(),
        )),
        EmailProvider::Resend => Ok(()),
    }
}

async fn transmit(client: &reqwest::Client, endpoint: &str, config: &EmailConfig, email: &str, code: &str, generation: &str) -> AppResult<()> {
    let response = client.post(endpoint)
        .bearer_auth(&config.api_key).header("Idempotency-Key", format!("registration/{generation}"))
        .json(&email_payload(config, email, code)).send().await
        .map_err(|_| AppError::ServiceUnavailable("验证码邮件发送超时或连接失败，请稍后重试。".into()))?;
    let status = response.status().as_u16();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    delivery_result(status, &body)
}

fn delivery_result(status: u16, body: &Value) -> AppResult<()> {
    if (200..300).contains(&status) && body["id"].as_str().is_some_and(|id| !id.is_empty()) { return Ok(()); }
    // Never return or log the provider's raw response: it may include recipient details.
    tracing::warn!(status, error_name = body["name"].as_str().unwrap_or("unknown"), "registration email rejected by Resend");
    Err(match status {
        401 => AppError::ServiceUnavailable("邮件服务认证失败，请联系管理员检查 Resend API Key。".into()),
        403 => AppError::ServiceUnavailable("发信域名未验证或发信权限不足，请联系管理员检查 Resend 配置。".into()),
        422 => AppError::ServiceUnavailable("发件地址配置有误，请联系管理员检查 Resend 配置。".into()),
        429 => AppError::TooManyRequests("邮件服务发送额度或频率已达上限，请稍后重试。".into()),
        _ => AppError::ServiceUnavailable("验证码邮件发送失败，请稍后重试。".into()),
    })
}

#[cfg(test)]
pub(crate) async fn seed_test_code(pool: &SqlitePool, secret: &str, email: &str, code: &str, timestamp: i64) {
    reserve(pool, &EmailConfig::default(), secret, email, code, &Uuid::new_v4().to_string(), timestamp).await.unwrap();
    sqlx::query("UPDATE registration_codes SET sent=1 WHERE email=?").bind(email).execute(pool).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    async fn attempt(pool: &SqlitePool, email: &str, code: &str, timestamp: i64) -> AppResult<()> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let result = consume(&mut tx, "test-secret", email, code, timestamp).await;
        tx.commit().await.unwrap();
        result
    }

    #[tokio::test]
    async fn verification_is_bound_to_email_expires_and_is_single_use() {
        let pool = pool().await;
        seed_test_code(&pool, "test-secret", "one@example.test", "123456", 1000).await;
        assert!(matches!(attempt(&pool, "other@example.test", "123456", 1001).await, Err(AppError::FormFailed { reason: "code_missing", .. })));
        assert!(matches!(attempt(&pool, "one@example.test", "000000", 1001).await, Err(AppError::FormFailed { reason: "code_incorrect", .. })));
        assert!(attempt(&pool, "one@example.test", "123456", 1001).await.is_ok());
        assert!(matches!(attempt(&pool, "one@example.test", "123456", 1002).await, Err(AppError::FormFailed { reason: "code_missing", .. })));
        seed_test_code(&pool, "test-secret", "one@example.test", "123456", 1400).await;
        assert!(matches!(attempt(&pool, "one@example.test", "123456", 1700).await, Err(AppError::FormFailed { reason: "code_expired", .. })));
    }

    #[tokio::test]
    async fn failed_attempts_lock_the_code_and_resend_invalidates_it() {
        let pool = pool().await;
        seed_test_code(&pool, "test-secret", "one@example.test", "123456", 1000).await;
        for _ in 0..4 { assert!(matches!(attempt(&pool, "one@example.test", "000000", 1001).await, Err(AppError::FormFailed { reason: "code_incorrect", .. }))); }
        assert!(matches!(attempt(&pool, "one@example.test", "000000", 1001).await, Err(AppError::FormFailed { reason: "code_locked", .. })));
        assert!(matches!(attempt(&pool, "one@example.test", "123456", 1002).await, Err(AppError::FormFailed { reason: "code_locked", .. })));
        let config = EmailConfig::default();
        reserve(&pool, &config, "test-secret", "one@example.test", "654321", "second", 1060).await.unwrap();
        assert!(matches!(attempt(&pool, "one@example.test", "654321", 1061).await, Err(AppError::FormFailed { reason: "code_not_sent", .. })));
        sqlx::query("UPDATE registration_codes SET sent=1").execute(&pool).await.unwrap();
        assert!(matches!(attempt(&pool, "one@example.test", "123456", 1061).await, Err(AppError::FormFailed { reason: "code_incorrect", .. })));
        assert!(attempt(&pool, "one@example.test", "654321", 1062).await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_reservations_and_daily_limits_cannot_be_bypassed() {
        let pool = pool().await;
        let config = EmailConfig { daily_limit: 2, ..Default::default() };
        let (first, second) = tokio::join!(
            reserve(&pool, &config, "secret", "one@example.test", "123456", "first", 1000),
            reserve(&pool, &config, "secret", "one@example.test", "654321", "second", 1000)
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        reserve(&pool, &config, "secret", "one@example.test", "123456", "third", 1060).await.unwrap();
        assert!(matches!(reserve(&pool, &config, "secret", "two@example.test", "123456", "fourth", 1120).await, Err(AppError::TooManyRequests(_))));
        let hash: String = sqlx::query_scalar("SELECT code_hash FROM registration_codes").fetch_one(&pool).await.unwrap();
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, "123456");
    }

    #[test]
    fn resend_requires_an_accepted_message_and_errors_are_actionable() {
        assert!(delivery_result(200, &json!({"id":"message-id"})).is_ok());
        for status in [200, 401, 403, 422, 429, 500] {
            let error = delivery_result(status, &json!({"message":"private provider response"})).unwrap_err();
            assert!(!error.to_string().contains("private provider response"));
        }
    }

    #[tokio::test]
    async fn resend_request_uses_correct_auth_domain_payload_and_idempotency() {
        use axum::{http::HeaderMap, routing::post, Router};
        let app = Router::new().route("/emails", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-key");
            assert_eq!(headers["idempotency-key"], "registration/fixture-generation");
            assert_eq!(body["from"], "JunoHub <fixture@example.test>");
            assert_eq!(body["to"], json!(["member@example.test"]));
            assert!(body["text"].as_str().unwrap().contains("123456"));
            assert!(body["html"].as_str().unwrap().contains("123456"));
            Json(json!({"id":"fixture-message"}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = EmailConfig { api_key: "fixture-key".into(), from: "JunoHub <fixture@example.test>".into(), ..Default::default() };
        let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).redirect(reqwest::redirect::Policy::none()).build().unwrap();
        transmit(&client, &format!("http://{address}/emails"), &config, "member@example.test", "123456", "fixture-generation").await.unwrap();
        server.abort();
    }

    #[test]
    fn resend_without_a_sender_address_is_reported_not_silently_defaulted() {
        let config = EmailConfig { api_key: "fixture-key".into(), ..Default::default() };
        assert!(config.from.trim().is_empty());
        let error = validate_delivery_config(&config).unwrap_err();
        assert!(error.to_string().contains("RESEND_FROM"), "{error}");
        // 空白字符串同样算未配置，不能被当成有效发件地址。
        let blank = EmailConfig { api_key: "fixture-key".into(), from: "   ".into(), ..Default::default() };
        assert!(validate_delivery_config(&blank).is_err());
        // 配好之后必须放行。
        let configured = EmailConfig { api_key: "fixture-key".into(), from: "JunoHub <noreply@example.test>".into(), ..Default::default() };
        assert!(validate_delivery_config(&configured).is_ok());
    }
}
