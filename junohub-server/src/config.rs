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
    pub email: EmailConfig,
    pub oss: crate::image_storage::OssConfig,
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
    /// 允许跨域访问本服务的浏览器来源（逗号分隔的 scheme://host[:port]）。
    /// 留空表示不放行任何跨域请求 —— 控制台经 Next.js 的 rewrite 同源代理
    /// 访问本服务，本来就不需要 CORS。
    pub allowed_origins: Vec<String>,
    /// 日志类数据的保留天数；0 表示永久保留，不清理。
    pub log_retention_days: i64,
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
        let jwt_secret = required_secret("JUNOHUB_JWT_SECRET", env::var("JUNOHUB_JWT_SECRET").ok())?;
        let encryption_key = required_secret("JUNOHUB_ENCRYPTION_KEY", env::var("JUNOHUB_ENCRYPTION_KEY").ok())?;
        Ok(Self {
            bind,
            database_url: env::var("JUNOHUB_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://junohub.db?mode=rwc".into()),
            jwt_secret,
            bootstrap_admin_email: env::var("JUNOHUB_BOOTSTRAP_ADMIN_EMAIL").unwrap_or_default(),
            bootstrap_admin_password: env::var("JUNOHUB_BOOTSTRAP_ADMIN_PASSWORD")
                .unwrap_or_default(),
            encryption_key,
            email: EmailConfig::from_env()?,
            oss: crate::image_storage::OssConfig::from_env(),
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
            // 槽位由心跳（每分钟）续期，TTL 只需要比心跳大几倍。
            // 之前默认 30 分钟，意味着进程被强杀后并发槽最多滞留半小时；
            // 3 分钟让异常退出的影响窗口降到分钟级。
            sub2api_slot_ttl_seconds: env::var("GATEWAY_CONCURRENCY_SLOT_TTL_MINUTES")
                .unwrap_or_else(|_| "3".into())
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
            allowed_origins: parse_origins(
                &env::var("JUNOHUB_ALLOWED_ORIGINS").unwrap_or_default(),
            )?,
            log_retention_days: env::var("JUNOHUB_LOG_RETENTION_DAYS")
                .unwrap_or_else(|_| "90".into())
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|days| *days >= 0)
                .context("JUNOHUB_LOG_RETENTION_DAYS must be a non-negative integer (0 keeps everything)")?,
        })
    }
}

#[derive(Clone)]
pub struct EmailConfig {
    pub provider: EmailProvider,
    pub smtp: SmtpConfig,
    pub api_key: String,
    pub from: String,
    pub daily_limit: i64,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self { provider: EmailProvider::Resend, smtp: SmtpConfig::default(), api_key: String::new(), from: String::new(), daily_limit: 100 }
    }
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig").field("provider", &self.provider).field("smtp", &self.smtp).field("api_key", &"[redacted]").field("from", &self.from).field("daily_limit", &self.daily_limit).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmailProvider { Resend, Smtp }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmtpSecurity {
    #[default]
    Tls,
    StartTls,
}

#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: String,
    pub from: String,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self { host: String::new(), port: 465, security: SmtpSecurity::Tls, username: String::new(), password: String::new(), from: String::new() }
    }
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig").field("host", &self.host).field("port", &self.port)
            .field("security", &self.security).field("username", &"[redacted]")
            .field("password", &"[redacted]").field("from", &self.from).finish()
    }
}

impl EmailConfig {
    fn from_env() -> Result<Self> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let provider = match get("JUNOHUB_EMAIL_PROVIDER").unwrap_or_else(|| "resend".into()).trim().to_ascii_lowercase().as_str() {
            "resend" => EmailProvider::Resend,
            "smtp" => EmailProvider::Smtp,
            _ => anyhow::bail!("JUNOHUB_EMAIL_PROVIDER must be resend or smtp"),
        };
        let daily_limit = get("JUNOHUB_EMAIL_DAILY_LIMIT").unwrap_or_else(|| "100".into()).parse::<i64>()
            .ok().filter(|n| *n > 0).context("JUNOHUB_EMAIL_DAILY_LIMIT must be a positive integer")?;
        let mut config = Self {
            provider,
            api_key: get("RESEND_API_KEY").unwrap_or_default().trim().to_owned(),
            from: get("RESEND_FROM").unwrap_or_default().trim().to_owned(),
            daily_limit,
            ..Default::default()
        };
        if provider == EmailProvider::Smtp {
            let security = match get("SMTP_SECURITY").unwrap_or_else(|| "tls".into()).trim().to_ascii_lowercase().as_str() {
                "tls" => SmtpSecurity::Tls,
                "starttls" => SmtpSecurity::StartTls,
                _ => anyhow::bail!("SMTP_SECURITY must be tls or starttls"),
            };
            let default_port = if security == SmtpSecurity::Tls { "465" } else { "587" };
            let port = get("SMTP_PORT").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| default_port.into())
                .trim().parse::<u16>().ok().filter(|n| *n > 0).context("SMTP_PORT must be between 1 and 65535")?;
            config.smtp = SmtpConfig {
                host: get("SMTP_HOST").unwrap_or_default().trim().into(),
                port,
                security,
                username: get("SMTP_USERNAME").unwrap_or_default().trim().into(),
                password: get("SMTP_PASSWORD").unwrap_or_default(),
                from: get("SMTP_FROM").unwrap_or_default().trim().into(),
            };
        }
        Ok(config)
    }
}

/// 解析 JUNOHUB_ALLOWED_ORIGINS。
///
/// 只接受 scheme://host[:port]，路径、查询串和通配符一律拒绝：`*` 或者
/// `https://*` 会让这项配置看起来收紧了、实际上没有，不如在启动时直接报错。
fn parse_origins(raw: &str) -> Result<Vec<String>> {
    let mut origins = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let parsed = url::Url::parse(entry)
            .with_context(|| format!("invalid JUNOHUB_ALLOWED_ORIGINS entry: {entry}"))?;
        anyhow::ensure!(
            matches!(parsed.scheme(), "http" | "https"),
            "JUNOHUB_ALLOWED_ORIGINS entry must be http or https: {entry}"
        );
        anyhow::ensure!(
            parsed.host_str().is_some()
                && parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "JUNOHUB_ALLOWED_ORIGINS entry must be a bare origin like https://hub.example.com: {entry}"
        );
        // url 会把 `https://*` 解析成主机名恰好是 `*`，放过去等于没配。
        anyhow::ensure!(
            !parsed.host_str().unwrap_or_default().contains('*'),
            "JUNOHUB_ALLOWED_ORIGINS 不接受通配符主机: {entry}"
        );
        let origin = parsed.origin().ascii_serialization();
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    Ok(origins)
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
    use super::*;

    fn email_config(entries: &[(&str, &str)]) -> Result<EmailConfig> {
        EmailConfig::from_lookup(|name| entries.iter().rev().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()))
    }

    #[test]
    fn smtp_config_supports_both_secure_modes_and_redacts_credentials() {
        let tls = email_config(&[("JUNOHUB_EMAIL_PROVIDER", "smtp"), ("SMTP_PASSWORD", "private-secret")]).unwrap();
        assert_eq!(tls.provider, EmailProvider::Smtp);
        assert_eq!(tls.smtp.security, SmtpSecurity::Tls);
        assert_eq!(tls.smtp.port, 465);
        assert!(!format!("{tls:?}").contains("private-secret"));
        let starttls = email_config(&[("JUNOHUB_EMAIL_PROVIDER", "smtp"), ("SMTP_SECURITY", "starttls")]).unwrap();
        assert_eq!(starttls.smtp.port, 587);
        assert_eq!(starttls.smtp.security, SmtpSecurity::StartTls);
        let custom = email_config(&[("JUNOHUB_EMAIL_PROVIDER", "smtp"), ("SMTP_PORT", "2525")]).unwrap();
        assert_eq!(custom.smtp.port, 2525);
        for (key, value) in [("SMTP_SECURITY", "none"), ("SMTP_PORT", "0"), ("SMTP_PORT", "65536"), ("JUNOHUB_EMAIL_PROVIDER", "unknown"), ("JUNOHUB_EMAIL_DAILY_LIMIT", "0")] {
            assert!(email_config(&[("JUNOHUB_EMAIL_PROVIDER", "smtp"), (key, value)]).is_err());
        }
        assert_eq!(email_config(&[]).unwrap().provider, EmailProvider::Resend);
    }

    #[test]
    fn allowed_origins_are_normalized_and_lookalikes_are_rejected() {
        assert_eq!(parse_origins("").unwrap(), Vec::<String>::new());
        assert_eq!(parse_origins("  , ").unwrap(), Vec::<String>::new());
        assert_eq!(
            parse_origins("https://hub.example.com, http://127.0.0.1:3003/").unwrap(),
            vec![
                "https://hub.example.com".to_string(),
                "http://127.0.0.1:3003".to_string()
            ]
        );
        // 重复项去掉，避免把同一来源列两遍。
        assert_eq!(
            parse_origins("https://a.test,https://a.test").unwrap().len(),
            1
        );
        for invalid in [
            "*",
            "https://*",
            "hub.example.com",
            "ftp://hub.example.com",
            "https://hub.example.com/api",
            "https://hub.example.com?x=1",
            "https://user:pass@hub.example.com",
        ] {
            assert!(
                parse_origins(invalid).is_err(),
                "{invalid} 应当被拒绝"
            );
        }
    }

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

fn required_secret(name: &str, value: Option<String>) -> Result<String> {
    let value = value.filter(|value| value.len() >= 32 && value.trim() == value
        && !value.starts_with("replace-") && !value.starts_with("development-only-")
        && value.bytes().collect::<std::collections::HashSet<_>>().len() >= 8)
        .with_context(|| format!("{name} must be configured with at least 32 random characters"))?;
    Ok(value)
}

#[cfg(test)]
mod secret_validation_tests {
    use super::*;
    #[test]
    fn startup_rejects_missing_short_or_example_secrets() {
        for value in [None, Some("short".into()), Some("replace-with-at-least-32-random-characters".into()), Some("a".repeat(64))] {
            assert!(required_secret("TEST_SECRET",value).is_err());
        }
        assert!(required_secret("TEST_SECRET",Some("8adc4b695ac6521d37f7b6caf511ed20c6eab29c71a989fb5ec5800a4a71ca363".into())).is_ok());
    }
}
