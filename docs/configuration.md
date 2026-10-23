# Configuration

Everything is optional — a bare `docker compose up -d` works. Precedence,
lowest to highest: built-in defaults ← `config.toml` in the config dir
(may be absent) ← `SUMIDOKU_*` environment variables.
`config.toml` keys are the snake_case names of the variables below
(`net.*` and `auth.*` sections for the grouped ones).

The one data root is `SUMIDOKU_CONFIG_DIR`: the database, cover cache,
source packages (`sources/`), materialized page images (`page-blobs/`)
and the default scheduled-backup destination (`backups/`) all live under
it and are not separately configurable. `sources/` in particular is
pinned — drop packages there and scan, no path setting exists.
`config.toml` is read from this directory too, so the config dir itself
is environment-only: a `config_dir` key in the file is ignored with a
warning (the file cannot relocate the directory it is read from).

## Volumes (Docker)

| Volume | Purpose |
| --- | --- |
| `/config` | the config dir: database, settings, cover cache, source packages |
| `/downloads` | downloaded chapters |

## Server

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_ADDR` | `0.0.0.0:3000` | listen address |
| `SUMIDOKU_CONFIG_DIR` | `/config` in Docker, `./data` otherwise | data root (see above); `config.toml` is read from here |
| `SUMIDOKU_LOG` | `info` | log level (tracing filter) |
| `SUMIDOKU_CALL_CONCURRENCY` | `8` | max concurrent calls into a source instance |
| `SUMIDOKU_USER_AGENT` | a Chrome desktop UA | global User-Agent for source fetches |
| `SUMIDOKU_COVER_CACHE_MAX` | `512` | max cached cover files on disk |
| `SUMIDOKU_WEBUI_DIR` | `./webui/build` | serve the web UI from this directory instead of the embedded build |
| `SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES` | `30` | evict source instances idle longer than this; `0` or negative disables the sweep |
| `SUMIDOKU_RSS_THRESHOLD_MB` | `1024` | process RSS threshold; above it the monitor evicts source instances most-idle first (in-flight calls are never touched). `0` disables the monitor |
| `SUMIDOKU_LOCAL_DIRS` | `[<config>/local]` | local manga roots scanned by the built-in Local source, semicolon-separated (empty segments ignored); the roots merge into one shelf, a relative path present in several roots lists the first root's copy, and keys from root N ≥ 1 carry the `N:` prefix. Overridable at runtime by the `local.dirs` setting from the Settings page |
| `SUMIDOKU_LOCAL_DIR` | — | one-item fallback for `SUMIDOKU_LOCAL_DIRS`, used only when that is unset |
| `SUMIDOKU_LOCAL_ONESHOTS_DIR` | — | name marking a oneshots directory for the Local source (empty = off): a directory whose relative path contains it (case-insensitive substring) is never listed itself, and every archive directly inside becomes its own one-chapter manga titled by the file stem. Overridable at runtime by the `local.oneshots_dir` setting from the Settings page |

## WASM runtime

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_WASM_MEMORY_MAX_MB` | `128` | per-source wasm linear-memory cap; `memory.grow` beyond it fails |
| `SUMIDOKU_WASM_FUEL` | `10000000000` | instruction budget per source export call |
| `SUMIDOKU_WASM_CALL_TIMEOUT_SECS` | `60` | per-call wall-clock timeout; a timed-out call restarts the instance |

## Library updates and downloads

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_UPDATE_INTERVAL_HOURS` | `0` (off) | fallback scheduled library update interval when `library.update_interval_hours` is unset |
| `SUMIDOKU_AUTO_DOWNLOAD_NEW_CHAPTERS` | `false` | download chapters found by library updates |
| `SUMIDOKU_AUTO_DOWNLOAD_LIMIT` | `5` | per-manga cap for auto downloads |
| `SUMIDOKU_DOWNLOADS_DIR` | `/downloads` in Docker, `./downloads` otherwise | downloads directory |
| `SUMIDOKU_DOWNLOAD_CONCURRENCY` | `3` | concurrent chapter downloads |
| `SUMIDOKU_DOWNLOAD_FORMAT` | `cbz` | download container format |
| `SUMIDOKU_DOWNLOAD_MOVE_PATH` | — | override `download.move_path` (setting default empty = off): move each packaged download to the rendered template path — variables `{source}`, `{title}`/`{manga}`, `{author}`, `{artist}`, `{chapter}`, `{filename}`; absolute targets like `/mnt/library/{author}/{manga}/{filename}` are the intended use |

The download and library-update behavior settings are re-read from the `setting`
table for each operation, so changes from the Settings page do not require a
restart. The corresponding file/env values above are fallbacks when a setting is
absent. `downloads_dir` remains deployment-level and has no runtime override.

| Setting key | Default | Purpose |
| --- | --- | --- |
| `auto_download_new_chapters` | `false` | automatically download chapters found by library updates |
| `auto_download_limit` | `5` | maximum automatic downloads per manga per update; `0` disables the candidates |
| `download_format` | `cbz` | `cbz` archive or `folder` image directory |
| `download_concurrency` | `3` | maximum sources downloading concurrently, with one chapter at a time per source |
| `library.excluded_download_categories` | `[]` | admin category ids excluded from automatic downloads only; manual downloads and reader-ahead are unaffected |
| `downloads.auto_skip_manga_with_unread` | `true` | skip automatic downloads when chapters that existed before the current refresh are unread; the new chapters themselves do not block |
| `downloads.auto_ignore_reuploads` | `false` | exclude candidates whose number is below the previous latest chapter number; update results, unread counts and notifications are unchanged |
| `delete_after_manual_read` | `false` | delete a download after directly marking its chapter read |
| `delete_after_read` | `false` | delete a download when the reader reaches the final page |
| `allow_delete_bookmarked` | `false` | allow the two read-driven deletion switches to delete bookmarked chapters |
| `downloads.download_ahead` | `false` | enqueue following unread chapters after a successful reader page load |
| `downloads.download_ahead_count` | `2` | maximum following unread chapters to consider for reader-ahead |

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
| `SUMIDOKU_AUTH_USERNAME` | `admin` | built-in admin's login username, written onto the user row on every boot; the login page prefills it |
| `SUMIDOKU_AUTH_PASSWORD` | — | built-in admin's password, re-seeded on every boot; enables authentication (username + password login) for the web UI and REST API |
| `SUMIDOKU_AUTH_API_KEYS` | — | comma-separated long-lived API keys; usable as `Authorization: Bearer <key>` |
| `SUMIDOKU_BASIC_PASSWORD` | — | legacy alias of `SUMIDOKU_AUTH_PASSWORD`; only applies when the latter is unset |

## Network

Sidecar setup and the routing model are in [docs/sidecars.md](sidecars.md).

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_NET_PROXY` | — | global forward proxy for source fetches |
| `SUMIDOKU_NET_TRAWL_PROXY` | — | trawl forward-proxy URL; fallback for challenged hosts when solving is unavailable or fails |
| `SUMIDOKU_NET_TRAWL_CA` | — | trawl's MITM root CA (PEM); required for HTTPS sources through trawl |
| `SUMIDOKU_NET_RELAY_DOMAINS` | — | comma-separated domains always fetched through trawl |
| `SUMIDOKU_NET_DIRECT_DOMAINS` | — | comma-separated domains pinned to direct (never escalated to trawl) |
| `SUMIDOKU_NET_ALLOW_DIRECT_FALLBACK` | `false` | allow falling back to direct when the proxy path fails |
| `SUMIDOKU_NET_BROWSER_HEADERS` | `true` | fill the browser header suite (`sec-ch-ua`, `accept`, `sec-fetch-*`, `upgrade-insecure-requests`) matching the Chrome user agent on source fetches; set `false` for honest semantics on API/XHR-heavy sources |
| `SUMIDOKU_NET_BROWSER_TRAWL` | — | trawl API URL (e.g. `http://trawl:8191`, requires trawl ≥ v1.9.1); browser backend for webview using trawl sessions, and the Cloudflare clearance solver: a challenged host is solved once here, then replayed direct with the solving browser's user agent |
| `SUMIDOKU_NET_BODY_MAX_MB` | `32` | response body cap for source fetches; shared by the wasm net host and the server-side cover/page fetches because both buffer a source-controlled endpoint's body in the same process — the cap counts decoded bytes; `0` disables |
| `SUMIDOKU_NET_ZIP_BODY_MAX_MB` | `128` | body cap for zip page archives, which pack a whole chapter into one response and legitimately exceed the page-image cap; `0` disables |

## config.toml example

Placed in the config dir (`./data/config.toml` outside Docker, `/config/config.toml` in the image):

```toml
addr = "0.0.0.0:3000"
log = "info"

[auth]
username = "admin"
password = "change-me"
api_keys = ["long-lived-key"]

[net]
proxy = "http://proxy:8080"
trawl_proxy = "http://trawl:8080"
browser_trawl = "http://trawl:8191"
```
