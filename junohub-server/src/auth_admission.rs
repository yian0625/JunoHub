//! Public authentication must be bounded before doing expensive password work.
use crate::{
    error::{AppError, AppResult},
    gateway_limits::{self, Limits},
    state::AppState,
};
use axum::{
    body::{to_bytes, Body},
    extract::{ConnectInfo, Path, Request, State},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;

pub async fn guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    // Only the socket peer is trusted. Client-supplied X-Forwarded-For cannot
    // mint fresh quotas; deployments behind a proxy still share this peer cap.
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 16 * 1024)
        .await
        .map_err(|_| AppError::PayloadTooLarge("登录或注册请求不能超过 16KiB".into()))?;
    let email = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value["email"]
                .as_str()
                .map(|value| value.trim().to_ascii_lowercase())
        })
        .filter(|email| email.len() <= 320)
        .unwrap_or_default();
    let mut scopes = vec![
        (
            "auth:global".into(),
            Limits {
                max_concurrency: 4,
                rpm: 300,
            },
        ),
        (
            format!("auth:peer:{}", hex::encode(Sha256::digest(peer.as_bytes()))),
            Limits {
                max_concurrency: 0,
                rpm: 180,
            },
        ),
    ];
    if !email.is_empty() {
        scopes.push((
            format!(
                "auth:email:{}",
                hex::encode(Sha256::digest(email.as_bytes()))
            ),
            Limits {
                max_concurrency: 0,
                rpm: 10,
            },
        ));
    }
    let _lease = gateway_limits::acquire(&state, &scopes)
        .await
        .map_err(|error| match error {
            AppError::TooManyRequests(_) => {
                crate::observability::security_rejection(
                    crate::observability::SecurityReason::AuthRateLimit,
                );
                AppError::TooManyRequests("登录或注册尝试过于频繁，请稍后重试".into())
            }
            _ => {
                crate::observability::security_rejection(
                    crate::observability::SecurityReason::AuthDependency,
                );
                AppError::ServiceUnavailable("登录保护服务暂时不可用，请稍后重试".into())
            }
        })?;
    Ok(next.run(Request::from_parts(parts, Body::from(body))).await)
}

pub async fn password_guard(
    state: State<AppState>,
    Path(path): Path<String>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    if path.trim_matches('/') == "password/update" {
        guard(state, request, next).await
    } else {
        Ok(next.run(request).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{middleware, routing::post, Router};
    async fn serve(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/login", post(|| async { "accepted" }))
            .route_layer(middleware::from_fn_with_state(state, guard));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        (format!("http://{address}/login"), server)
    }
    #[tokio::test]
    async fn auth_is_unavailable_without_admission_and_rejects_large_bodies() {
        let (url, server) = serve(crate::console::tests::state().await).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            client.post(&url).body("{}").send().await.unwrap().status(),
            503
        );
        assert_eq!(
            client
                .post(&url)
                .body("x".repeat(16385))
                .send()
                .await
                .unwrap()
                .status(),
            413
        );
        server.abort();
    }
    #[tokio::test]
    async fn password_protection_uses_the_same_decoded_path_as_the_handler() {
        let state = crate::console::tests::state().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/{*path}", post(|| async { "ok" }))
            .route_layer(middleware::from_fn_with_state(state, password_guard));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for path in [
            "/password/update",
            "/password/update/",
            "/%70assword/update",
            "/password%2Fupdate",
        ] {
            assert_eq!(
                client
                    .post(format!("{base}{path}"))
                    .body("{}")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                503,
                "{path}"
            );
        }
        assert_eq!(
            client
                .post(format!("{base}/profile/update"))
                .body("x".repeat(32768))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn global_auth_concurrency_bounds_expensive_handlers_and_releases_slots() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let mut state = crate::console::tests::state().await;
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        let entered = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let count = entered.clone();
        let wait = gate.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/login", listener.local_addr().unwrap());
        let router = Router::new()
            .route(
                "/login",
                post(move || {
                    let count = count.clone();
                    let wait = wait.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        wait.acquire().await.unwrap().forget();
                        "accepted"
                    }
                }),
            )
            .route_layer(middleware::from_fn_with_state(state, guard));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut requests = vec![];
        for _ in 0..4 {
            let client = client.clone();
            let url = url.clone();
            requests.push(tokio::spawn(async move {
                client.post(url).body("{}").send().await.unwrap()
            }));
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while entered.load(Ordering::SeqCst) < 4 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            client.post(&url).body("{}").send().await.unwrap().status(),
            429
        );
        assert_eq!(entered.load(Ordering::SeqCst), 4);
        gate.add_permits(4);
        for request in requests {
            assert_eq!(request.await.unwrap().status(), 200);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.add_permits(1);
        assert_eq!(
            client.post(&url).body("{}").send().await.unwrap().status(),
            200
        );
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires isolated JUNOHUB_TEST_REDIS_URL"]
    async fn auth_email_quota_cannot_be_reset_by_case_or_spoofed_forwarded_headers() {
        let mut state = crate::console::tests::state().await;
        state.sub2_redis = Some(
            redis::Client::open(std::env::var("JUNOHUB_TEST_REDIS_URL").unwrap())
                .unwrap()
                .get_connection_manager()
                .await
                .unwrap(),
        );
        let (url, server) = serve(state).await;
        let email = format!("test-{}@example.test", uuid::Uuid::new_v4());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for i in 0..12 {
            let response=client.post(&url).header("x-forwarded-for",format!("198.51.100.{i}"))
                .json(&serde_json::json!({"email":if i%2==0 {email.clone()} else {email.to_uppercase()}})).send().await.unwrap();
            assert_eq!(response.status().as_u16(), if i < 10 { 200 } else { 429 });
        }
        assert_eq!(
            client
                .post(&url)
                .json(&serde_json::json!({"email":format!("other-{email}")}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        server.abort();
    }
}
