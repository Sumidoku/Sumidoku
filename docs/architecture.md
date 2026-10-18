# Architecture

One page for the whole system, verified against the linked source. Update
the owning section in the same PR when a change alters a contract described
here. Link to symbols rather than copying details that drift.

## Server (`crates/server`)

axum 0.8, SQLite via sqlx. Entry `main.rs`; app wiring `lib.rs:build_app`.
Migrations in `crates/server/migrations`.

- REST API under `/api/v1` (`api/mod.rs`, `api/library.rs`, `api/reader.rs`,
  `api/page.rs`, `api/download.rs`, `api/update.rs`, `api/repo.rs`).
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
- OPDS catalogs: 1.2 (Atom) at `/opds/v1.2` (`opds/mod.rs`) and 2.0 (Readium
  Web Publication JSON) at `/opds/v2` (`opds/v2.rs`). The 2.0 feed builds its
  URLs from the 1.2 cover/file/stream endpoints; the two share auth, paging
  and query helpers from `opds/mod.rs`. Basic auth applies to the catalogs
  only, and only when `SUMIDOKU_BASIC_PASSWORD` is set.
- The web UI is served from the binary: release builds embed `webui/build`
  (`webui_assets.rs`, rust-embed); debug builds read it from disk.
- Library model (`model.rs`, `api/library.rs`): manga → chapter → page rows;
  `source_order` is the source's own chapter index. Chapter reconcile
  (`source/mod.rs:reconcile_chapters`) keeps a chapter the source stopped
  listing when it has download rows, and deletes it otherwise.
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
and the server embeds it (see above).

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
