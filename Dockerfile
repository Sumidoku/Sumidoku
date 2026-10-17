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
    cargo build --release -p sumidoku-server && \
    cp target/release/sumidoku-server /usr/local/bin/sumidoku-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -u 1000 -m sumidoku
COPY --from=server /usr/local/bin/sumidoku-server /usr/local/bin/sumidoku-server
COPY --from=webui /webui/build /webui

ENV SUMIDOKU_ADDR=0.0.0.0:3000 \
    DATABASE_URL=sqlite:///config/sumidoku.db?mode=rwc \
    SUMIDOKU_SOURCES_DIR=/sources \
    SUMIDOKU_DOWNLOADS_DIR=/downloads \
    SUMIDOKU_COVERS_DIR=/config/covers \
    SUMIDOKU_WEBUI_DIR=/webui

USER sumidoku
EXPOSE 3000
VOLUME ["/config", "/sources", "/downloads"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD curl -fsS http://localhost:3000/health || exit 1

CMD ["sumidoku-server"]
