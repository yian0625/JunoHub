mod admin;
mod auth;
mod config;
mod console;
mod error;
mod models;
mod proxy;
mod secrets;
mod state;
mod store;

use anyhow::Result;
use axum::{
    middleware,
    routing::{any, get, post},
    Json, Router,
};
use config::Config;
use models::Envelope;
use reqwest::Client;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use state::AppState;
use std::{sync::Arc, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "junohub_server=info,tower_http=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    if config.jwt_secret == "development-only-change-me" {
        tracing::warn!("正在使用开发 JWT 密钥；生产环境请设置 JUNOHUB_JWT_SECRET");
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    let state = AppState {
        pool,
        config: Arc::new(config.clone()),
        client: Client::builder()
            .timeout(Duration::from_secs(180))
            .build()?,
    };
    console::ensure_bootstrap_admin(&state).await?;
    let admin_routes = Router::new()
        .route("/{*path}", post(admin::dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ));
    let console_routes = Router::new()
        .route("/{*path}", post(console::dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_user,
        ));
    let app = Router::new()
        .route("/ping", get(health))
        .route("/health", get(health))
        .route("/health/liveness", get(health))
        .route("/health/readiness", get(health))
        .nest("/hub_admin", admin_routes)
        .route("/auth/register", post(console::register))
        .route("/auth/login", post(console::login))
        .nest("/console", console_routes)
        .route("/models", get(proxy::models))
        .route("/v1/models", get(proxy::models))
        .route("/v1/{*path}", any(proxy::passthrough))
        .route("/chat/completions", post(proxy::passthrough))
        .fallback(proxy::not_found)
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .expose_headers(Any),
        )
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    info!(address=%config.bind,"Juno Hub Rust server started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn health() -> Json<Envelope<serde_json::Value>> {
    Json(Envelope::ok(
        json!({"status":"ok","service":"junohub-server","language":"rust"}),
    ))
}
async fn shutdown() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler")
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {_=ctrl_c=>{},_=terminate=>{}}
}
