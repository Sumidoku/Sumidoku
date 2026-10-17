# syntax=docker/dockerfile:1

FROM node:22-alpine AS webui
WORKDIR /webui
COPY webui/package.json webui/package-lock.json ./
RUN npm ci
COPY webui/ ./
RUN npm run build

FROM rust:1-bookworm AS server
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY vendor/ vendor/
COPY crates/ crates/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p suzuri-server && \
    cp target/release/suzuri-server /usr/local/bin/suzuri-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -u 1000 -m suzuri
COPY --from=server /usr/local/bin/suzuri-server /usr/local/bin/suzuri-server
COPY --from=webui /webui/build /webui

ENV SUZURI_ADDR=0.0.0.0:3000 \
    DATABASE_URL=sqlite:///config/suzuri.db?mode=rwc \
    SUZURI_SOURCES_DIR=/sources \
    SUZURI_DOWNLOADS_DIR=/downloads \
    SUZURI_COVERS_DIR=/config/covers \
    SUZURI_WEBUI_DIR=/webui

USER suzuri
EXPOSE 3000
VOLUME ["/config", "/sources", "/downloads"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD curl -fsS http://localhost:3000/health || exit 1

CMD ["suzuri-server"]
