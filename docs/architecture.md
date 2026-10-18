# Architecture

One page for the whole system, verified against the linked source. Update
the owning section in the same PR when a change alters a contract described
here. Link to symbols rather than copying details that drift.

## Server (`crates/server`)

axum 0.8, SQLite via sqlx. Entry `main.rs`; app wiring `lib.rs:build_app`.
Migrations in `crates/server/migrations`.

- REST API under `/api/v1` (`api/mod.rs`, `api/library.rs`, `api/reader.rs`,
  `api/page.rs`, `api/download.rs`, `api/update.rs`, `api/repo.rs`,
  `api/backup.rs`).
  `GET /api/v1/sources/{id}/home` (`api/mod.rs:get_home`) returns the Aidoku
  `HomeLayout` as-is — an ordered `components` array; the array index is the
  stable key a `send_partial_result` stream patches against. A source
  without `get_home` answers 404 `unsupported`, same as dynamic filters and
  listings.
- Partial-result streaming (`api/partial.rs`): the home and search endpoints
  honor `Accept: application/x-ndjson` as an explicit opt-in. The source's
  mid-request `send_partial_result` payloads arrive as one JSON event per
  line in send order, then a terminal event closes the stream: home emits
  `{"type":"layout","layout":…}` (replace whole layout) and
  `{"type":"component","index":N,"component":…}` (replace at index, or
  append when `index` equals the current length — the index is resolved
  from the upstream title-upsert semantics by the wasm layer, see
  `crates/wasm/src/partial.rs`), search emits per-Manga deltas
  `{"type":"manga","manga":…}`. Both end with
  `{"type":"final","payload":…}` where the payload is the same JSON value
  as the buffered response, or `{"type":"error","error":{code,message}}`
  when the source call fails after streaming started. Requests without
  the header take the original buffered path unchanged. The connection is
  the key: a client disconnect drops the body stream, later sends fail and
  the forward task stops while the source call runs to completion.
- Source repositories (`source/repo.rs`, `api/repo.rs`): Aidoku-style repo
  indexes are fetched on add/refresh and cached in the `source_repo` table;
  browsing, install and update detection all read the cache, so an
  unreachable repo never breaks the installed-source list. Install downloads
  the `.aix` (http/https only, 64 MiB cap) into `sources/` and registers it
  through the same `register_aix` path as the directory scan; versions
  compare as Aidoku's monotonic integers.
- Pinned sources (`api/pins.rs`): an ordered list of source keys in the
  global `setting` table under `browse.pinned_sources` — Aidoku's
  `Browse.pinnedList` analogue, no schema migration. The array order is the
  pin order and seeds the destination priority of cross-source migration.
  `GET /api/v1/sources` items carry `pinned` and `pinOrder` (index in that
  list, null when unpinned) via `repo::source_json`.
  `PATCH /api/v1/sources/{id}` `{pinned: bool}` pins (appended to the tail)
  or unpins (removed); `POST /api/v1/sources/reorder-pinned` `{ids: [...]}`
  replaces the order wholesale and answers 400 unless ids are exactly the
  current pinned set, so a reorder never silently drops a pin.
  `delete_source` prunes the key. The Browse page shows the pinned sources
  in an ordered section above the full list, with drag-to-reorder
  (`webui/src/pages/BrowsePage.tsx`).
- Authentication (`auth.rs`): off until a password exists — either seeded
  from the config (`auth.password`, or the legacy `basic_password` alias)
  on every boot, or stored in the `auth_credential` table by a previous
  boot/password change. With a password, `require_auth` guards all of
  `/api/v1` except `/health`, `auth/status` and `auth/login`: the bearer
  credential (header, or `access_token` query for `<img>`/WebSocket
  clients) is checked against the configured `auth.api_keys` (SHA-256
  fingerprints in memory) and session tokens in `auth_token` (SHA-256 at
  rest, 30-day rolling expiry). Login/logout/password-change live under
  `/api/v1/auth/`; changing the password revokes every session token.
  Failures share one `401 {"error":{"code":"unauthorized"}}` shape. The
  password is argon2-hashed; the config-set password always re-seeds the
  stored hash at boot, so it wins over UI-changed passwords after a
  restart.
- OPDS catalogs: 1.2 (Atom) at `/opds/v1.2` (`opds/mod.rs`) and 2.0 (Readium
  Web Publication JSON) at `/opds/v2` (`opds/v2.rs`). The 2.0 feed builds its
  URLs from the 1.2 cover/file/stream endpoints; the two share auth, paging
  and query helpers from `opds/mod.rs`. Basic auth applies to the catalogs
  when authentication is enabled, checking the same site password (any
  non-empty username) — repeated verifications hit a one-entry cache so
  per-page image requests don't pay argon2 each time.
- The web UI is served from the binary: release builds embed `webui/build`
  (`webui_assets.rs`, rust-embed); debug builds read it from disk.
- Library model (`model.rs`, `api/library.rs`): manga → chapter → page rows;
  `source_order` is the source's own chapter index. Chapter reconcile
  (`source/mod.rs:reconcile_chapters`) keeps a chapter the source stopped
  listing when it has download rows, and deletes it otherwise.
- Backup export/import (`api/backup.rs`): `GET /api/v1/backup` answers one
  JSON document (a `Content-Disposition` attachment) with the single user's
  library rows and metadata, categories with membership, reading progress,
  app settings and source settings, plus source identity (key, name, version
  and a best-effort repo URL). `POST /api/v1/backup` restores it, optionally
  limited by `?sections=` to some of `library`, `categories`, `progress`,
  `settings`, `sourceSettings`. Restore upserts by natural key — source
  key + manga key, chapter key, category name — inside one SQLite
  transaction; an unknown `version` or an invalid row answers 400 with the
  database untouched. Entries naming a source that is not installed are
  skipped and listed in the response `warnings`; the `restored` map counts
  applied rows per section. Progress entries carry a chapter metadata
  snapshot so a fresh instance can recreate chapter rows ahead of the
  source's first fetch. The document's `version` field (currently `1`) is
  the format contract, and source setting values round-trip in the same
  bare-JSON form `PUT /sources/{id}/settings` accepts, so a backup can
  contain source credentials: treat the file as sensitive.
  The construction (`build_backup_doc`) and the restore execution
  (`import_backup_doc`) are shared with the scheduler and the
  restore-from-list endpoint below, so a scheduled file is the same
  document the GET handler serves.
- Aidoku backup import (`api/aidoku_backup.rs`):
  `POST /api/v1/import/aidoku` accepts a `.aib` upload — a binary plist of
  Aidoku's Codable `Backup` struct, decoded with the `plist` crate into
  tolerant structs that only name the consumed fields. It imports library
  membership (`library` + `manga` metadata), reading progress (`history` +
  `chapters` metadata, creating manga/chapter rows as needed, not in
  library for manga the backup does not list), and categories with
  membership (top-level `categories`, accepting both the bare-string and
  object plist shapes). Aidoku's `mangaId`/`chapterId` are the same
  source-ecosystem keys as Sumidoku's `manga.key`/`chapter.key`, so rows
  reconcile by (source key, manga key[, chapter key]) with no URL
  translation; Aidoku's `nsfw` stores ContentRating raw value minus one,
  so it maps 1→2 (suggestive) and 2→3 (NSFW) with 0 read as unknown.
  Unlike the BackupDoc restore's upsert, the merge is additive: existing
  library entries and categories keep their state (only `in_library` is
  raised and membership added), and progress merges
  `max(last_page_read)`, OR-ed `is_read` and the newer `last_read_at`.
  Entries whose source is not installed are skipped and reported in the
  response `skipped` list with a reason; the `imported` map counts
  processed entries per section. `?dryRun=true` runs the same single
  transaction and rolls it back, so the report is identical to a real
  run's and the database is untouched; an unparseable or wrongly shaped
  file answers 400 with no writes. The web UI settings page has the
  upload → dry-run preview → confirm flow under the backup section.
- Suwayomi/Mihon backup import (`api/suwayomi_backup.rs`):
  `POST /api/v1/import/suwayomi` accepts a Suwayomi-Server or
  Mihon/Tachiyomi backup as a binary body. Both apps write the same
  bytes — gzip around one protobuf `Backup` message — whatever the
  extension (`.tachibk`, `.proto.gz`); the endpoint sniffs magic bytes
  and also accepts a raw proto (as Mihon's decoder does) and, defensively,
  a zip holding a `.proto` entry. The prost messages are hand-written
  against the wire schema (Suwayomi's `backup/proto/models`, Mihon's
  `data/backup/models`), declaring only consumed fields so tracking,
  memos, preferences and Suwayomi's `meta` maps are skipped on decode.
  The merge reuses the same additive primitives as the Aidoku import
  (`api/import_merge.rs`), with two format-specific twists: a manga is
  in the library only when `favorite` is set (kotlinx ProtoBuf omits
  default-valued scalars, so the wire never carries `favorite=true` —
  absence decodes as true), and category membership references category
  `order` values, not array positions. Timestamps arrive as epoch
  milliseconds with 0 meaning absent. The hard part is source mapping:
  backups name Mihon sources by int64 id plus a display name (no
  language on the wire), while Sumidoku runs Aidoku sources. Each
  backup source group auto-matches only when exactly one installed
  source shares the normalized name (lowercase, alphanumeric only);
  zero or multiple hits stay unmatched — the importer never guesses —
  and the response `sources` list reports every group's state
  (`auto`/`mapped`/`unmatched`, plus the name-match candidates) with
  manga counts. A real run accepts
  `?sourceMapping={"<mihonSourceId>": "<aidokuSourceKey>"}` (ids are
  int64 hashes that can exceed 2^53, hence string keys) to resolve
  groups explicitly; unmatched-and-unmapped groups skip wholesale into
  the `skipped` list. `?dryRun=true` rolls back the same transaction.
  The web UI walks upload → grouped dry-run preview → per-source
  mapping dropdowns → confirm → result. Tracking, extension settings,
  saved searches and server preferences are out of scope; the manga
  `url` is the Mihon source's internal key, which may not line up with
  the corresponding Aidoku source's manga key — such entries import
  but a later refresh can miss them.
- Scheduled backups (`backup/mod.rs`): a `BackupScheduler` task (wired in
  `build_app` like the update scheduler) wakes every 60s, re-reads the
  `backup.*` keys from the `setting` table and, when `backup.enabled` is
  set and the run is overdue — no `backup.last_run` yet, or more than
  `backup.interval_hours` since it — writes
  `sumidoku-backup-<yyyymmdd-HHMMSS>.json` (UTC) to the backup dir through
  a tmp+rename swap, stamps `backup.last_run`, and prunes. The first tick
  doubles as the startup overdue check. Pruning keeps the newest
  `max(backup.retention_count, 1)` files by name order — the newest file
  is never deleted, even at retention 0 — and ignores files not matching
  the name pattern. Each setting resolves env (`SUMIDOKU_BACKUP_ENABLED` /
  `_INTERVAL_HOURS` / `_RETENTION_COUNT` / `_DIR`) over the `setting`
  table over the built-in defaults (disabled, 24h, keep 10,
  `<config_dir>/backups`). `GET /api/v1/backups` lists the dir
  (name/size/createdAt, newest first, plus the effective `dir`);
  `POST /api/v1/backups/restore` reads a listed file server-side and runs
  it through `import_backup_doc`, accepting only names that match the
  backup pattern (everything else, `..` included, is a 400; a
  well-shaped but missing file is a 404).
- Downloads (`download/mod.rs`): serial within a source, parallel across
  sources, with a page-level fetch cap inside a chapter.
- Page blobs (`source/mod.rs:materialize_image_pages`): a
  `PageContent::Image` page holds an actor-local rid, so `load_pages`
  exchanges it for PNG bytes in the same call that fetched the page list
  and stores them under `config_dir/page-blobs/<chapter_id>/<idx>.png`; the
  page row keeps just the file name. The reader (`api/page.rs`), OPDS-PSE
  (`source/mod.rs:page_content`) and downloads (`download/mod.rs:
  page_bytes`) all read the blob; chapter dirs whose chapter row is gone
  are swept at startup (`source/mod.rs:sweep_page_blobs`).
- Image processors: when a source exports `process_page_image` /
  `process_cover_image`, `fetch_page_image` / `fetch_cover_image`
  (`source/mod.rs`) pass the fetched response through the actor
  (`SourceHandle::process_page_image`) and replace the bytes with the
  returned PNG — for the reader, OPDS, downloads and the cover endpoint
  alike. Cover caching is unchanged: the processed bytes are what the
  on-disk cache stores.
- Library updates (`update/mod.rs`): same scheduling shape as downloads —
  serial per source, at most 3 sources at once — broadcasting progress as
  `UpdateEvent`s. Category `include_in_update` is a three-state filter.
- Outbound requests pass a `NetPolicy` (`source/routed.rs:SumidokuNetPolicy`):
  direct, per-domain relay, or a global proxy, decided per host. When
  `net.trawl_proxy` is set, a direct request answered 403/503 with Cloudflare
  headers retries once through the trawl forward proxy (MITM CA in
  `net.trawl_ca`), ahead of the 429/5xx backoff — in the source net imports
  (`crates/wasm/src/imports/net.rs:perform`) and in the server's own fetches
  alike; `net.direct_domains` opts a host out, `net.relay_domains` routes a
  host through trawl on every request. Set-Cookie on the solved response
  lands in the source's cookie jar, so later direct requests reuse the
  clearance.

## WASM sources (`crates/wasm`)

Aidoku source packages (`.aix`, zip) run in wasmer. One actor per source
(`runtime/actor.rs`) owns the store (`runtime/store.rs`), per-source rate
limiting (`runtime/rate_limit.rs`), and the host functions sources import
(`imports/`), including the HTML DOM helper (`html_dom.rs`) and the Boa
JavaScript engine (`js_engine.rs`) behind the `js` context functions — one
isolated context per ABI descriptor, time- and loop-limited, with no host
access beyond what the ABI exposes.
The `webview` ABI functions split in two (`imports/js.rs`, `webview.rs`).
The script-running subset is backed by the same Boa engine with one isolated
engine per webview descriptor, separate from `js` contexts:
`webview_load_html` resets the engine (a new document replaces the old
one's state), installs a `window = globalThis` and a `location.href` shim —
there is no DOM, so `document.*` is deliberately absent — and runs the
document's inline `<script>`s in order. External `src` scripts are skipped
(there is no document loader), non-JavaScript `type`s are not executed,
user scripts wrap the page scripts by injection time, and a failing script
is logged without failing the load or the scripts after it.
`webview_wait_for_load` is a synchronous no-op; `webview_eval` /
`webview_eval_async` behave like their `context_*` counterparts under the
same time and loop limits.
The navigation and cookie functions — `webview_load`,
`webview_get_cookies`, `webview_delete_cookie` — have two backends
(`browser.rs`, `browser/cdp.rs`). With `net.browser_ws` unset they return
`-1` (`JsError::MissingResult`; the ABI has no "unsupported" code) and log
the function name and reason, so a source relying on them sees the calling
operation fail with a diagnosable line in the source log. With
`net.browser_ws` pointing at a user-supplied Chromium CDP endpoint (e.g. a
browserless container; Sumidoku ships no browser) they drive that browser:
`webview_load` navigates a page and waits for the page load event — the
same completion signal as WKWebView's `didFinish` on iOS — and the cookie
functions read and edit the cookie store. Every webview rid maps to one
isolated browser context, created on first use and disposed when the rid
is destroyed, so sessions never bleed across webviews; a dead connection
is retried on the next call and the browser-side contexts it orphaned are
disposed after the reconnect. Cookies from the browser context flow into
the source's session jar after every load and on every `get_cookies`,
keyed by each cookie's own `Domain` attribute — that is what makes
webview login work: the source loads the login page, the login completes
in the browser, and the source's plain API requests then carry the
session cookie. `webview_load` only navigates GET requests (CDP has no
POST navigation; iOS would load the full URLRequest) and a non-GET
request fails with `-1` and a log line rather than silently degrading.
`webview_set_rule_list` (content blocking) stays explicitly unsupported
in both backends. An invalid
webview rid maps to `-4` (`InvalidHandler`), and webview and js-context
rids are not interchangeable.
`send_partial_result` payloads are decoded per in-flight call and pushed
to streaming callers (`SourceHandle::get_home_streaming` /
`get_search_manga_list_streaming`); home partials pass through the
upstream title-upsert accumulator (`partial.rs`) so consumers receive
index-addressed events, and an empty final layout falls back to the
accumulated partials. Calls without a streaming caller discard partials,
as before. The `canvas` ABI (`imports/canvas.rs`) is rendered by
tiny-skia: a context is a premultiplied-RGBA pixmap plus a current
transform, and images live in the same store as straight RGBA8 buffers
(`runtime/store.rs:ImageData`), converted at the boundary. `fill` /
`stroke` take postcard-encoded `aidoku::canvas::Path` / `StrokeStyle`
payloads, `set_transform` reassembles the source-side decomposition as
translate·scale·rotate (the ABI's radians become tiny-skia's degrees),
and `draw_image` / `copy_image` blit through a bilinear pattern fill
whose transform maps image space onto the destination rect. Text is laid
out glyph by glyph from `ttf-parser` outlines filled at the baseline
point — no shaping or kerning. Fonts come from a process-global `fontdb`
(`fonts.rs`): an embedded DejaVu Sans (`assets/`, Bitstream Vera license)
is the fallback that keeps Docker/headless hosts working, system font
directories are scanned once for coverage (notably CJK), and `load_font`
fetches over the source's own net policy into the same database;
`new_font` / `system_font` resolve to the embedded face when no match
exists. Canvas RIDs follow the normal `destroy` lifecycle, and the host
consumes them eagerly wherever a result must outlive the actor call:
`SourceHandle::take_page_image` encodes an image rid to PNG and drops it.
`source.rs`
loads and validates packages; `package.rs` reads the `.aix` zip layout
(`Payload/source.json` manifest, `Payload/main.wasm` payload, optional
`Payload/icon.png`) into memory without extracting. The server scans the
`sources/` directory under `config_dir` for `.aix` files and extracted
source directories alike; both register the same way, with `wasm_path`
pointing at the `.aix` or the directory's `main.wasm`. An `.aix` icon is
written next to the package as `<name>.png` so installed sources serve
`/sources/{id}/icon` the same way the directory scan's `icon.png` does.
Static package files (`source.json`, `settings.json`, `filters.json`) are
read by the same helper: directory sources from next to `main.wasm`, `.aix`
sources from `Payload/` inside the zip without extracting. Source settings
(`GET/PUT /api/v1/sources/{id}/settings`) come from the source's dynamic
`get_settings` export when present, otherwise from the static
`settings.json`; the static Aidoku schema (flat, camelCase) is normalized
to the nested shape the dynamic export produces (`api::settings`), so the
API and web UI see a single contract. `POST .../settings/reset` clears the
overrides and re-seeds package defaults; `POST .../login/basic` runs the
source's `handle_basic_login` and, on success, persists the credentials and
a `logged_in` marker into `source_setting`; `POST .../cache/clear` drops the
source's cookies and restarts the instance. Per-source state spans two
tables: `source_setting` (defaults and login credentials) and
`source_cookie` (cookies keyed `(source_id, domain, name)`). A running
instance's cookie jar reports changes through a `CookieSink` to a batch
writer, so a login session survives the restart that follows a settings
change.

`vendor/aidoku` is the vendored upstream `aidoku` crate with the `imports`
feature. It tracks upstream; local fixes don't belong there.

## Web UI (`webui`)

Vite 7 + React 19 SPA, TypeScript. `src/pages` are the routes,
`src/components` shared pieces, `src/api` the REST client,
`@tanstack/react-query` for server state. `npm run check` is
`tsc --noEmit`; there is no test suite. The build lands in `webui/build`
and the server embeds it (see above). When authentication is on, the
client (`api/client.ts`) sends the stored session token as a bearer
header, media/websocket URLs carry it as an `access_token` query
(`withToken`), and any `401` clears the token and lands on `/login`.

## Known gaps

### Source login: OAuth and web login

A source's `Login` setting (`api::settings`, from a dynamic `get_settings` or
the static `settings.json`) can declare `method` `basic` | `oauth` | `web`.
Only `basic` is implemented: `POST /api/v1/sources/{id}/login/basic` runs the
source's `handle_basic_login` (wired in `crates/wasm/src/source.rs`) and
persists `<key>.username` / `<key>.password` / the `logged_in` marker. The
other two methods are unhandled, and there is no logout endpoint.

The reference semantics are Aidoku's app layer (not the source SDK): for
`oauth` it builds the authorization URL from `url` or the `url_key` default,
optionally appends PKCE (`code_challenge` / `code_challenge_method=S256` /
`response_type=code`; verifier is 128 chars from the unreserved set and the
challenge is unpadded base64url SHA-256), opens a web-auth session with
`callback_scheme ?? "aidoku"`, then either POSTs
`grant_type=authorization_code&code&code_verifier[&redirect_uri][&client_id]`
(parameters not percent-encoded) to `token_url` and stores the raw response
string at `<key>`, or stores the callback URL string at `<key>` and fires the
setting's notification. For `web` it loads `url` in a per-source webview,
reads the host's `name→value` cookies, stores `<key>.keys` / `<key>.values`,
calls the source's `handle_web_login(key, cookies)` (the wasm export exists in
`vendor/aidoku`; no host call is wired), sets `<key>="logged_in"`, keeps
`localStorageKeys` under `<key>.ls.<lsKey>`, and clears the store on logout
when `clear_cookies_on_log_out` is set.

Blocker: a browser SPA cannot read a third-party login's redirect or cookies,
and the source's baked `redirect_uri` / custom `callback_scheme` cannot be
retargeted without provider registration. Implementing `oauth` therefore needs
a manual callback-URL step (or a hosted callback for the rare rewritable
redirect) plus a PKCE + token-exchange path; `web` needs a cookie
paste/import step. Neither has a natural automatic-browser path server-side.

When picked up, the pieces line up as: a new login module under
`crates/server/src/api/` (start/complete for OAuth, cookie intake for web,
and logout removing `<key>` and its `.username`/`.password`/`.keys`/`.values`/
`.codeVerifier`/`.ls.*` children), a `handle_web_login` host call in
`crates/wasm/src/source.rs` + `runtime/actor.rs`, and the `Login` branches in
`webui/src/pages/settings/SettingItem.tsx` (currently basic-only).

## CI/CD (`.github/workflows`)

- `ci.yml`: a paths-filter gates the legs — rust legs (clippy, nextest,
  compile on ubuntu + macOS) skip web-only changes; the web leg (tsc +
  build) skips rust-only ones. The docker leg always runs: it is the only
  check that exercises the embedded web UI, and it smoke-tests `/health`,
  the served `<title>`, and `/api/v1/settings`. The terminal `ci` job
  aggregates every leg (skipped counts as filtered-out, failure fails it);
  branch protection pins `changes` and `ci` because a skipped matrix leg
  never reports its expanded `build (os)` name.
- `release.yml`: a push to `main` touching `Cargo.toml` reads the
  workspace version and skips when that release already exists
  (`workflow_dispatch` is the manual fallback). Builds cross-compile the
  two linux targets and build both darwin targets on `macos-latest`, then
  push the same multi-arch image to Docker Hub (`sumidoku/sumidoku`) and
  GitHub Container Registry (`ghcr.io/sumidoku/sumidoku`), each tagged
  `latest`/`X.Y.Z`/`<major>.x` (e.g. `0.x`), and create the GitHub release
  with the four archives. The ghcr login uses the run's `GITHUB_TOKEN`, so
  the job asks for `packages: write`. The ghcr package starts private on
  first push and must be flipped to public in the org's package settings
  once — anonymous pulls fail until then.
