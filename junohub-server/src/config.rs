use std::{env, net::SocketAddr};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub jwt_secret: String,
    pub bootstrap_admin_email: String,
    pub bootstrap_admin_password: String,
    pub encryption_key: String,
    pub sub2api_admin_email: String,
    pub sub2api_database_url: String,
    pub sub2api_redis_url: String,
    pub sub2api_slot_ttl_seconds: i64,
    pub sub2api_wait_ttl_seconds: i64,
    pub sub2api_sticky_ttl_seconds: i64,
    pub sub2api_response_sticky_ttl_seconds: i64,
    pub sub2api_session_hash_read_old_fallback: bool,
    pub sub2api_session_hash_dual_write_old: bool,
    pub scheduler_worker_enabled: bool,
    pub account_engine_gateway_enabled: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind = env::var("JUNOHUB_BIND")
            .unwrap_or_else(|_| "0.0.0.0:9003".into())
            .parse()
            .context("invalid JUNOHUB_BIND")?;
        let sticky_wait_seconds = parse_duration_seconds(
            &env::var("GATEWAY_SCHEDULING_STICKY_SESSION_WAIT_TIMEOUT")
                .unwrap_or_else(|_| "120s".into()),
        )
        .context("invalid GATEWAY_SCHEDULING_STICKY_SESSION_WAIT_TIMEOUT")?;
        let fallback_wait_seconds = parse_duration_seconds(
            &env::var("GATEWAY_SCHEDULING_FALLBACK_WAIT_TIMEOUT").unwrap_or_else(|_| "30s".into()),
        )
        .context("invalid GATEWAY_SCHEDULING_FALLBACK_WAIT_TIMEOUT")?;
        Ok(Self {
            bind,
            database_url: env::var("JUNOHUB_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://junohub.db?mode=rwc".into()),
            jwt_secret: env::var("JUNOHUB_JWT_SECRET")
                .unwrap_or_else(|_| "development-only-change-me".into()),
            bootstrap_admin_email: env::var("JUNOHUB_BOOTSTRAP_ADMIN_EMAIL").unwrap_or_default(),
            bootstrap_admin_password: env::var("JUNOHUB_BOOTSTRAP_ADMIN_PASSWORD")
                .unwrap_or_default(),
            encryption_key: env::var("JUNOHUB_ENCRYPTION_KEY")
                .unwrap_or_else(|_| "development-only-encryption-key".into()),
            sub2api_admin_email: env::var("ADMIN_EMAIL").unwrap_or_default(),
            sub2api_database_url: env::var("SUB2API_DATABASE_URL").unwrap_or_else(|_| {
                let user = env::var("POSTGRES_USER").unwrap_or_else(|_| "sub2api".into());
                let password = env::var("POSTGRES_PASSWORD").unwrap_or_default();
                let database = env::var("POSTGRES_DB").unwrap_or_else(|_| "sub2api".into());
                format!("postgres://{user}:{password}@junohub-accounts-postgres:5432/{database}")
            }),
            sub2api_redis_url: env::var("SUB2API_REDIS_URL").unwrap_or_else(|_| {
                let password = env::var("REDIS_PASSWORD").unwrap_or_default();
                let database = env::var("REDIS_DB").unwrap_or_else(|_| "0".into());
                if password.is_empty() {
                    format!("redis://junohub-accounts-redis:6379/{database}")
                } else {
                    format!("redis://:{password}@junohub-accounts-redis:6379/{database}")
                }
            }),
            sub2api_slot_ttl_seconds: env::var("GATEWAY_CONCURRENCY_SLOT_TTL_MINUTES")
                .unwrap_or_else(|_| "30".into())
                .parse::<i64>()
                .context("invalid GATEWAY_CONCURRENCY_SLOT_TTL_MINUTES")?
                .checked_mul(60)
                .filter(|value| *value > 0)
                .context("GATEWAY_CONCURRENCY_SLOT_TTL_MINUTES must be positive")?,
            sub2api_wait_ttl_seconds: sticky_wait_seconds.max(fallback_wait_seconds).max(1),
            sub2api_sticky_ttl_seconds: positive_env_i64(
                "GATEWAY_OPENAI_WS_STICKY_SESSION_TTL_SECONDS",
                3600,
            )?,
            sub2api_response_sticky_ttl_seconds: positive_env_i64(
                "GATEWAY_OPENAI_WS_STICKY_RESPONSE_ID_TTL_SECONDS",
                3600,
            )?,
            sub2api_session_hash_read_old_fallback: bool_env(
                "GATEWAY_OPENAI_WS_SESSION_HASH_READ_OLD_FALLBACK",
                true,
            )?,
            sub2api_session_hash_dual_write_old: bool_env(
                "GATEWAY_OPENAI_WS_SESSION_HASH_DUAL_WRITE_OLD",
                true,
            )?,
            scheduler_worker_enabled: bool_env("JUNOHUB_SCHEDULER_WORKER_ENABLED", false)?,
            account_engine_gateway_enabled: bool_env(
                "JUNOHUB_ACCOUNT_ENGINE_GATEWAY_ENABLED",
                false,
            )?,
        })
    }
}

fn positive_env_i64(name: &str, default: i64) -> Result<i64> {
    env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<i64>()
        .with_context(|| format!("invalid {name}"))
        .and_then(|value| {
            (value > 0)
                .then_some(value)
                .context(format!("{name} must be positive"))
        })
}

fn bool_env(name: &str, default: bool) -> Result<bool> {
    match env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("invalid {name}"),
    }
}

fn parse_duration_seconds(value: &str) -> Result<i64> {
    let value = value.trim();
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        let millis = number.parse::<i64>().context("invalid milliseconds")?;
        return (millis >= 0)
            .then_some((millis + 999) / 1000)
            .context("duration must not be negative");
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600)
    } else {
        (value, 1)
    };
    number
        .parse::<i64>()
        .context("invalid duration number")?
        .checked_mul(multiplier)
        .filter(|seconds| *seconds >= 0)
        .context("duration is out of range")
}

#[cfg(test)]
mod tests {
    use super::parse_duration_seconds;

    #[test]
    fn parses_sub2api_duration_environment_values() {
        assert_eq!(parse_duration_seconds("120s").unwrap(), 120);
        assert_eq!(parse_duration_seconds("2m").unwrap(), 120);
        assert_eq!(parse_duration_seconds("1h").unwrap(), 3600);
        assert_eq!(parse_duration_seconds("1501ms").unwrap(), 2);
        assert!(parse_duration_seconds("-1ms").is_err());
        assert!(parse_duration_seconds("never").is_err());
    }
}
