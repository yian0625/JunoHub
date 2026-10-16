//! Bounded labels, request correlation and complete-body HTTP metrics. No request bodies/credentials.
use crate::{auth::CurrentUser, state::AppState};
use axum::{
    body::{Body, HttpBody},
    extract::{Request, State},
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use futures_util::StreamExt;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry, TextEncoder,
};
use std::sync::LazyLock;
use std::time::Instant;
use tracing::Instrument;
const ROUTES: [&str; 19] = [
    "chat_completions",
    "responses",
    "messages",
    "images_generations",
    "images_edits",
    "videos_generations",
    "audio_generations",
    "audio_speech",
    "audio_transcriptions",
    "embeddings",
    "search",
    "rerank",
    "tasks",
    "models",
    "gemini",
    "control",
    "other",
    "authentication",
    "audio_translations",
];
const OUTCOMES: [&str; 6] = ["2xx", "3xx", "4xx", "5xx", "other", "cancelled"];
struct Metrics {
    registry: Registry,
    started: IntCounterVec,
    active: IntGaugeVec,
    completed: IntCounterVec,
    duration: HistogramVec,
    log_failures: IntCounter,
    security_rejections: IntCounterVec,
    uptime: IntGauge,
}
static METRICS: LazyLock<Metrics> = LazyLock::new(|| {
    let registry = Registry::new();
    #[cfg(target_os = "linux")]
    registry
        .register(Box::new(
            prometheus::process_collector::ProcessCollector::for_self(),
        ))
        .unwrap();
    let started = IntCounterVec::new(
        Opts::new(
            "junohub_http_requests_total",
            "Requests received by this process.",
        ),
        &["route"],
    )
    .unwrap();
    let active = IntGaugeVec::new(
        Opts::new(
            "junohub_http_in_flight",
            "Responses still in progress, including streams.",
        ),
        &["route"],
    )
    .unwrap();
    let completed = IntCounterVec::new(
        Opts::new(
            "junohub_http_completed_total",
            "Complete response outcomes, including cancellation and body failures.",
        ),
        &["route", "outcome"],
    )
    .unwrap();
    let duration = HistogramVec::new(
        HistogramOpts::new(
            "junohub_http_duration_seconds",
            "Time through completion or cancellation of the response body.",
        )
        .buckets(vec![0.1, 0.5, 1.0, 5.0, 15.0, 30.0, 60.0, 180.0]),
        &["route"],
    )
    .unwrap();
    let log_failures = IntCounter::new(
        "junohub_log_write_failures_total",
        "Failed writes of request logs or operation audit records.",
    )
    .unwrap();
    let security_rejections = IntCounterVec::new(
        Opts::new(
            "junohub_security_rejections_total",
            "Rejected unsafe requests and damaged authorization configurations.",
        ),
        &["reason"],
    )
    .unwrap();
    registry
        .register(Box::new(security_rejections.clone()))
        .unwrap();
    for reason in [
        "unowned_resource",
        "unmanaged_resource",
        "auth_rate_limit",
        "auth_dependency",
        "invalid_endpoint",
        "invalid_account_permissions",
    ] {
        security_rejections.with_label_values(&[reason]);
    }
    let uptime = IntGauge::new("junohub_uptime_seconds", "Time since process startup.").unwrap();
    registry.register(Box::new(started.clone())).unwrap();
    registry.register(Box::new(active.clone())).unwrap();
    registry.register(Box::new(completed.clone())).unwrap();
    registry.register(Box::new(duration.clone())).unwrap();
    registry.register(Box::new(log_failures.clone())).unwrap();
    registry.register(Box::new(uptime.clone())).unwrap();
    for route in ROUTES {
        started.with_label_values(&[route]);
        active.with_label_values(&[route]);
        duration.with_label_values(&[route]);
        for outcome in OUTCOMES {
            completed.with_label_values(&[route, outcome]);
        }
    }
    Metrics {
        registry,
        started,
        active,
        completed,
        duration,
        log_failures,
        security_rejections,
        uptime,
    }
});
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static INSTANCE: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

pub fn initialize() {
    LazyLock::force(&STARTED);
    LazyLock::force(&INSTANCE);
    LazyLock::force(&METRICS);
}
pub fn instance() -> &'static str {
    &INSTANCE
}
pub enum SecurityReason {
    UnownedResource,
    UnmanagedResource,
    AuthRateLimit,
    AuthDependency,
    InvalidEndpoint,
    InvalidAccountPermissions,
}
pub fn security_rejection(reason: SecurityReason) {
    let label = match reason {
        SecurityReason::UnownedResource => "unowned_resource",
        SecurityReason::UnmanagedResource => "unmanaged_resource",
        SecurityReason::AuthRateLimit => "auth_rate_limit",
        SecurityReason::AuthDependency => "auth_dependency",
        SecurityReason::InvalidEndpoint => "invalid_endpoint",
        SecurityReason::InvalidAccountPermissions => "invalid_account_permissions",
    };
    METRICS
        .security_rejections
        .with_label_values(&[label])
        .inc();
}
pub fn log_failure() {
    METRICS.log_failures.inc();
}
pub fn runtime() -> serde_json::Value {
    let mut resources =
        serde_json::json!({"resident_memory_bytes": null, "open_fds": null, "cpu_seconds": null});
    #[cfg(target_os = "linux")]
    for family in METRICS.registry.gather() {
        if let Some(metric) = family.get_metric().first() {
            match family.get_name() {
                "process_resident_memory_bytes" => {
                    resources["resident_memory_bytes"] =
                        serde_json::json!(metric.get_gauge().get_value())
                }
                "process_open_fds" => {
                    resources["open_fds"] = serde_json::json!(metric.get_gauge().get_value())
                }
                "process_cpu_seconds_total" => {
                    resources["cpu_seconds"] = serde_json::json!(metric.get_counter().get_value())
                }
                _ => {}
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = &mut resources;
    }
    serde_json::json!({"resources":resources,"uptime_seconds":STARTED.elapsed().as_secs(),"in_flight":ROUTES.iter().map(|r|METRICS.active.with_label_values(&[r]).get()).sum::<i64>(),"http_requests":ROUTES.iter().map(|r|METRICS.started.with_label_values(&[r]).get()).sum::<u64>(),"log_write_failures":METRICS.log_failures.get()})
}
fn route(path: &str) -> usize {
    match path {
        "/v1/chat/completions" | "/chat/completions" => 0,
        "/v1/responses" | "/responses" | "/backend-api/codex/responses" => 1,
        "/v1/messages" => 2,
        "/v1/images/generations" => 3,
        "/v1/images/edits" => 4,
        "/v1/videos/generations" => 5,
        "/v1/audio/generations" => 6,
        "/v1/audio/speech" => 7,
        "/v1/audio/transcriptions" => 8,
        "/v1/audio/translations" => 18,
        "/v1/embeddings" => 9,
        "/v1/search" => 10,
        "/v1/rerank" => 11,
        p if p.starts_with("/v1/tasks/") => 12,
        "/v1/models" | "/models" => 13,
        p if p.starts_with("/v1/models/") => 13,
        p if p.starts_with("/v1beta/") => 14,
        p if p.starts_with("/auth/") => 17,
        p if p.starts_with("/console/")
            || p.starts_with("/hub_admin/")
            || p.starts_with("/account-api/") =>
        {
            15
        }
        _ => 16,
    }
}
struct Guard {
    index: usize,
    started: Instant,
    status: u16,
    done: bool,
}
impl Guard {
    fn new(index: usize) -> Self {
        METRICS.started.with_label_values(&[ROUTES[index]]).inc();
        METRICS.active.with_label_values(&[ROUTES[index]]).inc();
        Self {
            index,
            started: Instant::now(),
            status: 499,
            done: false,
        }
    }
    fn finish(&mut self, status: u16) {
        if self.done {
            return;
        }
        self.done = true;
        METRICS
            .active
            .with_label_values(&[ROUTES[self.index]])
            .dec();
        let group = if status == 499 {
            5
        } else {
            match status / 100 {
                2 => 0,
                3 => 1,
                4 => 2,
                5 => 3,
                _ => 4,
            }
        };
        METRICS
            .completed
            .with_label_values(&[ROUTES[self.index], OUTCOMES[group]])
            .inc();
        METRICS
            .duration
            .with_label_values(&[ROUTES[self.index]])
            .observe(self.started.elapsed().as_secs_f64());
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done {
            self.finish(self.status);
        }
    }
}
pub async fn trace(mut request: Request, next: Next) -> Response {
    // Juno persists this UUID before dispatch and joins the invoice by it.
    // Keep valid incoming UUIDs; arbitrary header text must not enter tracing.
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    request
        .headers_mut()
        .insert("x-request-id", HeaderValue::from_str(&id).unwrap());
    let index = route(request.uri().path());
    let mut guard = Guard::new(index);
    let no_body = request.method() == axum::http::Method::HEAD;
    let span = tracing::info_span!("http_request",request_id=%id,route=ROUTES[index],method=%request.method());
    let response = next.run(request).instrument(span).await;
    let status = response.status().as_u16();
    let (mut parts, body) = response.into_parts();
    parts
        .headers
        .insert("x-request-id", HeaderValue::from_str(&id).unwrap());
    // 后端端口也对宿主机开放，补上基础防护头。
    // 这里不加 CSP：接口返回的是 JSON 与图片，CSP 由前端页面负责。
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
    ] {
        parts.headers.insert(
            axum::http::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    if no_body || body.is_end_stream() || matches!(status, 204 | 304) {
        guard.finish(status);
        return Response::from_parts(parts, body);
    }
    let mut stream = body.into_data_stream();
    // Captured guard is dropped even if the async stream is never polled.
    let stream = async_stream::stream! {
        let mut guard=guard;
        while let Some(chunk)=stream.next().await {
            if chunk.is_err(){guard.finish(502);}
            yield chunk;
        }
        guard.finish(status);
    };
    Response::from_parts(parts, Body::from_stream(stream))
}
fn mutates(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or_default(),
        "create"
            | "update"
            | "delete"
            | "import"
            | "upsert"
            | "reorder"
            | "sync"
            | "dispatch"
            | "complete"
            | "acknowledge"
            | "recheck"
    )
}
pub async fn audit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let actor = request.extensions().get::<CurrentUser>().map(|u| u.uid);
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let response = next.run(request).await;
    if mutates(&path) {
        if let Some(actor) = actor {
            if sqlx::query("INSERT INTO operation_audit(actor_id,action,request_id,status_code,created_at) VALUES(?,?,?,?,?)")
            .bind(actor).bind(&path).bind(request_id).bind(response.status().as_u16() as i64).bind(crate::store::now()).execute(&state.pool).await.is_err() {
            log_failure();tracing::error!("operation audit write failed");
        }
        }
    }
    response
}
pub fn prometheus() -> String {
    METRICS
        .uptime
        .set(STARTED.elapsed().as_secs().min(i64::MAX as u64) as i64);
    let mut output = Vec::new();
    TextEncoder::new()
        .encode(&METRICS.registry.gather(), &mut output)
        .expect("registered metrics are encodable");
    String::from_utf8(output).expect("Prometheus text is UTF-8")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_are_bounded_and_never_contain_client_ids() {
        assert_eq!(route("/v1/tasks/private-id"), 12);
        assert_eq!(route("/v1/models/private-model"), 13);
        assert_eq!(route("/private-token"), 16);
        assert!(!prometheus().contains("private"));
    }
    #[tokio::test]
    async fn trace_preserves_billing_uuid_and_replaces_invalid_ids() {
        use axum::{middleware, routing::get, Router};
        let app = Router::new()
            .route(
                "/billing-attribution",
                get(|headers: axum::http::HeaderMap| async move {
                    headers["x-request-id"].to_str().unwrap().to_owned()
                }),
            )
            .layer(middleware::from_fn(trace));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/billing-attribution",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let billing_id = uuid::Uuid::new_v4().to_string();
        for supplied in [
            Some(billing_id.as_str()),
            Some("arbitrary-client-text"),
            None,
        ] {
            let mut request = client.get(&url);
            if let Some(id) = supplied {
                request = request.header("x-request-id", id);
            }
            let response = request.send().await.unwrap();
            let trace_id = response.headers()["x-request-id"]
                .to_str()
                .unwrap()
                .to_owned();
            assert!(uuid::Uuid::parse_str(&trace_id).is_ok());
            assert_eq!(response.text().await.unwrap(), trace_id);
            if supplied == Some(billing_id.as_str()) {
                assert_eq!(trace_id, billing_id);
            } else {
                assert_ne!(trace_id, "arbitrary-client-text");
            }
        }
        server.abort();
    }

    #[tokio::test]
    async fn streaming_metrics_cover_body_completion_error_and_client_cancellation() {
        use axum::{middleware, routing::get, Router};
        let route_label = ROUTES[14];
        let active = METRICS.active.with_label_values(&[route_label]);
        let cancelled = METRICS
            .completed
            .with_label_values(&[route_label, "cancelled"]);
        let successful = METRICS.completed.with_label_values(&[route_label, "2xx"]);
        let failed = METRICS.completed.with_label_values(&[route_label, "5xx"]);
        let base_cancelled = cancelled.get();
        let base_ok = successful.get();
        let base_fail = failed.get();
        let app = Router::new()
            .route(
                "/v1beta/hold",
                get(|| async {
                    Body::from_stream(async_stream::stream! {
                        yield Ok::<_,std::io::Error>("data: first\n\n");
                        std::future::pending::<()>().await;
                    })
                }),
            )
            .route("/v1beta/complete", get(|| async { "done" }))
            .route("/v1beta/empty", get(|| async { "" }))
            .route(
                "/v1beta/error",
                get(|| async {
                    Body::from_stream(async_stream::stream! {
                        yield Ok::<_,std::io::Error>("partial");
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        yield Err(std::io::Error::other("mock body failure"));
                    })
                }),
            )
            .layer(middleware::from_fn(trace));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut response = client
            .get(format!("{url}/v1beta/hold"))
            .send()
            .await
            .unwrap();
        assert!(
            uuid::Uuid::parse_str(response.headers()["x-request-id"].to_str().unwrap()).is_ok()
        );
        response.chunk().await.unwrap().unwrap();
        assert_eq!(active.get(), 1);
        drop(response);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while active.get() != 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(cancelled.get(), base_cancelled + 1);
        assert_eq!(
            client
                .get(format!("{url}/v1beta/complete"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "done"
        );
        assert_eq!(successful.get(), base_ok + 1);
        let response = client
            .head(format!("{url}/v1beta/complete"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.bytes().await.unwrap().is_empty());
        assert!(client
            .get(format!("{url}/v1beta/empty"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .is_empty());
        assert_eq!(successful.get(), base_ok + 3);
        assert_eq!(cancelled.get(), base_cancelled + 1);

        let response = client
            .get(format!("{url}/v1beta/error"))
            .send()
            .await
            .unwrap();
        assert!(response.bytes().await.is_err());
        assert_eq!(failed.get(), base_fail + 1);
        assert_eq!(active.get(), 0);
        let text = prometheus();
        assert!(text.lines().any(
            |line| line.starts_with("junohub_http_duration_seconds_bucket{")
                && line.contains("le=\"+Inf\"")
                && line.contains("route=\"gemini\"")
        ));
        server.abort();
    }
}
