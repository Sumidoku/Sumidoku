# syntax=docker/dockerfile:1

# Packages a prebuilt sumidoku-server binary; nothing is compiled here.
# Expected layout, as produced by the release workflow (or by hand for a
# local build):
#   dist/amd64/sumidoku-server   x86_64-unknown-linux-gnu build
#   dist/arm64/sumidoku-server   aarch64-unknown-linux-gnu build
# The web UI is already embedded in the binary (rust-embed over webui/build).

# trixie: runtime glibc (2.41) must cover the CI build image's (ubuntu-24.04, 2.39)
FROM debian:trixie-slim
ARG TARGETARCH
# compose healthchecks shell out to curl; the slim base doesn't ship one
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -u 1000 -m sumidoku
LABEL org.opencontainers.image.source="https://github.com/Sumidoku/Sumidoku" \
      org.opencontainers.image.description="Self-hosted manga server: Aidoku WASM sources, REST API" \
      org.opencontainers.image.licenses="MIT"
COPY --chmod=755 "dist/$TARGETARCH/sumidoku-server" /usr/local/bin/sumidoku-server
# 777 so an arbitrary --user uid:gid can write when nothing is bind-mounted
RUN install -d -m 777 /config /downloads
ENV SUMIDOKU_ADDR=0.0.0.0:3000 \
    SUMIDOKU_CONFIG_DIR=/config \
    SUMIDOKU_DOWNLOADS_DIR=/downloads
USER sumidoku
EXPOSE 3000
VOLUME ["/config", "/downloads"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD curl -fsS http://localhost:3000/health || exit 1

CMD ["sumidoku-server"]
