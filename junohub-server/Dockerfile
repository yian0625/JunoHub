FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY migrations ./migrations
# 只构建发布需要的两个 bin。默认的 `cargo build` 会把 junohub-connect 及其
# client_adapters（约 4.4 万行）一起编译，白白增加构建时间。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    touch src/main.rs \
    && cargo build --release --bin junohub-server --bin junohub-backup \
    && cp /app/target/release/junohub-server /app/junohub-server \
    && cp /app/target/release/junohub-backup /app/junohub-backup

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data junohub \
    && mkdir -p /data \
    && chown junohub:junohub /data
COPY --from=builder /app/junohub-server /usr/local/bin/junohub-server
COPY --from=builder /app/junohub-backup /usr/local/bin/junohub-backup
WORKDIR /data
ENV JUNOHUB_BIND=0.0.0.0:9003
ENV JUNOHUB_DATABASE_URL=sqlite:///data/junohub.db?mode=rwc
EXPOSE 9003
USER junohub
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=5 \
  CMD curl --fail --silent http://127.0.0.1:9003/health/readiness >/dev/null || exit 1
CMD ["junohub-server"]
