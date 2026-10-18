# Architecture

One page for the whole system, verified against the linked source. Update
the owning section in the same PR when a change alters a contract described
here. Link to symbols rather than copying details that drift.

## Server (`crates/server`)

axum 0.8, SQLite via sqlx. Entry `main.rs`; app wiring `lib.rs:build_app`.
Migrations in `crates/server/migrations`.

- REST API under `/api/v1` (`api.rs`, `download_api.rs`, `update_api.rs`).
- OPDS 1.2 catalog at `/opds/v1.2` (`opds.rs`); basic auth applies to the
  catalog only, and only when `SUMIDOKU_BASIC_PASSWORD` is set.
- The web UI is served from the binary: release builds embed `webui/build`
  (`webui_assets.rs`, rust-embed); debug builds read it from disk.
- Library model (`model.rs`, `library.rs`): manga → chapter → page rows;
  `source_order` is the source's own chapter index. Chapter reconcile
  (`service.rs:reconcile_chapters`) keeps a chapter the source stopped
  listing when it has download rows, and deletes it otherwise.
- Downloads (`download.rs`): serial within a source, parallel across
  sources, with a page-level fetch cap inside a chapter.
- Library updates (`update.rs`): same scheduling shape as downloads —
  serial per source, at most 3 sources at once — broadcasting progress as
  `UpdateEvent`s. Category `include_in_update` is a three-state filter.
- Outbound requests pass a `NetPolicy` (`routed.rs:SumidokuNetPolicy`):
  direct, per-domain relay, or a global proxy, decided per host.

## WASM sources (`crates/wasm`)

Aidoku source packages (`.aix`, zip) run in wasmer. One actor per source
(`actor.rs`) owns the store (`store.rs`), per-source rate limiting
(`rate_limit.rs`), and the host functions sources import (`imports/`),
including the HTML DOM helper (`html_dom.rs`). `source.rs` loads and
validates packages; the server scans `SUMIDOKU_SOURCES_DIR` for them.

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
  the served `<title>`, and `/api/v1/settings`.
- `release.yml`: a push to `main` touching `Cargo.toml` reads the
  workspace version and skips when that release already exists
  (`workflow_dispatch` is the manual fallback). Builds cross-compile the
  two linux targets and build both darwin targets on `macos-latest`, then
  push a multi-arch image to Docker Hub (`sumidoku/sumidoku` tags
  `latest`/`X.Y.Z`/`X.Y`) and create the GitHub release with the four
  archives.
