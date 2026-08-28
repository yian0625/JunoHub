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
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind = env::var("JUNOHUB_BIND")
            .unwrap_or_else(|_| "0.0.0.0:9003".into())
            .parse()
            .context("invalid JUNOHUB_BIND")?;
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
        })
    }
}
