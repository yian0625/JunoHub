use std::sync::Arc;

use redis::aio::ConnectionManager;
use reqwest::Client;
use sqlx::{PgPool, SqlitePool};

use crate::{config::Config, oauth::OAuthSessionStore};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub sub2_pool: PgPool,
    pub sub2_redis: Option<ConnectionManager>,
    pub oauth_sessions: OAuthSessionStore,
    pub config: Arc<Config>,
    pub client: Client,
}
