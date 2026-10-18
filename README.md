# Sumidoku

[![CI](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml/badge.svg)](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/Sumidoku/Sumidoku)](https://github.com/Sumidoku/Sumidoku/releases)
[![docker pulls](https://img.shields.io/docker/pulls/sumidoku/sumidoku)](https://hub.docker.com/r/sumidoku/sumidoku)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Self-hosted manga server: Aidoku WASM sources, REST + OPDS API, shipped as a single binary or a Docker image.

## Quick start

```sh
docker compose up -d
```

The compose file pulls the published `sumidoku/sumidoku:latest` image and mounts `./config` and `./downloads`. Open http://localhost:3000. The same image is published to GitHub Container Registry as `ghcr.io/sumidoku/sumidoku`; point the compose `image:` at either.

## Volumes and environment

| Volume | Purpose |
| --- | --- |
| `/config` | database, settings, cover cache, source packages |
| `/downloads` | downloaded chapters |

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_ADDR` | `0.0.0.0:3000` | listen address |
| `SUMIDOKU_CONFIG_DIR` | `/config` | data root: database, covers and sources live under it |
| `SUMIDOKU_DOWNLOADS_DIR` | `/downloads` | downloads directory |
| `SUMIDOKU_COVERS_DIR` | `/config/covers` | cover cache directory |
| `SUMIDOKU_NET_TRAWL_PROXY` | — | trawl forward-proxy URL; enables Cloudflare-bypass retries |
| `SUMIDOKU_NET_TRAWL_CA` | — | trawl's MITM root CA (PEM); required for HTTPS sources through trawl |
| `SUMIDOKU_NET_RELAY_DOMAINS` | — | comma-separated domains always fetched through trawl |
| `SUMIDOKU_NET_BROWSER_WS` | — | Chromium CDP websocket URL; enables real-browser webview for source login flows |

## Cloudflare bypass (trawl)

Sources behind Cloudflare anti-bot protection are fetched through a [trawl](https://github.com/germondai/trawl) sidecar: a challenge-aware forward proxy that passes ordinary traffic through and solves detected challenges with a real browser.

With `SUMIDOKU_NET_TRAWL_PROXY` set, a direct request answered `403`/`503` with Cloudflare headers is retried once through trawl. Cookies on the solved response (`cf_clearance`) land in the source's session jar, so later direct requests reuse the clearance; when it expires the cycle repeats. Sources that challenge every request can skip the failed direct attempt by listing their domains in `SUMIDOKU_NET_RELAY_DOMAINS`. Without the variable, behavior is unchanged: requests go direct and the challenge page reaches the source.

HTTPS sources also need `SUMIDOKU_NET_TRAWL_CA`: trawl terminates TLS locally to inspect responses and re-signs with its own root CA, which the client must trust. The compose file already runs the sidecar and shares the CA through the `trawl-ca` volume — trawl writes `ca.crt` into it on first boot, Sumidoku reads it from `/trawl-ca/ca.crt`.

## Browser-backed webview (source login)

Sources that log in through a web view (load the login page in a real browser, then call their API with the session cookie) need a Chromium sidecar: Sumidoku ships no browser, it drives one over CDP. Point `SUMIDOKU_NET_BROWSER_WS` at the sidecar's CDP websocket — for the [browserless](https://ghcr.io/browserless/chromium) image that is `ws://<host>:3000/chromium`; other CDP-capable images (e.g. headless-shell on :9222) work the same way, with whatever path that image serves.

The compose file carries an optional `chromium` service, off by default:

```sh
docker compose --profile browser up -d
```

and `SUMIDOKU_NET_BROWSER_WS=ws://chromium:3000/chromium` (a commented line is already in the compose environment). Each webview a source opens gets an isolated browser context — cookies never bleed across webviews — and after every page load the context's cookies flow into the source's session jar, so once the login page completes, the source's own API requests carry the session. Without the variable, behavior is unchanged: the navigation/cookie webview functions report themselves unsupported in the source log.

## Installing sources

Drop `.aix` source packages (or extracted source directories) into `./config/sources`, then press the scan button on the web UI's browse page, or call:

```sh
curl -X POST http://localhost:3000/api/v1/sources/scan
```

## OPDS

The OPDS 1.2 catalog is served at `/opds/v1.2`. Basic auth applies to the catalog only, and only when `SUMIDOKU_BASIC_PASSWORD` is set (any non-empty username).

## Local build

The web UI is embedded into the binary at compile time, so it must be built first:

```sh
cd webui && npm ci && npm run build && cd ..
cargo build --release -p sumidoku-server
```

## Development

- `cargo test` — server test suite
- `cd webui && npm run dev` — Vite dev server, proxying API calls to the server on :3000

## License

MIT
