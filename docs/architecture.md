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
- Library updates (`update/mod.rs`): same scheduling shape as downloads —
  serial per source, at most 3 sources at once — broadcasting progress as
  `UpdateEvent`s. Category `include_in_update` is a three-state filter.
- Outbound requests pass a `NetPolicy` (`source/routed.rs:SumidokuNetPolicy`):
  direct, per-domain relay, or a global proxy, decided per host.

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
same time and loop limits. The functions that need a real browser —
`webview_load` (navigation), `webview_set_rule_list` (content blocking),
`webview_get_cookies` / `webview_delete_cookie` (cookie store) — return
`-1` (`JsError::MissingResult`; the ABI has no "unsupported" code) and log
the function name and reason, so a source relying on them sees the calling
operation fail with a diagnosable line in the source log. An invalid
webview rid maps to `-4` (`InvalidHandler`), and webview and js-context
rids are not interchangeable.
`send_partial_result` payloads are decoded per in-flight call and pushed
to streaming callers (`SourceHandle::get_home_streaming` /
`get_search_manga_list_streaming`); home partials pass through the
upstream title-upsert accumulator (`partial.rs`) so consumers receive
index-addressed events, and an empty final layout falls back to the
accumulated partials. Calls without a streaming caller discard partials,
as before. `source.rs`
loads and validates packages; `package.rs` reads the `.aix` zip layout
(`Payload/source.json` manifest, `Payload/main.wasm` payload) into memory
without extracting. The server scans the `sources/` directory under
`config_dir` for `.aix` files and extracted source directories alike;
both register the same way, with `wasm_path` pointing at the `.aix` or
the directory's `main.wasm`.

`vendor/aidoku` is the vendored upstream `aidoku` crate with the `imports`
feature. It tracks upstream; local fixes don't belong there.

## Web UI (`webui`)

Vite 7 + React 19 SPA, TypeScript. `src/pages` are the routes,
`src/components` shared pieces, `src/api` the REST client,
`@tanstack/react-query` for server state. `npm run check` is
`tsc --noEmit`; there is no test suite. The build lands in `webui/build`
and the server embeds it (see above).

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
