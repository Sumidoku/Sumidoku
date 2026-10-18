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
| `SUMIDOKU_AUTH_PASSWORD` | — | site password; enables authentication for the web UI, REST API and OPDS |
| `SUMIDOKU_AUTH_API_KEYS` | — | comma-separated long-lived API keys; usable as `Authorization: Bearer <key>` |
| `SUMIDOKU_BASIC_PASSWORD` | — | legacy alias of `SUMIDOKU_AUTH_PASSWORD`; only applies when the latter is unset |
| `SUMIDOKU_BACKUP_ENABLED` | — | force scheduled backups on/off, overriding the `backup.enabled` setting |
| `SUMIDOKU_BACKUP_INTERVAL_HOURS` | — | override the `backup.interval_hours` setting (default 24) |
| `SUMIDOKU_BACKUP_RETENTION_COUNT` | — | override the `backup.retention_count` setting (default 10) |
| `SUMIDOKU_BACKUP_DIR` | `<config>/backups` | scheduled-backup destination, overriding the `backup.dir` setting; may live outside the config dir |

## Authentication

Authentication is off until `SUMIDOKU_AUTH_PASSWORD` (or the legacy
`SUMIDOKU_BASIC_PASSWORD`) is set. With a password configured, the web UI
shows a login page, `/api/v1` requires `Authorization: Bearer <token>` (a
login-issued session token or a configured API key), and the OPDS catalogs
require Basic auth with the same password. Unauthenticated requests get a
uniform `401` (`{"error":{"code":"unauthorized"}}`). The config-set password
is hashed (argon2) into the database on every boot, so it overrides a
password previously changed through the UI; API keys stay in the config
only.

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

The OPDS 1.2 catalog is served at `/opds/v1.2`. When authentication is enabled, the catalogs require Basic auth with the site password (any non-empty username); see [Authentication](#authentication).

## Backup

`GET /api/v1/backup` downloads the full library state — library membership and manga metadata, categories, reading progress, app settings and source settings — as one JSON document with a `version` field (currently `1`; unknown versions are rejected with `400`). `POST /api/v1/backup` imports it back, optionally limited with `?sections=library,categories,progress,settings,sourceSettings`. Import upserts by natural key (source key + manga key, chapter key, category name) inside a single transaction, so a failed or invalid backup leaves the database untouched; entries whose source is not installed are skipped and reported in the response `warnings`. The settings page of the web UI has download and restore buttons. The file contains source settings, which may include site passwords — store it accordingly.

## Scheduled backups

The server can also write backups on its own schedule, so a corrupted database or a bad change between manual exports stays recoverable. It is configured through the `setting` table (the web UI's settings page exposes all of it under "自动备份"), and each value can be pinned by the env override above for headless deployments:

| Setting key | Default | Purpose |
| --- | --- | --- |
| `backup.enabled` | `false` | master switch |
| `backup.interval_hours` | `24` | hours between runs (minimum 1) |
| `backup.retention_count` | `10` | how many newest files to keep |
| `backup.dir` | `<config>/backups` | destination directory |

When enabled, a background task writes `sumidoku-backup-<yyyymmdd-HHMMSS>.json` (UTC) every interval — the same document `GET /api/v1/backup` serves — stamps `backup.last_run`, and deletes files beyond the retention count, oldest first; the newest file is never deleted, even with `backup.retention_count = 0`. At startup, if more than one interval elapsed since `backup.last_run` (or no backup was ever taken), one backup runs immediately, so downtime does not leave a gap. `GET /api/v1/backups` lists the files (name, size, creation time, newest first); `POST /api/v1/backups/restore` with `{"name": "<file>"}` restores one through the same validated import path as the upload endpoint, and the settings page lists them with a restore action.

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
