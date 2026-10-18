# Architecture

One page for the whole system, verified against the linked source. Update
the owning section in the same PR when a change alters a contract described
here. Link to symbols rather than copying details that drift.

## Server (`crates/server`)

axum 0.8, SQLite via sqlx. Entry `main.rs`; app wiring `lib.rs:build_app`.
Migrations in `crates/server/migrations`.

- REST API under `/api/v1` (`api/mod.rs`, `api/library.rs`, `api/reader.rs`,
  `api/page.rs`, `api/download.rs`, `api/update.rs`).
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
(`imports/`), including the HTML DOM helper (`html_dom.rs`). `source.rs`
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
  `latest`/`X.Y.Z`/`X.Y`, and create the GitHub release with the four
  archives. The ghcr login uses the run's `GITHUB_TOKEN`, so the job asks
  for `packages: write`.
