# Configuration

Everything is optional — a bare `docker compose up -d` works. Precedence,
lowest to highest: built-in defaults ← `./config.toml` (relative to the
working directory, may be absent) ← `SUMIDOKU_*` environment variables.
`config.toml` keys are the snake_case names of the variables below
(`net.*` and `auth.*` sections for the grouped ones).

The one data root is `SUMIDOKU_CONFIG_DIR`: the database, cover cache,
source packages (`sources/`), materialized page images (`page-blobs/`)
and the default scheduled-backup destination (`backups/`) all live under
it and are not separately configurable. `sources/` in particular is
pinned — drop packages there and scan, no path setting exists.

## Volumes (Docker)

| Volume | Purpose |
| --- | --- |
| `/config` | the config dir: database, settings, cover cache, source packages |
| `/downloads` | downloaded chapters |

## Server

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_ADDR` | `0.0.0.0:3000` | listen address |
| `SUMIDOKU_CONFIG_DIR` | `/config` in Docker, `./data` otherwise | data root (see above) |
| `SUMIDOKU_LOG` | `info` | log level (tracing filter) |
| `SUMIDOKU_CALL_CONCURRENCY` | `8` | max concurrent calls into a source instance |
| `SUMIDOKU_USER_AGENT` | a Chrome desktop UA | global User-Agent for source fetches |
| `SUMIDOKU_COVER_CACHE_MAX` | `512` | max cached cover files on disk |
| `SUMIDOKU_WEBUI_DIR` | `./webui/build` | serve the web UI from this directory instead of the embedded build |
| `SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES` | `30` | evict source instances idle longer than this; `0` or negative disables the sweep |
| `SUMIDOKU_RSS_THRESHOLD_MB` | `1024` | process RSS threshold; above it the monitor evicts source instances most-idle first (in-flight calls are never touched). `0` disables the monitor |
| `SUMIDOKU_LOCAL_DIR` | `<config>/local` | local manga root scanned by the built-in Local source: one directory per manga, chapters as subdirectories (image folders) or `.cbz`/`.zip` files; may live outside the config dir |

## WASM runtime

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_WASM_MEMORY_MAX_MB` | `128` | per-source wasm linear-memory cap; `memory.grow` beyond it fails |
| `SUMIDOKU_WASM_FUEL` | `10000000000` | instruction budget per source export call |
| `SUMIDOKU_WASM_CALL_TIMEOUT_SECS` | `60` | per-call wall-clock timeout; a timed-out call restarts the instance |

## Library updates and downloads

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_UPDATE_INTERVAL_HOURS` | `0` (off) | scheduled library update interval |
| `SUMIDOKU_AUTO_DOWNLOAD_NEW_CHAPTERS` | `false` | download chapters found by library updates |
| `SUMIDOKU_AUTO_DOWNLOAD_LIMIT` | `5` | per-manga cap for auto downloads |
| `SUMIDOKU_DOWNLOADS_DIR` | `/downloads` in Docker, `./downloads` otherwise | downloads directory |
| `SUMIDOKU_DOWNLOAD_CONCURRENCY` | `3` | concurrent chapter downloads |
| `SUMIDOKU_DOWNLOAD_FORMAT` | `cbz` | download container format |
| `SUMIDOKU_DOWNLOAD_MOVE_PATH` | — | override `download.move_path` (setting default empty = off): move each packaged download to the rendered template path — variables `{source}`, `{title}`/`{manga}`, `{author}`, `{artist}`, `{chapter}`, `{filename}`; absolute targets like `/mnt/library/{author}/{manga}/{filename}` are the intended use |
| `SUMIDOKU_OPDS_LAZY_DOWNLOAD` | `false` | let OPDS readers trigger downloads of missing pages |

## Backup

These four override the `backup.*` settings (headless deployments); see
[docs/backup.md](backup.md) for the format and scheduling model.

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_BACKUP_ENABLED` | — | force scheduled backups on/off, overriding `backup.enabled` |
| `SUMIDOKU_BACKUP_INTERVAL_HOURS` | — | override `backup.interval_hours` (setting default 24) |
| `SUMIDOKU_BACKUP_RETENTION_COUNT` | — | override `backup.retention_count` (setting default 10) |
| `SUMIDOKU_BACKUP_DIR` | `<config>/backups` | scheduled-backup destination; may live outside the config dir |

## Authentication

See [docs/authentication.md](authentication.md) for the model.

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_AUTH_PASSWORD` | — | built-in admin's password, re-seeded on every boot; enables authentication (username + password login) for the web UI, REST API and OPDS |
| `SUMIDOKU_AUTH_API_KEYS` | — | comma-separated long-lived API keys; usable as `Authorization: Bearer <key>` |
| `SUMIDOKU_BASIC_PASSWORD` | — | legacy alias of `SUMIDOKU_AUTH_PASSWORD`; only applies when the latter is unset |

## Network

Sidecar setup and the routing model are in [docs/sidecars.md](sidecars.md).

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_NET_PROXY` | — | global forward proxy for source fetches |
| `SUMIDOKU_NET_TRAWL_PROXY` | — | trawl forward-proxy URL; challenged hosts are solved once and then kept on trawl |
| `SUMIDOKU_NET_TRAWL_CA` | — | trawl's MITM root CA (PEM); required for HTTPS sources through trawl |
| `SUMIDOKU_NET_RELAY_DOMAINS` | — | comma-separated domains always fetched through trawl |
| `SUMIDOKU_NET_DIRECT_DOMAINS` | — | comma-separated domains pinned to direct (never escalated to trawl) |
| `SUMIDOKU_NET_ALLOW_DIRECT_FALLBACK` | `false` | allow falling back to direct when the proxy path fails |
| `SUMIDOKU_NET_BROWSER_HEADERS` | `true` | fill the browser header suite (`sec-ch-ua`, `accept`, `sec-fetch-*`, `upgrade-insecure-requests`) matching the Chrome user agent on source fetches; set `false` for honest semantics on API/XHR-heavy sources |
| `SUMIDOKU_NET_BROWSER_TRAWL` | — | trawl API URL (e.g. `http://trawl:8191`, requires trawl ≥ v1.9.1); browser backend for webview using trawl sessions |
| `SUMIDOKU_NET_BODY_MAX_MB` | `32` | response body cap for source fetches; shared by the wasm net host and the server-side cover/page fetches because both buffer a source-controlled endpoint's body in the same process — the cap counts decoded bytes; `0` disables |
| `SUMIDOKU_NET_ZIP_BODY_MAX_MB` | `128` | body cap for zip page archives, which pack a whole chapter into one response and legitimately exceed the page-image cap; `0` disables |

## config.toml example

```toml
addr = "0.0.0.0:3000"
config_dir = "/config"
log = "info"

[auth]
password = "change-me"
api_keys = ["long-lived-key"]

[net]
proxy = "http://proxy:8080"
trawl_proxy = "http://trawl:8080"
browser_trawl = "http://trawl:8191"
```
