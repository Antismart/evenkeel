# evenkeel-server: multi-stage build. SQLX_OFFLINE uses the committed .sqlx
# query data so the build needs no database.
FROM rust:1.96-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY .sqlx ./.sqlx
ENV SQLX_OFFLINE=true
RUN cargo build --release -p evenkeel-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/evenkeel-server /usr/local/bin/evenkeel-server
EXPOSE 3030
HEALTHCHECK --interval=10s --timeout=3s --retries=5 CMD curl -sf http://127.0.0.1:3030/healthz || exit 1
ENTRYPOINT ["evenkeel-server"]
