mod access_guide;
mod admin;
mod auth;
mod auth_admission;
mod client_config_validation;
mod client_pair;
mod client_protocol;
mod config;
mod console;
mod direct_provider;
mod email_verification;
mod error;
mod gateway_billing;
mod gateway_limits;
mod gateway_resources;
mod gateway_tasks;
mod gateway_upload;
mod image_storage;
mod juno_keys;
mod model_api;
mod model_request_path;
mod models;
mod monitor;
mod oauth;
mod oauth_completion;
mod observability;
mod proxy;
mod secrets;
mod smtp_email;
mod state;
mod store;
mod sub2_chat_bridge;
mod sub2_dispatch;
mod sub2_domain;
mod sub2_gateway;
mod sub2_policy;
mod sub2_protocol;
mod sub2_repository;
mod sub2_scheduler;
mod sub2_sse;
mod sub2_sticky;
mod sub2_worker;
mod sub2api;
mod tokenhub_images;
mod upstream_probe;

use anyhow::Result;
use axum::{
    http::HeaderValue,
    middleware,
    routing::{any, get, post},
    Json, Router,
};
use config::Config;
use models::Envelope;
use reqwest::Client;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, sqlite::SqlitePoolOptions};
use state::AppState;
use std::{sync::Arc, time::Duration};
use tower_http::cors::{Any, CorsLayer};
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
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    let sub2_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.sub2api_database_url)
        .await?;
    let redis_client = redis::Client::open(config.sub2api_redis_url.as_str())?;
    let mut sub2_redis = redis::aio::ConnectionManager::new(redis_client).await?;
    let pong: String = redis::cmd("PING").query_async(&mut sub2_redis).await?;
    if pong != "PONG" {
        anyhow::bail!("Sub2API Redis health check failed");
    }
    let state = AppState {
        pool: pool.clone(),
        sub2_pool,
        sub2_redis: Some(sub2_redis),
        oauth_sessions: oauth::OAuthSessionStore::persistent(
            pool.clone(),
            config.encryption_key.clone(),
        ),
        config: Arc::new(config.clone()),
        client: Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Custom API-key headers must never follow a redirect to another origin.
                if attempt.previous().len() >= 10 {
                    attempt.error("too many upstream redirects")
                } else if attempt
                    .previous()
                    .last()
                    .is_some_and(|previous| previous.origin() != attempt.url().origin())
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?,
    };
    let accounts = sub2_repository::Sub2Repository::new(state.sub2_pool.clone());
    let protected_accounts = accounts
        .protect_plaintext_credentials(&state.config.encryption_key)
        .await?;
    if protected_accounts > 0 {
        info!(
            accounts = protected_accounts,
            "encrypted legacy account credentials"
        );
    }
    // 加密清单是正向匹配：上游新增凭据字段名时不会自动受保护。这里把
    // “看着像密钥却仍是明文”的字段变成一条告警，而不是等它默默落库。
    match accounts.audit_plaintext_credentials().await {
        Ok(unprotected) if !unprotected.is_empty() => tracing::warn!(
            accounts = unprotected.len(),
            fields = ?unprotected,
            "账号凭据中存在未被加密的疑似密钥字段，请核对 secrets.rs 的加密清单"
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "无法检查账号凭据的加密状态"),
    }
    console::ensure_bootstrap_admin(&state).await?;
    let image_state = state.clone();
    tokio::spawn(async move {
        match image_storage::migrate_legacy_avatars(&image_state).await {
            Ok(count) if count > 0 => info!(count, "migrated existing avatars to OSS"),
            Err(error) => tracing::warn!(%error, "could not migrate existing avatars to OSS"),
            _ => {}
        }
    });
    migrate_sub2api_provider_base_urls(&state).await;
    match client_pair::repair_legacy_junohub_pi_models(&state).await {
        Ok(count) if count > 0 => info!(count, "repaired reversed JunoHub Pi model fields"),
        Err(error) => tracing::warn!(%error, "could not repair legacy JunoHub Pi model fields"),
        _ => {}
    }
    let reconciled_pools = console::reconcile_managed_pools(&state).await?;
    if reconciled_pools > 0 {
        info!(
            pools = reconciled_pools,
            "linked legacy JunoHub providers to account pools"
        );
    }
    observability::initialize();
    monitor::heartbeat(
        &state,
        "account_scheduler",
        state.config.scheduler_worker_enabled,
        None,
    )
    .await;
    monitor::heartbeat(&state, "task_poller", true, None).await;
    monitor::heartbeat(&state, "monitor", true, None).await;
    let _monitor_worker = monitor::spawn(state.clone());
    let _retention_worker = monitor::spawn_retention(state.clone());
    let _scheduler_worker = sub2_worker::spawn(state.clone());
    let _task_worker = gateway_tasks::spawn(state.clone());
    let admin_routes = Router::new()
        .route("/{*path}", post(admin::dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            observability::audit,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ));
    let account_routes = Router::new()
        .route("/{*path}", post(console::account_dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_admission::password_guard,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_account,
        ));
    let console_routes = Router::new()
        .route("/{*path}", post(console::dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            observability::audit,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_user,
        ));
    let metrics_routes = Router::new()
        .route("/metrics", get(monitor::metrics))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            monitor::metrics_auth,
        ));
    let auth_routes = Router::new()
        .route("/auth/register", post(console::register))
        .route("/auth/register/code", post(email_verification::send_code))
        .route("/auth/login", post(console::login))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_admission::guard,
        ));
    let mut app = Router::new()
        .merge(metrics_routes)
        .route("/ping", get(health))
        .route("/health", get(health))
        .route("/health/liveness", get(health))
        .route("/health/readiness", get(monitor::readiness))
        .nest("/hub_admin", admin_routes)
        .merge(auth_routes)
        .route(
            "/media/avatars/{asset_id}",
            get(image_storage::serve_avatar),
        )
        .route(
            "/integrations/juno/keys/ensure",
            post(juno_keys::ensure_juno_key),
        )
        .route("/integrations/juno/models", get(juno_keys::juno_models))
        .route("/integrations/juno/usage", post(juno_keys::juno_usage))
        .route("/client-connect/redeem", post(client_pair::redeem))
        .nest("/account-api", account_routes)
        .nest("/console", console_routes)
        .route("/models", get(proxy::models))
        .route("/v1/models", get(proxy::models))
        .route("/v1/models/{*model}", get(proxy::model))
        .route("/v1/tasks/{task_id}", get(gateway_tasks::query))
        .route("/v1/messages", post(client_protocol::anthropic_messages))
        .route(
            "/v1beta/models/{*path}",
            any(client_protocol::gemini_generate),
        )
        .route("/v1/{*path}", any(proxy::passthrough))
        .route("/chat/completions", post(proxy::passthrough))
        .route("/responses", any(proxy::passthrough))
        .route("/responses/{*path}", any(proxy::passthrough))
        .route("/backend-api/codex/responses", any(proxy::passthrough))
        .route(
            "/backend-api/codex/responses/{*path}",
            any(proxy::passthrough),
        )
        .fallback(proxy::not_found)
        .with_state(state);
    // 默认不挂 CORS 层：控制台经 Next.js rewrite 同源访问，浏览器不跨域直连本服务。
    // 只有显式配置了 JUNOHUB_ALLOWED_ORIGINS 才放行对应来源。
    if let Some(cors) = cors_layer(&config.allowed_origins) {
        info!(origins = ?config.allowed_origins, "已放行跨域访问来源");
        app = app.layer(cors);
    }
    let app = app.layer(middleware::from_fn(observability::trace));
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    info!(address=%config.bind,"Juno Hub Rust server started");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

async fn migrate_sub2api_provider_base_urls(state: &AppState) {
    // 一次性迁移：把指向已删除 Go 网关的存量 base_url 改写为 Rust 引擎自身地址。
    // 旧地址用 SQL 拼接表达，避免在代码中出现活的 Go 网关引用。
    let updated = sqlx::query(
        "UPDATE user_providers SET base_url='http://junohub-server:9003/v1',updated_at=? WHERE provider_type='sub2api' AND base_url LIKE 'http://'||'sub2api'||':8080%'",
    )
    .bind(crate::store::now())
    .execute(&state.pool)
    .await;
    match updated {
        Ok(result) if result.rows_affected() > 0 => {
            info!(
                providers = result.rows_affected(),
                "migrated sub2api provider base_url to Rust account engine"
            );
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(error = %error, "failed to migrate sub2api provider base_url"),
    }
}

/// 浏览器跨域策略。
///
/// 来源是这里唯一的信任边界，所以 `allow_headers` 保持 Any —— 网关要透传
/// `x-api-key`、`anthropic-version` 等任意上游请求头，逐个枚举既挡不住谁、
/// 又容易漏配。返回 None 表示完全不挂 CORS 层。
fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    let allowed = origins
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect::<Vec<_>>();
    if allowed.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(allowed)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any),
    )
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
