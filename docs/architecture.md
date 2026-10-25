# Architecture

One page for the whole system, verified against the linked source. Update
the owning section in the same PR when a change alters a contract described
here. Link to symbols rather than copying details that drift.

## Server (`crates/server`)

axum 0.8, SQLite via sqlx. Entry `main.rs`; app wiring `lib.rs:build_app`.
Migrations in `crates/server/migrations`. Applied migrations are immutable:
sqlx embeds each file's SHA-384 at compile time and validates it against
`_sqlx_migrations` at startup, aborting the boot on any byte change, so a
schema change is always a new numbered file — never an edit, comments
included (`db.rs:shipped_migrations_are_immutable` pins the shipped set).

- REST API under `/api/v1` (`api/mod.rs`, `api/library.rs`, `api/reader.rs`,
  `api/page.rs`, `api/download.rs`, `api/update.rs`, `api/repo.rs`,
  `api/backup.rs`).
  `GET /api/v1/sources/{id}/home` (`api/mod.rs:get_home`) returns the Aidoku
  `HomeLayout` as-is — an ordered `components` array; the array index is the
  stable key a `send_partial_result` stream patches against. A source
  without `get_home` answers 501 `unsupported`, same as dynamic filters and
  listings.
  `POST /api/v1/sources/{id}/deep-link` (`api/mod.rs:deep_link`) hands a URL
  to the source's `handle_deep_link`. `Manga`/`Chapter` results are resolved
  to internal ids before answering — the manga is found or registered through
  the same find-or-register path a browse-page click takes
  (`find_or_register_manga`), and the chapter id is looked up by key (null
  when the source's chapter list lacks it, the web UI then lands on the manga
  page) — so the web UI navigates to `/manga/:id` or `/reader/:chapterId`
  with no second lookup; `Listing` passes through and the source browse page
  switches to that listing, and an unmatched URL answers `{"result": null}`.
  A source without the export answers 501 `unsupported`, same as `get_home`.
  `GET /api/v1/sources/{id}/listings` merges the source's declared listings —
  static `source.json` entries first, then the dynamic `get_listings` export —
  into one shape (`api::listings`). Static entries parse with AidokuRunner's
  `SourceInfo` decoding semantics: a bare string means `id = name = string`, a
  missing `name` falls back to the id, and `kind` is `List` only for the
  numeric value 1 (AidokuRunner decodes `ListingKind` as UInt8, anything else
  lands on `Default`). Every listing answers `{id, name, kind}` with `kind`
  normalized to the lowercase string `default` | `list` (static kinds are
  numeric, dynamic exports serialize the enum name — the server is the single
  shape authority, so both land on one string). The web UI renders a listing
  whose kind is `list` as a row layout (small cover, title, subtitle, tags)
  and keeps the cover grid otherwise (`webui/src/pages/SourceBrowsePage.tsx`).
  Listings embedded in deep-link results and home components pass through as
  the source returned them; the web UI consumes those for navigation only.
  `GET /api/v1/sources/{id}/listings/{listing_id}` (`api/mod.rs:get_manga_list`)
  resolves the real `Listing` before calling `get_manga_list`: the id is looked
  up in the source's declared listings, merged in the same order and with the
  same static decode semantics as the listings endpoint above. An unknown id
  passes through with an empty name and `Default` kind — Aidoku only ever
  sends a listing the source itself returned, so there is no upstream
  lookup-failure behavior to mirror, and the source decides how to answer. A
  failed dynamic-listings fetch degrades to the static list rather than
  failing the request.
- API documentation (`api/docs.rs`, `assets/api-docs/`): `/api/openapi.json`
  publishes the hand-maintained OpenAPI 3.0.3 document that is the single REST
  contract for every `/api/v1` route; external OPDS feeds are excluded. `cd webui && npm run
  generate:api` uses only Node built-ins to regenerate the committed
  `webui/src/api/types.ts` from the schema graph reachable through ordinary
  JSON parameters, requests and responses. `npm run build` regenerates first,
  and CI repeats generation followed by `git diff --exit-code` on the generated
  file to reject stale output. The test-only validator in
  `crates/server/tests/common/contract.rs` checks real JSON responses in the
  shared request helpers against path/method/status/content schemas, including
  references, required properties, maps, arrays, enums, nullable fields,
  oneOf/allOf and x-tuple. SSE streams and NDJSON events stay outside
  generation and response validation; their manually maintained TypeScript
  protocols live in `webui/src/api/protocol.ts`. Binary content and external
  OPDS feeds are excluded from generation and response validation as well.
  `/api/docs` and `/api/docs/` serve Swagger UI 5.34.0 with its JavaScript,
  CSS, Apache-2.0 license and bundled dependency notices embedded in the
  binary. `validatorUrl: null` disables external spec validation, so the page
  needs no external assets. The route drift test reads `api/mod.rs` and
  compares the complete path/method set with the JSON; update the document in
  the same PR as route or payload changes. Both Docker smoke tests require the
  spec and docs page.
- Unified event stream (`events.rs`, `api/events.rs`): `GET /api/v1/events`
  is a same-origin SSE endpoint for downloads and library updates. The event
  hub subscribes to the existing download/update broadcasts and keeps the
  latest 1024 events in process memory, in id order. A new connection replays
  retained events after `Last-Event-ID`, sends a current download/update
  snapshot, then sends live events with duplicate ids suppressed. Ring
  overflow or a process restart simply makes the snapshot authoritative. A
  cursor beyond the current maximum id is reset to that snapshot. Snapshot
  query failures close the stream instead of presenting an empty download
  list as current. The event names are `snapshot`, `download`, `running`,
  `update`, `updateSummary` and `updateComplete`. `updateSummary` carries the
  start and per-item counts, while `updateComplete` carries the final stopped
  state, so the updates page does not poll. Update jobs and summaries reach
  the hub through one ordered channel. A lagged client connection ends so
  native `EventSource` reconnects; a lagged forwarding subscription instead
  asks each connection for a fresh snapshot. Heartbeat comments are sent every
  15 seconds. The browser keeps one SSE connection open across route changes;
  `AppLayout` starts it only when the auth state allows and stops it on logout,
  password change and unauthorized responses. The manga detail page also seeds
  active downloads with `GET /downloads` and merges pending events into that
  response; only a live snapshot replaces the seed. Browser API requests use the
  login cookie, CLI clients may use bearer authentication, and URL access
  tokens are not accepted. The former download and update WebSocket endpoints
  are removed.
- Background tasks (`task/mod.rs`, `api/task.rs`, migration `0023_task.sql`):
  the `task` table is the queue — a persistent row per task whose string id
  doubles as the deduplication key (e.g. `local_scan:3`). Enqueueing over a
  live (pending/running) row only bumps its priority — kind, source and
  payload stay with the execution that owns them — while a terminal row is
  requeued fresh with the new trigger's identity and priority (kmrs's
  `ON CONFLICT` upsert; manual 8 > scheduled 4 > derived 1). `TaskManager`
  dispatches pending rows to a worker pool sized by
  `task_pool_size` (config file/env `SUMIDOKU_TASK_POOL_SIZE`, default 2) in
  priority-then-creation order with id as the tie-breaker, keyset-paging past
  blocked groups and failed kinds without skipping later candidates; tasks
  of one local source never run concurrently, and a global (source-less)
  task conflicts with everything (kmrs `GROUP_ID` semantics over `source_id`,
  0 reserved for the global group). Every transition is a single guarded
  UPDATE whose RETURNING row is what gets broadcast as the instance-wide
  `task` event through the SSE hub (`events.rs:forward_tasks`), so events
  never republish a state a later transition replaced. Cancellation publishes
  before history trimming, so a terminal requeue cannot overtake it.
  `DELETE /api/v1/tasks/{id}` cancels: the row flips
  to cancelled atomically for pending and running alike, a running handler
  also gets a flag it checks between items, and the worker's terminal write
  is guarded on status = 'running' so a mid-flight cancel always wins; a
  finished task answers 409 `task_finished`. Boot recovery
  (`TaskManager::recover`, wired in `build_app`) requeues rows left running
  by the previous process, so handlers must tolerate re-execution. A kind
  with no registered handler (e.g. a downgrade over a newer database) fails
  visibly instead of sitting pending. Finished rows are trimmed to the
  latest 100 on every terminal transition. `GET /api/v1/tasks` lists live
  work first and accepts `status`/`kind`/`sourceId` filters. This change
  registers no production task kinds or producers yet; the local
  scan/analyze/empty_trash handlers land with #256.
- Partial-result streaming (`api/partial.rs`): the home, search and
  manga-update endpoints honor `Accept: application/x-ndjson` as an explicit
  opt-in. The source's mid-request `send_partial_result` payloads arrive as
  one JSON event per line in send order, then a terminal event closes the
  stream: home emits `{"type":"layout","layout":…}` (replace whole layout)
  and `{"type":"component","index":N,"component":…}` (replace at index, or
  append when `index` equals the current length — the index is resolved
  from the upstream title-upsert semantics by the wasm layer, see
  `crates/wasm/src/partial.rs`), search emits per-Manga deltas
  `{"type":"manga","manga":…}`. The manga-update paths — `POST /api/v1/manga`
  (register), `GET /api/v1/manga/{id}?refresh` and
  `GET /api/v1/manga/{id}/chapters?refresh` (`api/mod.rs`) — emit the same
  `{"type":"manga","manga":…}` event per partial: mirroring AidokuRunner's
  `partialMangaPublisher`, each one is the complete source-side `Manga` as it
  stood mid-fetch, and unlike home there is no accumulation — the returned
  manga stays authoritative. Register answers a buffered `{"id":…}` when the
  manga is already registered (there is no source call to stream); the
  background library update (`update/mod.rs`) and migration keep the
  buffered call, since no client waits on them. All streams end with
  `{"type":"final","payload":…}` where the payload is the same JSON value
  as the buffered response, or `{"type":"error","error":{code,message}}`
  when the source call fails after streaming started. Requests without
  the header take the original buffered path unchanged. The connection is
  the key: a client disconnect drops the body stream, later sends fail and
  the forward task stops while the source call runs to completion.
- Cross-source aggregate search (`api/search.rs`): `POST /api/v1/search`
  takes `{query?, page?, sourceIds: number[]}` and runs one search per
  source concurrently — per-request fan-out capped at 4, with the shared
  call semaphore (`state.rs`) still gating source calls globally. Results
  come back in request order as `{"sources": [{sourceId, entries,
  hasNextPage, error?}]}`, entries tagged exactly like the single-source
  search (owning `sourceId`, plus the internal `id` when registered). The
  built-in local source answers from the same directory-scan arm as the
  single-source endpoint (see the local-source bullet). A
  failing source — unknown id, disabled, or a failed source call — only
  marks its own entry (`entries: []`, `hasNextPage: false`,
  `error: {code, message}`) and never fails the request. With
  `Accept: application/x-ndjson` the endpoint reuses the same stream
  plumbing: one `{"type":"source","source":…}` event per finished source
  and one `{"type":"sourceError","sourceId":…,"error":…}` per failed one,
  in completion order, then a final event whose payload is the same JSON
  value as the buffered response. The per-source failure type is
  `sourceError`, not `error`, because the streaming client treats
  `{"type":"error"}` as the terminal failure of the whole request. The
  browse page's search view (`webui/src/pages/browse/SearchPanel.tsx`) renders
  each arriving section as a horizontal cover scroller and offers tri-state
  (include/exclude) content-rating / language / source filters persisted
  to localStorage — filtering is client-side and only decides which
  source ids are sent.
- Source repositories (`source/repo.rs`, `api/repo.rs`): Aidoku-style repo
  indexes are fetched on add/refresh and cached in the `source_repo` table;
  browsing, install and update detection all read the cache, so an
  unreachable repo never breaks the installed-source list. Refreshing is the
  only way the cache moves: the API's manual per-repo refresh, plus an
  optional periodic sweep of every repo when `repo_refresh_interval_hours`
  (config file/env `SUMIDOKU_REPO_REFRESH_INTERVAL_HOURS`, default 0 = off)
  is non-zero; a failed refresh keeps the old cache and records
  `fetch_error`. Install downloads
  the `.aix` (http/https only, 64 MiB cap) into `sources/` and registers it
  through the same `register_aix` path as the directory scan; versions
  compare as Aidoku's monotonic integers. Install also stamps
  `source.repo_id` with the installing repo (directory-scan sideloads stay
  NULL), and `repo::source_json` reports `missingFromRepos` when a
  repo-installed source's key is absent from every cached index — Aidoku's
  "missing source list" warning, the deprecation signal for external sources.
- Local `.aix` import (`api/source_import.rs`): `POST /api/v1/sources/import`
  (admin, multipart with a single file field) is the third install path, for
  sideloading a package file through the web UI. The route's body cap derives
  from the package entry caps in `crates/wasm/src/package.rs` (wasm payload +
  JSON metadata + icon, plus framing slack); an over-limit upload is a 413.
  The upload's manifest is read in memory first — its key names the landed
  file — then the bytes land atomically in `sources/` (temp name + rename,
  the repo-install pattern) and register through the same `register_aix`
  pipeline, so manifest parsing, settings seeding, feature stamping and icon
  extraction behave identically to the other two paths; a registration
  failure cleans up the landed file and the icon sidecar. A same-key import
  replaces the installed source with repo-update semantics: the response is
  `source_json` plus `migrationRequired`/`migrationWarnings` like
  `update_source`, and `source.repo_id` is left untouched (NULL on a fresh
  import, same as a directory sideload).
- Breaking-change key migration (`source/rekey.rs`): registration stores the
  manifest's `config.breakingChangeVersion` (Aidoku's
  `SourceInfo.Configuration`) in `source.breaking_change_version`, and
  `source_json` exposes it. Registration also persists the whole manifest
  `config` section verbatim in `source.config` (null when absent), and
  `repo::source_json` exposes it as `config` in the installed-source DTO.
  Registration also serializes the spawned instance's `SourceFeatures`
  into `source.features`; the DTO exposes `hasHome` from it. Rows installed
  before the column existed start NULL and are backfilled once at startup
  (`source::backfill_source_metadata`): each package's manifest re-stamps
  a NULL `config` and a stale `breaking_change_version` (MAX, never
  regressing), and its module is compiled without instantiating to
  re-stamp a NULL `features` (COALESCE keeps any live value). Rows whose
  package fails to read or compile stay NULL, and the web UI keeps the
  home entry visible on unknown rather than hiding it.
  The web UI honors `hidesFiltersWhileSearching` on the source browse page
  (a non-empty search box hides the filter UI and the search request drops
  filter values, mirroring AidokuRunner clearing filters on a non-empty
  query). The manga detail page follows Aidoku's declared-first,
  derived-fallback tap-to-search (`config?.supportsX ?? …` in
  AidokuRunner): manifest `supportsTagSearch` submits a `Select` with id
  `genre` and the raw tag; when undeclared, a tag is clickable only on an
  exact, case-sensitive option-label match against an `is_genre`
  multi-select or select (submitted value `ids[i] ?? options[i]`), and a
  creator name is clickable when the served filter list (`GET
  /sources/{id}/filters`) has a text filter with id `author`/`artist`,
  submitted as a `Text` filter value — Aidoku's query-less filter search
  in both cases.
  `maximumParallelRequests` becomes a per-source call semaphore (see
  `state.rs` below). When an update crosses it
  (`old version < breakingChangeVersion <= new version`, Aidoku checks only
  the lower bound) and the source has manga rows, the update path runs key
  migration after the new package registers. With `features.migration` the
  flow mirrors Aidoku's `AppDelegate.handleSourceMigration`: every manga and
  chapter key is remapped through `handle_key_migration`
  (`system.startMigration`/`endMigration` notifications when the source has
  the notification feature, chapter calls pass the old manga key), mappings
  are collected outside the transaction and then applied in one transaction
  that re-keys rows in place — ids stay, so progress, categories and
  download flags ride along. Keys the source maps to an empty string, fails
  on, or leaves unchanged are skipped (empty/failed ones land in
  `warnings`); a new key that collides with `UNIQUE(source_id, key)` /
  `UNIQUE(manga_id, key)` is retried to a fixpoint (the occupying row may
  itself move) and otherwise keeps its old key with a warning — Aidoku does
  no collision handling at all (a failed Core Data save is only logged), so
  this is deliberately stricter. Without `features.migration` nothing is
  re-keyed; the update responses flag `migrationRequired` (only when the
  source has in-library rows, Aidoku's `forceMigrate` condition) and the web
  UI routes to `/migrate?source=<id>&sameSource=1`, the cross-source wizard
  prefilled with the same source as destination. `POST /sources/{id}/update`
  returns `migrationRequired`/`migrationWarnings` next to the source DTO;
  `POST /source-repos/update-all` groups them per source key. To serve that
  fallback, `POST /api/v1/migrate` accepts same-source entries when the
  target key differs from the origin's (identical `(source, key)` still
  reports `same_source`); the new-key row is fetched or reused and progress
  carries by chapter number as usual.
- Built-in local sources (`local/mod.rs`, migration `0011_source_kind.sql`):
  `source.kind` is `aidoku` for every installed package and `local` for local
  instances; `local.local` is the seeded default instance. Registration goes
  through the shared internal-source endpoint `POST /api/v1/sources/internal`
  (`api/internal_source.rs`, a tagged-union dispatch — a future built-in kind is
  one more variant): `{type: "local", name, paths, oneshotsDir?}` validates
  every path (must exist and is a directory), derives the key as `local.<slug>`
  with a `-2` collision counter (the opds rule), and never pins — the Browse
  page groups internal sources into their own Internal sources section, with
  pinning an opt-in on top. Every local instance is deletable through the
  ordinary surface — except that deleting `local.local` answers 400 while the
  deployment config explicitly names local roots (`SUMIDOKU_LOCAL_DIRS` / file),
  because boot (`local::ensure_default_instance`, wired in `build_app`) would
  otherwise recreate it empty. Every wasm-capability endpoint (filters,
  listings, home, deep link, login) answers 501 `unsupported` for local rows.
  There is no index table: browsing walks the instance's roots recursively in
  real time — the instance's own `paths` source setting wins; the default
  instance additionally falls back to the legacy global `local.dirs` setting
  and then the config layer (config `local_dirs` (`SUMIDOKU_LOCAL_DIRS`,
  semicolon-separated with empty segments ignored; the single
  `SUMIDOKU_LOCAL_DIR` / `local_dir` spelling is a one-item fallback, default
  `[<config_dir>/local]`); `state.rs:local_dirs` resolves the layers, and a
  wrong-shaped stored value counts as unset) — `POST
  /sources/{id}/search` (`api/mod.rs:search_source`) branches for
  `kind = local` to `scan_manga_dirs`, which discovers manga top-down
  with nearest-qualifying-ancestor absorption: a directory directly
  holding chapter archives qualifies (the Komga trigger), one with no
  archive anywhere below qualifies by the backward-compatible relaxed
  rule (nested levels included), anything else is a pure container and
  discovery descends; a claimed manga absorbs its subtree, so nested
  series never appear as manga of their own and a flat single-level
  library scans byte-identically to the pre-nesting behavior. A
  configured oneshots name (the instance's `oneshotsDir` source setting —
  an explicit blank turns the rule off, an absent setting falls back for
  the default instance to the legacy global `local.oneshots_dir`, then to
  config `local_oneshots_dir` / `SUMIDOKU_LOCAL_ONESHOTS_DIR`) outranks
  both passes: a
  directory whose relative path contains it (case-insensitive
  substring, kmrs's `ScanOptions::oneshots_dir`) is never claimed —
  each archive directly inside becomes its own one-chapter manga keyed
  by the archive path (`oneshots/Foo.cbz`) and titled by the file
  stem, nothing below the directory is picked up, and the ComicInfo.xml
  / `series.json` / `cover.*` next to an archive are ignored as series
  sidecars (archive-internal ComicInfo/OPF metadata still applies, so
  the file stem is only the fallback title). Such
  a key names a file, not a directory: chapter listing, page reads and
  the cover chain treat it as the degenerate form whose only chapter
  is the archive itself (chapter key = the leaf name, the same shape
  as a top-level archive chapter), so the cover resolves to that
  chapter's first page. Keys are
  paths relative to their root (validated: no `..`, empty segments,
  absolute or drive-prefixed forms) carrying the root index: a bare
  path addresses root 0 (the pre-multi-dir form, so rows persisted
  before multiple roots keep resolving) and `N:<path>` addresses root
  N ≥ 1; `0:` is rejected so a root-0 manga has exactly one key form,
  and an out-of-range index answers not found. Browsing unions the
  roots and deduplicates by relative path: the same path present in
  several roots lists once, from the lowest-indexed root (first
  directory wins), while shadowed copies stay addressable by their
  numbered keys. Titles are the leaf segment of the relative path; the
  case-insensitive substring filter matches the title (Suwayomi's
  `contains(query, ignoreCase = true)`), the internal id is attached
  when the manga is registered, and `hasNextPage` is always false; the
  response is always buffered (there is no source call to stream), and
  the aggregate search reuses the same arm so a selected Local yields a
  section instead of an error entry. A manga persists only when opened
  or shelved (Suwayomi semantics): `find_or_register_manga` scans the
  directory, resolves metadata by a first-hit-wins chain (no per-field
  merge) — top-level `ComicInfo.xml` (authoritative when present, even
  unparsable) → the first chapter archive carrying one, in chapter
  order (extracted in-memory) → a `series.json` sidecar (flat schema or
  Mylar's nested `metadata`: title, role-mapped creators, description,
  combined genres/tags, status, age rating and `readingDirection`; a broken
  file warns and falls through) → the first EPUB chapter whose OPF parses
  (EPUB 3 `belongs-to-collection`, then OPF 2 `calibre:series`/`series`, then
  the first `dc:title`; role-aware creators, the first HTML-cleaned
  `dc:description`, normalized/deduplicated `dc:subject` and spine
  `page-progression-direction`; no status counterpart). JSON arrays and OPF
  author/artist/tag lists stay structured, so their commas are literal; flat
  series.json scalars and ComicInfo.xml fields use comma splitting. Mylar
  `readingDirection` and OPF
  spine direction set `Manga.viewer`. OPF `series_index` and language stay
  unmapped rather than leaking the first EPUB's book metadata into every
  chapter. The result feeds the same
  `upsert_manga` + `reconcile_chapters` as an online fetch; the scan
  skips dot entries, `@eaDir` and unreadable items at every level
  (failed items land in the reconcile protection set by full relative
  path, a failed container prefix-protecting its unwalked subtree) and
  dedupes by key. Chapters are `.cbz`/`.zip`/`.epub` archives and
  image-leaf
  directories (direct image files) at any depth under the manga, keyed
  by their path relative to the manga directory, ordered by the number
  parsed from the name (`第?N话` / `ch.?\s*N` / `vol.?\s*N`) descending
  with a natural-order tiebreak, unnumbered ones last; pages are a
  chapter's own direct image files, sorted case-insensitive naturally.
  A divina-profile EPUB (image-based books; reflowable text is out of
  scope) instead takes its page list from direct image spine items or
  XHTML/SVG pages that each resolve to exactly one image (`local/epub.rs`:
  the `mimetype` entry must be `application/epub+zip`, and the package
  document is located through `META-INF/container.xml`). Manifest, spine
  and zip entries must agree, body text must stay within 15 normalized
  UTF-16 units per page, and invalid or non-divina packages yield no
  pages. A page's mime is the OPF-declared media type with the extension
  mapping as fallback. Migration 0021 clears cached local EPUB page rows
  and counts from older parsers while preserving reading progress; the
  next open rebuilds them under these rules. Migration 0022 adds
  `chapter.epub_divina_compatible`: true for divina, false for other
  EPUBs, and NULL for non-EPUB or unknown. Local scans and page rebuilds
  refresh the value.
  `load_pages`
  (`source/mod.rs`) lists the chapter from the filesystem for local manga
  and stores `{"Local": entry}` page rows; the reader (`api/page.rs`)
  reads image files directly and extracts single archive entries
  (`by_name`) through a version-checked 16-handle `ZipArchive` LRU
  (`local/zip_cache.rs`), so replaced archives are reopened. Page rows,
  counts, and compatibility are committed together. The downsample pipeline
  is untouched. Covers reuse the disk cache with
  the byte source switched to the fallback chain (`local::resolve_cover`):
  `cover.*` sidecar in the manga directory → first page of the chapter
  list's last chapter (Suwayomi's `chapters.last()`, the lowest-numbered
  one) → 404 placeholder.    `?refresh=1` on the chapters endpoint re-scans;
  the library update scheduler treats the local scan as the source call.
  Auto-download skips local manga (files are already local), and the web
  UI hides the download buttons for `kind = local`, shows a manual refresh
  button on the chapter list, and edits each instance's roots and oneshots
  marker in its source settings: `GET/PUT /api/v1/sources/{id}/settings`
  answer a server-synthesized definition for local rows (`paths` as an
  unknown-to-Aidoku `list` type plus a `oneshotsDir` text) with the
  effective, fallback-resolved values; PUT validates `paths` against the
  filesystem and rejects unknown keys, `oneshotsDir: null` clears the
  override, and reset clears only the oneshots override (the roots have no
  default). The retired global `local` settings group is rejected on write
  (400); the legacy flat `local.dirs` / `local.oneshots_dir` keys remain
  accepted and keep working as the default instance's fallback layer. RAR/CBR, PDF and 7z are
  out of scope (no mature pure-Rust
  readers, or an external runtime library).
- Built-in OPDS source (`opds_source/mod.rs`): `source.kind = opds` turns an
  OPDS catalog — Komga, Kavita, Calibre-Web — into a first-class source,
  multi-instance from day one. Both wire formats are accepted, detected per
  response by Content-Type with a JSON body sniff for mislabeled servers: 1.2
  (Atom) through `parse_feed`, 2.0 (JSON) through `parse_feed_json`, both
  filling the same `Feed` model. Registration shares the internal-source
  endpoint: `POST /api/v1/sources/internal` with `{type: "opds", name, url,
  username?, password?}` takes
  name + catalog URL + optional basic credentials; keys are `opds.<slug>`
  with a counter on collision (`opds.komga`, `opds.komga-2`), credentials
  land in `source_setting` (`username`/`password`), instances are
  deletable (cascade) and excluded from repo management and metadata
  backfill (`kind = 'aidoku'` scope), and every wasm-capability endpoint
  answers 501 `unsupported` exactly like the local source. Capability
  mapping: search = the catalog's OpenSearch descriptor (`rel="search"`
  link on the root feed → `{searchTerms}` template) or, on 2.0 catalogs,
  the templated `rel="search"` link itself (`{?query}` expanded directly,
  no descriptor), falling back to a
  bounded BFS traversal (depth 2, 12 feeds) when the descriptor is absent
  and flagging the response `degraded`; manga = a drill-down entry (Atom
  `<id>` is the key, its feed link is `manga.url` — the only re-fetch
  handle); chapters = the series feed's acquisition entries, paginated
  feeds followed to completion (cap 20); pages = the entry's OPDS-PSE
  stream link expanded into `Url` pages (`pse:count`, 0-based page
  numbers — Komga's `books/{id}/pages/{n}` needs no archive download)
  or, on 2.0, the publication manifest's `readingOrder` enumerated into
  the same per-page `Url` rows (the book entry's `self` link carries the
  manifest, and it wins over the acquisition archive when both exist),
  else the acquisition archive is downloaded once to
  `config_dir/opds-cbz/<chapter_id>` and image entries are served through
  the shared `ZipCache` LRU as an `OpdsZip` page-content variant
  (invalidated on page-row rewrite, self-healing after cache loss);
  covers = the first book entry's `image`/`thumbnail` link through the
  existing disk cover cache. All catalog traffic goes out with the
  instance's basic credentials through `http_get`/`fetch_page_image`/
  `fetch_cover_image` opds arms; a 401 surfaces as `bad_gateway` with the
  status in the message. Search, register, refresh, the background
  updater, aggregate search and migrate all branch to the catalog client
  the same way they branch for `kind = local`, so migrate between an
  OPDS instance and any other source works unchanged. The Browse page
  gets an admin-only internal-source dialog (`BrowsePage.tsx`
  `InternalSourceModal`, a kind picker over the registration dispatch) and
  hides the home/settings buttons on opds cards.
  The 2.0 mapping (`parse_feed_json`): navigation items become drill
  entries (href doubles as the key), publications become entries
  (`metadata.identifier/title/author/modified/description` onto the 1.2
  fields; a feed-typed `self` link is the series drill-down, any other
  JSON `self` link the book's manifest; `images[0]` is the cover, the
  rest thumbnails), and groups are flattened — their navigation and
  publications join the same entry list — so series feeds and search
  results look identical to 1.2 downstream.
- Pinned sources (`api/pins.rs`): an ordered list of source keys in the
  global `setting` table under `browse.pinned_sources` — Aidoku's
  `Browse.pinnedList` analogue, no schema migration. The array order is the
  pin order and seeds the destination priority of cross-source migration.
  `GET /api/v1/sources` items carry `pinned` and `pinOrder` (index in that
  list, null when unpinned) via `repo::source_json`.
  `PATCH /api/v1/sources/{id}` takes `{pinned?: bool, enabled?: bool}`;
  pinned pins (appended to the tail) or unpins (removed), and enabled
  toggles whether the source can be instantiated. Disabling a source updates
  the row and evicts any running instance so the next capability call observes
  the new state; enabling leaves the row as enabled and lets the next access
  respawn the instance. `POST /api/v1/sources/reorder-pinned` `{ids: [...]}`
  replaces the order wholesale and answers 400 unless ids are exactly the
  current pinned set, so a reorder never silently drops a pin.
  `delete_source` prunes the key. The Browse page shows the pinned sources
  in an ordered section above the full list, with drag-to-reorder
  (`webui/src/pages/BrowsePage.tsx`).
- Cross-source migration (`api/migrate.rs`): moves the requesting user's
  library entries between
  sources, Aidoku-style. `GET /api/v1/migrate/sources` lists installed
  sources with their in-library `mangaCount` (plus pin data from
  `source_json`); `GET /api/v1/migrate/lookup?mangaId=&targetSourceId=[&query=]`
  runs a title search on one destination source through the source's normal
  search path (rate limits intact) and returns the candidates with internal
  ids attached — the batch flow's auto-match calls it per destination in
  priority order and takes the first source with any hit (its first
  result), the single flow calls it across all destinations 3 at a time.
  `POST /api/v1/migrate` `{mode, entries: [{fromMangaId, toSourceId,
  toMangaKey, toManga?}]}` executes serially: each entry is one SQLite
  transaction (the `get_manga_update` fetch that initializes a new target's
  metadata and chapters happens before the transaction), so a failed entry
  never rolls back the ones already committed and reports
  `{ok: false, reason}` (`not_found`, `not_in_library`, `source_not_found`,
  `same_source`, `fetch_failed`) next to the successful
  `{ok: true, toMangaId, warnings}`. An existing target row is reused and
  merged into. Carried state: library membership (target keeps its own
  `in_library_at` when already in library, else inherits the origin's),
  category membership (union), and reading progress matched by chapter
  `number` (duplicates resolve to the lowest `source_order`; numberless
  origin chapters fall back to `volume` but only match numberless target
  chapters), merged with max(`last_page_read`), OR-ed `is_read`/
  `is_bookmarked` and the newest `last_read_at`; unmatched chapters land in
  the entry's `warnings`. Modes: `copy` keeps the origin untouched,
  `migrate` removes it from the library and resets its progress fields (the
  row stays for the download flag), `migrateWithHistory` removes it from
  the library but keeps its progress rows. The web UI has the batch
  source-first wizard at `/migrate` (`webui/src/pages/migrate/`) and the
  single-manga flow in the detail page's more-menu
  (`webui/src/pages/manga/MigrateModal.tsx`).
- Authentication (`auth.rs`): only the built-in admin (`USER_ID = 1`) can
  authenticate; its login identity lives in the `user` table. Login is
  username + password (`POST /api/v1/auth/login`), answered with an admin
  session token (`auth_token.user_id`, SHA-256 at rest, 30-day rolling expiry)
  and the `{id, username}` identity; there is no role field or user management
  API. The login username defaults to `admin` and can be set in config
  (`auth.username` / `SUMIDOKU_AUTH_USERNAME`), re-seeded onto the user row on
  every boot; `/api/v1/auth/status` exposes it as a top-level `username` only
  with a valid authenticated identity and omits it for anonymous requests.
  Auth is off until the built-in admin has a password, seeded from config on
  every boot or stored by a previous password change. With auth off every
  request acts as the same admin. `require_auth` is the only route gate:
  bearer credentials resolve against UI-managed keys (SHA-256 fingerprints in
  the `api_key` table, created and revoked through `/api/v1/auth/api-keys`,
  plaintext returned once at creation) or an admin session, and handlers
  continue to bind the legacy `AuthUser.id` storage key (always 1). Password
  changes revoke every admin session, while the config-set password is
  re-seeded at boot.
- Single-owner migration (`0019_single_owner.sql`): non-admin settings and
  sync origins are deleted explicitly, then non-admin users and their
  user-owned rows are removed through existing foreign-key cascades. The
  built-in row is forced back to the admin role, while admin data and sessions
  remain. Legacy `user_id` columns and `user.1.*` settings keys remain only as
  storage details; there are no additional accounts, roles or role-dependent
  product branches. Before those deletions, the migration rehomes each latest
  admin-owned tombstone that has a non-admin origin and rewrites its identity
  to the admin. A one-time `sync.single_owner_rebaseline` marker then appends
  current admin rows and latest tombstones as local events, preserving the
  existing sequence history. Every appended version is strictly newer than
  that entity's prior events, so preserved state also wins equal-time conflicts
  on existing peers. Live row versions and snapshot timestamps move together;
  the marker is cleared in the same transaction and otherwise retried on the
  next boot. Ordinary compaction is unchanged. Incoming sync changes with any
  other `origin_user_id` are rejected so old peers cannot reintroduce reader
  data.
- The web UI is served from the binary: release builds embed `webui/build`
  (`webui_assets.rs`, rust-embed); debug builds read it from disk.
- Library model (`model.rs`, `api/library.rs`): manga → chapter → page rows
  hold shared content; owner state lives in `manga_user` (library
  membership, viewer override, incognito, sort key) and `chapter_user`
  (progress, read/bookmark/download flags, history stamp). Their legacy
  composite keys include `user_id`, which is always 1, and every REST query
  and write binds that same admin identity. `source_order` is the source's own chapter
  index.   `GET /api/v1/manga/{id}/chapters` orders by
  `sort=source|chapter|date` (the source's listing order, chapter number and
  upload date respectively) and `order=asc|desc` — absent keeps the key's
  default: source stays ascending while chapter and date default to
  newest-first; invalid values answer 400. Numberless chapters count lowest
  and missing dates sink to the bottom of the default date order.
  `GET /api/v1/library`
  filters by `search`, `categoryId`, `filter`/`filterExclude` (comma-separated
  boolean flags `unread`/`downloaded`/`bookmarked`, EXISTS / NOT EXISTS over
  chapter flags) and the tri-state pairs
  `contentRating`/`contentRatingExclude` (comma-separated manga-level
  ratings 1..=3, `IN` / `NOT IN` on `manga.content_rating`),
  `source`/`sourceExclude` (source ids, `IN` / `NOT IN` on
  `manga.source_id`) and
  `category`/`categoryExclude` (comma-separated category ids, EXISTS / NOT
  EXISTS over the user's `category_manga` rows; id `0` is the
  uncategorized arm — include means no category rows, exclude means at
  least one): within one dimension the include set is an OR and the
  exclude set a NOT-OR, dimensions combine with the other filters via AND
  (Aidoku's library filter semantics). Empty or whitespace-only values are
  treated as absent; invalid tokens answer 400. The entry payload's
  `unreadCount`, `sort=unread` and the ad-hoc `filter=unread` all count a
  chapter with no read `chapter_user` row as unread, including a missing row.
  `sort=latestChapter` orders by each title's maximum chapter `fetched_at`
  (the chapter's first-seen time), descending with `manga.id ASC` ties;
  titles without chapters sort last.
- Sync change foundation (`sync.rs`, migrations `0017_sync_change_tracking.sql`
  and `0018_sync_protocol.sql`): local `user_id` and accepted
  `origin_user_id` values are always 1; user references below are legacy
  protocol fields for the admin, not additional accounts. Tracked rows have an internal
  `last_modified_at` integer Unix-millisecond timestamp, default `0` for
  pre-tracking history. Only sync-relevant value changes stamp it and append a
  `sync_change` event: identical upserts, ignored inserts and timestamp-only
  updates stay quiet, as do the manga fetch/scheduler bookkeeping columns and
  chapter `fetched_at`. Every non-delete event has
  `modified_at == snapshot.lastModifiedAt`; a delete event instead carries the
  deletion time while its snapshot retains the deleted row's previous version.
  Ordinary updates and deletes use `max(now, old + 1)`; a key- or
  owner-changing UPDATE also includes the NEW key's historical maximum + 1 in
  the NEW user's namespace. An ordinary insert with timestamp zero uses
  `max(now, the entity key's historical maximum + 1)`, so recreating a deleted
  key cannot move its version backwards. Explicit nonzero
  insert timestamps and changed nonzero update timestamps are preserved for
  sync apply. Only a changed nonzero UPDATE value is treated as explicit: a
  value equal to OLD is indistinguishable from leaving the column untouched
  and follows the ordinary increment rule. The journal has no foreign keys to
  tracked entities and belongs to users through `user_id ON DELETE CASCADE`;
  deleting a user removes only that user's events. Its `AUTOINCREMENT`
  sequence, not a wall-clock time, is the delta cursor. Migration 0017 seeds an
  ordered baseline of parent manga, user state, categories and memberships at
  modification time `0`; manga without user state have no per-user baseline.
  A new user row journals its parent manga first, and user-state or membership
  changes also journal a parent-manga signal for that user. These signals only
  allocate a fresh sequence for a composite refresh and keep the shared parent
  version; child entities carry their own versions.
  Migration 0018 adds `origin`, `origin_user_id`, `origin_entity_key` and
  `origin_sequence`. Existing rows are backfilled from the local instance,
  user, mapped entity key and sequence. `setting.sync.instance_id` is seeded
  once with 16 random bytes encoded as hex, and the journal insert trigger
  fills local origin metadata only when `origin_sequence = 0`. `entity_key`
  remains the local mapped key; the origin fields retain the original event
  identity. `sync_alias` stores remote-to-local entity mappings separately from
  event versions, so stale events and compaction cannot change the current
  version's origin. `sync_origin_state` records the greatest applied origin
  sequence per origin/origin user/local user/kind/original key, while the
  singleton `sync_checkpoint` starts at base sequence zero. `changes_since` reads one
  SQLite snapshot, uses `limit + 1`, and returns the last returned sequence
  while more of the user's events exist, otherwise the global sequence
  high-water mark, including other users' sequences; a cursor ahead of that
  mark is rejected.
  Chapter and progress baselines are zero-version and ordered parent first.
  Their audience is the union of users with `manga_user` and `chapter_user`;
  history-only users receive parent manga/chapter events without gaining
  library membership. Chapter changes emit a parent-manga signal, and progress
  changes emit parent manga then parent chapter signals before their own event.
  `sync_chapter_state` keeps the chapter's local ids, source/manga/chapter
  natural keys and content snapshot. Chapter snapshots exclude `fetched_at`;
  progress tracks only page/read/bookmark/history values, not `is_downloaded`,
  download queue rows, files or page blobs. `chapter_user.last_read_at` remains
  a reading-history timestamp in seconds, not the millisecond modification
  version. Rekeying keeps the local entity key and refreshes natural identity.
  Source, manga and chapter deletion capture chapter/progress snapshots before
  the parent rows disappear, so their tombstones retain natural identity and
  the cascades do not duplicate them. REST and backup-v1 shapes are unchanged.
- Direct instance sync (`api/sync.rs`, `sync/apply.rs`, `sync/compact.rs`):
  `GET /api/v1/sync/changes?origin=<peer>&since=<cursor>&limit=<1..500>` is
  authenticated; the peer origin is required, `since` defaults to zero, and
  the default limit of 100 is capped at 500. One SQLite snapshot returns only
  the authenticated user's rows after the local cursor whose original origin
  is not the peer. The response is `{origin, userId, nextCursor, hasMore,
  changes}` with the local instance id in `origin`. Every change has `origin`,
  `originUserId`, `originSequence`, `entityKind`, `entityKey`, `deleted`,
  `modifiedAt`, `identity` and `snapshot`; wire `entityKey` is
  `origin_entity_key`, never the local mapped id. Local sequences are only
  delta cursors, while origin and origin sequence identify the original event.
  `POST /api/v1/sync/changes` accepts `{origin, changes}` in the same shape.
  The envelope sender must not be the local instance; individual events may
  retain a third-party origin. The whole batch is validated before writing,
  then each event gets its own `BEGIN IMMEDIATE` transaction and an
  `{originSequence, status}` result of `applied`, `duplicate`, `stale` or
  `skipped` (`reason = source_unavailable`). Exact repeats and older origin
  sequences are rejected by `sync_origin_state`; applied events, legitimate
  no-ops and stale events advance it, but an unavailable source rolls the event
  back without advancing so installing the source and resending works. A stale
  event leaves the row unchanged but records its origin mapping so a later
  rekey still updates the same local entity. `sync_alias` owns these
  remote-to-local mappings independently of event versions; local events also
  register their own origin so a relay can sync back to the source instance.
  First matches use source/manga/chapter natural keys and category names; a
  numeric category id alone never proves a match. Category snapshots always
  carry numeric `includeInDownload: 0` for older peers that require the field.
  Apply accepts legacy values but normalizes the journaled snapshot to `0` and
  never writes the retired column; compaction likewise rewrites reused live and
  tombstone snapshots to `0`, so legacy history cannot reintroduce old values
  into a new baseline. Category membership identities
  carry `categoryOrigin`, `categoryOriginUserId` and `categoryOriginEntityKey`
  from their current parent event, allowing parent resolution across relay
  origins in either arrival order. Missing parents become zero-version stubs
  without fake events or library membership; a later authoritative parent fills
  them. Ordinary
  entities compare `(modified_at, origin, origin_sequence)`: newer tombstones
  delete, newer updates recreate, and deleting an absent entity only advances
  dedupe. Category membership versions consider the current local key and every
  historical key linked by `sync_alias`, so a tombstone left by stub merging
  still orders later relayed updates. Stub coalescing applies each relation's
  winning live/deleted state; if the winning physical row has a higher version
  than its latest event, a local event records that version explicitly. Progress
  comparisons include the actual row's `last_modified_at` even when its current
  journal event is absent.
  Progress merges `is_read` with OR, page and history with
  NULL-safe max, and bookmark by the deterministic version winner. A result
  equal to current state only advances dedupe; an exact incoming result keeps
  the incoming origin, while a new merged result gets a local-origin event.
  Apply captures the starting local sequence, removes only its automatic
  journal entries, writes the final row version explicitly, then journals the
  authoritative remote event and locally derived parent/cascade events with
  their proper origins. Shared manga/chapter fanout is rebuilt for every
  affected local user, while user-owned state stays with the authenticated
  user. `sync_chapter_state` is retained except for real deletion.
  Admin-only `POST /api/v1/sync/compact` compacts in one immediate transaction.
  It writes current rows and every latest tombstone in manga, manga_user,
  category, chapter, chapter_user, category_manga order. Baseline rows receive
  new local cursor sequences and preserve each current entity event's origin;
  `sync_alias` retains the other remote-to-local mappings. A current stub
  recreated after a tombstone instead gets a local origin; tombstones keep
  their own origins. The response `{baseCursor, removed}` records the first new
  baseline sequence; `sync_alias` and `sync_origin_state` are untouched. A
  nonzero cursor below
  that sequence gets 409 `resync_required` and must restart at zero, which
  returns the complete baseline rather than an empty pseudo-page. Replaying a
  baseline is idempotent through the same dedupe and merge rules, and later
  deltas continue from the new high-water mark. Compaction is operator-invoked,
  never scheduled, and tombstones and origin state survive it.
- Filter groups (`api/filter_groups.rs`, migration `0013_filter_group.sql`):
  Aidoku-style saved rule sets. A `filter_group` row belongs to the admin
  (`unique (user_id, name)` with `user_id = 1`, `sort_order`), its
  `filter_group_rule` rows cascade with it (`unique (group_id, type, value)`;
  flag rules carry a NULL value, so the write path — not the index — dedupes
  them, SQLite treating NULLs as distinct). `GET`/`POST /api/v1/filter-groups`,
  `PATCH`/`DELETE /api/v1/filter-groups/{id}` (PATCH renames and/or
  replaces the rule set wholesale) and `PATCH .../reorder` follow the
  category handlers' shape. Rule types: `downloaded` (EXISTS
  `chapter_user.is_downloaded`), `started` (EXISTS
  `chapter_user.last_read_at`), `completed` (`m.status = 2`),
  `has_unread` (default-unread: EXISTS a chapter with no read
  `chapter_user` row, including a missing row), and the valued
  `source` / `content_rating` / `category` (one id per row). Evaluation
  matches Aidoku's filter loop (`compile_group`): includes OR within one
  valued dimension, excludes apply per value, dimensions and flag rules
  AND — "all rules ANDed" holds only across dimensions.
  `GET /api/v1/library?groupId=` compiles one group into the same WHERE
  clause, ANDed with the ad-hoc parameters; a nonexistent group 404s like a
  nonexistent category. The web UI manages groups in Settings →
  Filter groups and renders them as a single-select chip strip above the
  category chips; two admin display settings (legacy `user.1.*` keys,
  next to `incognito.enabled`) steer the page: `library.default_category`
  (`all` / `uncategorized` / a category id) seeds the initial chip only
  on the first visit — a stored chip selection (#243's localStorage
  prefs) always wins afterwards — and `library.show_uncategorized`
  hides the uncategorized chip.
  `GET /api/v1/library/source-counts` returns per-source in-library totals
  (`{sourceId, count}`) for the web UI's source filter dropdown — counts
  include disabled sources' rows and only in-library manga. Chapter reconcile
  (`source/mod.rs:reconcile_chapters`) keeps a chapter the source stopped
  listing when it has download rows, and deletes it otherwise — except keys
  in the `protected` set (the local source's failed-directory protection,
  see below), which survive the pass unread. A previously unseen key with one
  unique vanished number match rekeys that existing row in place rather than
  deleting and recreating it: the chapter id, reading progress, download rows
  and artifacts survive, and its `fetched_at` is preserved. Page rows are
  cleared and `page_count` resets to zero so the next page load fetches the new
  key. Only truly new chapters receive the current timestamp; rekeyed replacements remain in
  `new_ids`, leaving update results and notifications intact while the
  downloads-only re-upload filter decides their candidates. Manga rows
  persist the source `viewer` (aidoku `Viewer` as int) and chapter rows the
  source `thumbnail`; a source declaring exactly one language fills missing
  chapter languages with it on reconcile, and a whitespace-only chapter title
  folds to NULL, matching the Aidoku host's `""` → nil normalization. A non-zero
  `manga_user.viewer_flags` is the admin's viewer override: API responses
  report it as the manga `viewer` while the source value stays stored.
- Incognito reading (`api/reader.rs`): "history" is `chapter_user.last_read_at`,
  stamped whenever `lastPageRead` is written; the history page selects rows
  where it is non-null. The admin's
  `incognito.enabled` setting — stored as the legacy key
  `user.1.incognito.enabled` and translated to a bare key by
  `GET`/`PUT /api/v1/settings` — or a per-manga `manga_user.incognito` flag
  (Aidoku's pause-history analogue) pauses the stamp: a progress write still
  saves `last_page_read`/`is_read` but leaves `last_read_at` untouched — new
  reads never enter history and existing stamps are kept. Marking unread
  (a bare `isRead: false` — no `lastPageRead` in the same request — on
  `PATCH /api/v1/chapter/{id}` or the `/chapter/batch` equivalent) is
  Aidoku's mark-unread, not a plain flag write: it wipes the
  whole reading trace — `last_page_read`, the resume target and the history
  stamp all go (explicit user intent, so incognito does not protect the
  stamp), while the bookmark survives; the detail page's row menu and batch
  selection share this semantic. A patch carrying `lastPageRead` alongside
  `isRead: false` is "still reading" and keeps the trace. Marking read never touches the stamp by
  itself, deliberately diverging from Aidoku's date-refresh. `GET`/`PATCH
  /api/v1/manga/{id}` carries `incognito` (the PATCH upserts the `manga_user`
  row, so the flag also works for manga not in the library), and the backup
  document round-trips it, defaulting to off for backups predating the field.
  The web UI exposes the admin-wide toggle in Settings (`SettingsPage.tsx`) and
  the per-manga toggle as an icon button in the detail header
  (`MangaDetailPage.tsx`).
- Backup export/import (`api/backup.rs`): `GET /api/v1/backups/document` answers one
  JSON document (a `Content-Disposition` attachment) with the admin's complete
  state: library rows and metadata, categories with membership, reading
  progress, app settings and source settings, plus source identity (key, name,
  version and a best-effort repo URL). `POST /api/v1/backups/document` restores into the
  same owner state, optionally limited by `?sections=` to some of `library`,
  `categories`, `progress`, `settings`, `sourceSettings`. Restore upserts by natural key — source
  key + manga key, chapter key, category name — inside one SQLite
  transaction; an unknown `version` or an invalid row answers 400 with the
  database untouched. Entries naming a source that is not installed are
  skipped and listed in the response `warnings`; the `restored` map counts
  applied rows per section. Category update and automatic-download exclusions
  use the backup-only `includeInUpdate` and `includeInDownload` fields (`2` for
  excluded categories, `0` for all others). Legacy `includeInDownload: 1` marks
  include-only categories, which an exclusion list cannot represent. A category
  restore containing any such entry forces `auto_download_new_chapters` to
  `false`, even when the target or backup enables it; review and reconfigure
  category exclusions and automatic downloads before re-enabling them. The raw
  `library.excluded_update_categories` and `library.excluded_download_categories`
  settings are omitted from exports and ignored during settings restore;
  category restoration maps names to target-local ids and merges them into the
  existing exclusion settings without writing the retired category columns.
  Progress entries carry a
  chapter metadata snapshot so a fresh instance can recreate chapter rows ahead of the
  source's first fetch. The document's `version` field (currently `1`) is
  the format contract, and source setting values round-trip in the same
  bare-JSON form `PUT /sources/{id}/settings` accepts, so a backup can
  contain source credentials: treat the file as sensitive.
  The construction (`build_backup_doc`) and the restore execution
  (`import_backup_doc`) are shared with the scheduler and the
  restore-from-list endpoint below, so a scheduled file is the same
  document an admin's GET handler serves.
- Aidoku backup import (`api/aidoku_backup.rs`):
  `POST /api/v1/backups/import/aidoku` accepts a `.aib` upload — a binary plist of
  Aidoku's Codable `Backup` struct, decoded with the `plist` crate into
  tolerant structs that only name the consumed fields. It imports into the
  requesting user's own state: library
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
  `POST /api/v1/backups/import/suwayomi` accepts a Suwayomi-Server or
  Mihon/Tachiyomi backup as a binary body. Both apps write the same
  bytes — gzip around one protobuf `Backup` message — whatever the
  extension (`.tachibk`, `.proto.gz`); the endpoint sniffs magic bytes
  and also accepts a raw proto (as Mihon's decoder does) and, defensively,
  a zip holding a `.proto` entry. The prost messages are hand-written
  against the wire schema (Suwayomi's `backup/proto/models`, Mihon's
  `data/backup/models`), declaring only consumed fields so tracking,
  memos, preferences and Suwayomi's `meta` maps are skipped on decode.
  The merge lands in the requesting user's own state and reuses the same
  additive primitives as the Aidoku import
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
  saved searches and server preferences are out of scope. The manga
  `url` is the Mihon source's internal key, whose shape can differ
  from the corresponding Aidoku source's (Suwayomi's copymanga
  extension stores site paths — `/comic/<slug>`,
  `<slug>/chapter/<uuid>` — where Aidoku's `zh.copymanga` uses the
  bare `<slug>`/`<uuid>`); a per-source registry (`api/key_realign.rs`)
  rewrites the known shapes to the target source's native format
  before any row lands — manga, chapter and history keys alike,
  idempotently — and the group report counts the rewrites under
  `realignedKeys`. Keys of sources without a registry entry import
  as-is, which may not line up with the Aidoku source's — such
  entries import but a later refresh can miss them.
- Scheduled backups (`backup/mod.rs`): a `BackupScheduler` task (wired in
  `build_app` like the update scheduler) wakes every 60s, re-reads the
  `backup.*` keys from the `setting` table and, when `backup.enabled` is
  set and the run is overdue — no `backup.last_run` yet, or more than
  `backup.interval_hours` since it — writes
  `sumidoku-backup-<yyyymmdd-HHMMSS>.json` (UTC) to the backup dir through
  a tmp+rename swap, stamps `backup.last_run`, and prunes. The scheduled
  document is always built as the built-in admin: a server-side full
  export (global settings and source settings included). The
  first tick doubles as the startup overdue check. Pruning keeps the newest
  `max(backup.retention_count, 1)` files by name order — the newest file
  is never deleted, even at retention 0 — and ignores files not matching
  the name pattern. Each setting resolves env (`SUMIDOKU_BACKUP_ENABLED` /
  `_INTERVAL_HOURS` / `_RETENTION_COUNT` / `_DIR`) over the `setting`
  table over the built-in defaults (disabled, 24h, keep 10,
  `<config_dir>/backups`). `GET /api/v1/backups/files` lists the dir
  (name/size/createdAt, newest first, plus the effective `dir`);
  `POST /api/v1/backups/files/restore` reads a listed file server-side and runs
  it through `import_backup_doc`, accepting only names that match the
  backup pattern (everything else, `..` included, is a 400; a
  well-shaped but missing file is a 404).
- Downloads (`download/mod.rs`): serial within a source, parallel across
  sources, with a page-level fetch cap inside a chapter. Download rows use
  the legacy `(chapter_id, user_id)` primary key with `user_id = 1`: enqueue,
  pause, cancel, list and stream events all belong to the admin.
  `DownloadSettings::load` reads the admin's current settings for enqueue,
  dispatch, update and reader operations: format and source concurrency are not
  captured at manager construction, and the update scheduler likewise reloads
  its behavior toggles. Only `downloads_dir` stays fixed at deployment time.
  Artifact files stay at deterministic paths and are reused when a chapter is
  already downloaded. Both normal completion and existing-artifact settlement
  commit the finished `download` row and the admin's
  `chapter_user.is_downloaded` flag in one `BEGIN IMMEDIATE` transaction;
  either write failing rolls both back, and the `finished` event is emitted
  only after commit. The manga detail seeds its active queue and per-page
  progress with one download-list request, then follows events; terminal events
  clear the activity and completion sets `isDownloaded`. Manual batch downloads
  accept selected chapters or whole-manga unread, newest-N and all rules; the UI
  skips downloaded chapters and the server deduplicates active rows. Non-text pages are measured from newly fetched bytes on
  every download, but their page-row updates commit only after packaging
  succeeds: a failed attempt writes no partial metadata, and the move-failure
  fallback still commits the measurements with the valid artifact. A row's
  stored `path` resolves through `download::product_path` — absolute is used
  as-is, relative joins `downloads_dir` — and every read/delete site
  (`api/download.rs`) goes through it.
  `DownloadManager::delete` is the shared removal routine for explicit delete,
  clear and read-driven deletion: it removes the row, clears `is_downloaded`
  and deletes the artifact once no other row references it. Manual progress
  (`isRead: true` without `lastPageRead`) and reader completion
  (`lastPageRead = pageCount - 1`) have independent `delete_after_manual_read`
  and `delete_after_read` switches. Read-driven deletion skips bookmarks unless
  `allow_delete_bookmarked` is enabled; that check does not restrict explicit
  deletion. `enqueue_reader_ahead` runs after a successful reader page load:
  `downloads.download_ahead` enables it and `downloads.download_ahead_count`
  bounds the following unread chapters in source reading order. Existing
  downloaded/queued/running chapters are skipped, errors may retry, and the
  ordinary enqueue dedupe prevents repeated page loads from growing the queue;
  download workers and image requests never recurse into reader-ahead.
- Download move (`download/mod.rs:move_product`): when
  `download.move_path` is set (setting table, env
  `SUMIDOKU_DOWNLOAD_MOVE_PATH` overrides), a finished package is moved
  to the path its template renders to. Variables `{source}`,
  `{title}`/`{manga}`, `{author}`, `{artist}`, `{chapter}` and
  `{filename}` (the default base name, no extension) each pass through
  `sanitize()`, with empty or unknown variables rendering as `Unknown`.
  A template containing `{filename}` renders the full file name (`.cbz`
  is appended when it lacks a known archive extension); anything else
  renders a directory the default product name is appended under. The
  move creates parents and renames, falling back to copy+remove on
  cross-device targets (a mounted share is normally another filesystem);
  a failed move (mount down, permission, disk full) logs and keeps the
  product at its default location without failing the download. Moved
  rows store the absolute target (a relative template target stays
  relative under `downloads_dir`), so the move target is a shared
  content area: downloading the same chapter again renders the same
  absolute file, overwrites it with identical content or settles onto it
  without repacking, and deletion removes the file only
  once no row references it (an already-missing file counts as
  deleted). Metadata edits only steer future downloads — earlier
  products stay where they were moved.
- Page blobs (`source/mod.rs:materialize_image_pages`): a
  `PageContent::Image` page holds an actor-local rid, so `load_pages`
  exchanges it for PNG bytes in the same call that fetched the page list
  and stores them under `config_dir/page-blobs/<chapter_id>/<idx>.png`; the
  page row keeps just the file name. The reader (`api/page.rs`) and
  downloads (`download/mod.rs:page_bytes`) read the blob; chapter dirs whose chapter row is gone
  are swept at startup (`source/mod.rs:sweep_page_blobs`).
- Page image dimensions (`source/page_dimensions.rs`, migration
  `0015_page_dimensions.sql`): the stored `PageRow` projection carries
  nullable `width`/`height` and an internal `dimensions_checked` marker. Dimensions
  are measured only during downloads (`download/mod.rs:fetch_page`): after the
  page file is written, the downloader reads only the first 64 KiB head and runs
  it through image 0.25's `ImageReader::into_dimensions` (decoder metadata, not
  full pixel decoding). If the head is inconclusive and the file is larger than
  the window, it falls back to a full read; a read failure leaves the row
  retryable and invalid image bytes become checked with nulls. Page-list loads
  (`load_pages`) do not fetch or measure images just for metadata. Page-row
  rewrites reset all three fields so changed images cannot inherit stale
  dimensions. `GET /api/v1/chapter/{id}/pages` keeps the existing content/index
  shape and adds top-level `width`/`height` only when both are known; text and
  unknown pages omit the pair.
- Reader page downsampling (`api/page.rs`, `source/downsample.rs`):
  `GET /api/v1/chapter/{id}/page/{index}?width=N` serves a downsampled
  variant of any image page kind (blob, zip entry or fetched URL), the
  server-side counterpart of Aidoku's `Reader.downsampleImages`. The width
  is quantized up to a multiple of 256 and variants are stored as PNG under
  `page-blobs/<chapter_id>/variants/<bucket>/<idx>.png`, so buckets never
  share bytes and a chapter reload (`materialize_image_pages`' wipe) or the
  startup blob sweep invalidates them for free; a `<idx>.pass` marker
  records "not resized at this bucket" (narrow image or undecodable bytes)
  so repeat hits skip re-decoding. Pages never upscale and any failure
  falls back to the original bytes, so a request without `width` — and
  downloads, which never pass one — stays byte-identical to the
  unmodified pipeline. Decode+resize runs on a blocking thread with
  a Lanczos3 filter (line art survives shrinking better; the cost lands
  only on a variant's first request). The web reader asks for its
  container width times the device pixel ratio
  (`webui/src/pages/ReaderPage.tsx:readImgWidth`).
- Page descriptions (`source/mod.rs:fetch_page_descriptions`): per the
  Aidoku contract a page with `has_description` and no inline
  `description` gets its text from the source's `get_page_description`, so
  `load_pages` fetches it in the same pass that materializes image pages
  and stores it in the page row's `description` field. A failed
  description export fails the whole chapter load, same as image
  materialization. The reader renders a flagged page with a description
  as a text page (`webui/src/pages/ReaderPage.tsx:pageText`); downloads
  keep reading only the page `content`.
- Image processors: when a source exports `process_page_image` /
  `process_cover_image`, `fetch_page_image` / `fetch_cover_image`
  (`source/mod.rs`) pass the fetched response through the actor
  (`SourceHandle::process_page_image`) and replace the bytes with the
  returned PNG — for the reader, downloads and the cover endpoint alike. The processor input follows AidokuRunner's `store(value:)`
  semantics: bytes the host can decode arrive as pixels, so width/height
  and canvas composition work (jmcomic-style descrambling depends on it)
  and `ImageRef.data()` returns a PNG re-encode; bytes the host cannot
  decode arrive raw (`ImageRef.data()` returns them byte-for-byte), so
  sources with encrypted-at-rest images can decrypt them. A source may
  also return the input image unchanged (jmcomic's unscrambled chapters
  and GIF pages do); the actor then yields the input pixels, or the raw
  bytes when they were never decodable. Cover caching is unchanged: the
  processed bytes are what the on-disk cache stores.
- Browse/home covers are proxied through
  `GET /api/v1/sources/{id}/image?url=…` (`source/cover.rs:get_source_image`):
  only http/https URLs are accepted and the response goes through
  `fetch_cover_image` (so a declared cover processor runs) without a disk
  cache, so a hotlink-protected host sees the source's `get_image_request`
  headers exactly as on the detail and reader paths.
- Manga covers (`source/cover.rs`): `GET /api/v1/manga/{id}/cover` serves a
  disk cache (`covers_dir/<id>` plus an `<id>.content-type` sidecar) and
  refetches `manga.cover_url` through `fetch_cover_image` only when the
  file is missing or `last_fetched_at` is newer than its mtime.
  `GET /api/v1/manga/{id}/covers/alternates` calls the source's
  `get_alternate_covers` with the stored manga row and returns the URL
  list, 501 when the source does not export it.
  `PUT /api/v1/manga/{id}/cover` `{url}` persists a pick to
  `manga.cover_url` and deletes both cache files: the freshness key is
  `last_fetched_at`, which a user pick must not move, so without the
  delete the stale bytes keep serving and the pick never takes effect.
  Because the cover response is `max-age=86400` cacheable, the web UI
  versions cover `<img>` URLs with the manga's `cover` string
  (`webui/src/api/client.ts:coverUrl`), and the detail page's picker
  (`webui/src/pages/manga/CoverPickerModal.tsx`) grids the alternates
  through the source-image proxy above.
- Library updates (`update/mod.rs`): serial per source, at most 3 sources at
  once, broadcasting progress as `UpdateEvent`s. The global
  `library.skip_titles` setting defaults to `hasUnread`, `completed` and
  `notStarted`; `library.refresh_metadata` defaults to false;
  `library.update_interval_hours` falls back to config
  `update_interval_hours` (default 0 = never); and
  `library.excluded_update_categories` defaults to an empty array of category
  ids stored in the legacy `user.1.library.excluded_update_categories` key.
  The admin can read and write all library settings. Migration
  `0016_excluded_update_categories.sql` originally converted per-user legacy
  `category.include_in_update = 2` ids; after migration 0019 only the admin's
  setting remains. Selection keeps `update_strategy = Never` and the admin's
  exclusion list: a title whose categories overlap that list is skipped. The
  category API does not expose or consult `include_in_update`;
  `PATCH /api/v1/categories/{id}` with any `includeInUpdate` field answers 400
  and names the owner setting instead. The web UI exposes all update controls
  to the admin. A manual `POST /api/v1/update/fetch`
  with a `categoryId` ignores that category in the global exclusion list,
  matching Aidoku's behavior when pulling a specific category. The skip
  predicates are evaluated against the admin's `chapter_user` rows:
  `hasUnread` skips when any chapter has no read row (a missing row is
  unread), `notStarted` skips when the admin has no read chapter (including a
  title with no chapters), and `completed` skips status 2. A source schedule
  (`next_update_is_fallback = 0`) skips while its stored `next_update_time`
  is in the future. A fallback schedule (`= 1`) instead tests
  `chapters_last_fetched_at + the current effective interval`, so shortening,
  lengthening or disabling the interval takes effect without rewriting old
  manga rows; interval 0 is immediately due. The stored next update time is
  still advanced after success for the next source round trip. Manual fetches
  and the scheduled run both use the admin's library and auto-download rules.
  Each update reloads `auto_download_new_chapters`, `auto_download_limit` and
  the download filters. `library.excluded_download_categories` is the single
  admin category exclusion list, stored as
  `user.1.library.excluded_download_categories`; migration
  `0020_excluded_download_categories.sql` converts only the admin's legacy
  `include_in_download = 2` ids and preserves an existing new setting. Legacy
  `include_in_download = 1` include-only categories cannot be represented by an
  exclusion list, so any such row also forces `auto_download_new_chapters` to
  `false`; review and reconfigure the exclusions and automatic downloads before
  re-enabling them. Category REST responses omit `includeInDownload`, a PATCH
  carrying it answers 400 and names the new setting, and backup remains its only
  portable tri-state representation. REST and sync deletion remove the id from
  both owner-scoped exclusion arrays in the same transaction, so a reused id
  does not inherit the deleted category's exclusions. Exclusions affect only
  update-driven automatic downloads, never an explicit manual enqueue. With
  `downloads.auto_skip_manga_with_unread`, the unread query excludes the
  current `new_ids`, so only pre-existing unread chapters block and a missing
  progress row counts as unread. `downloads.auto_ignore_reuploads` removes
  candidates whose number is below the previous latest chapter number; it does
  not alter `new_ids`, update counts, unread state or notifications.
  The scheduler checks due every 60s and reloads the interval instead of
  capturing it at construction, so setting changes need no restart. A scheduled
  union stamps `library.last_scheduled_run_at` in the settings at start and
  finish; due uses that persisted timestamp and is immediate when no scheduled
  batch has run. The key is excluded from structured settings extras, and both
  settings write forms ignore it because only the scheduler owns it. Backup
  exports omit it and imports ignore it. Manual batches still stamp the
  summary's `last_run_at` for reporting, but never the scheduled-only clock, so
  they cannot postpone the union. Restarting the process does not defer a due
  union by another interval.
  Each manga fetch also reloads refresh/interval. WASM receives the refresh
  flag as `needs_details`; details returned by WASM, local or OPDS are
  upserted only when it is true. Before that upsert, the write transaction
  re-reads the current update strategy, so a Never set during a slow fetch is
  not overwritten by the pre-fetch snapshot. On success, only a returned
  `Some(at)` different from the pre-fetch value is a new source schedule: it
  is stored with `next_update_is_fallback = 0`. None or an exact echo of the
  old value means no new source schedule: a positive interval stores success
  time + interval hours with the flag 1, while interval 0 stores NULL with
  the flag 0. Other detail-upsert paths preserve provenance when the
  timestamp is unchanged and reset it to 0 when the timestamp changes; the
  update transaction's final write stores the resolved provenance. The same
  transaction reconciles chapters and stamps `chapters_last_fetched_at`; a
  failed fetch or persistence pass does not advance either timestamp. Events
  and the summary carry the fetch's owner. The SSE stream and
  `GET /api/v1/update/summary` present manual and scheduled updates to the admin.
- Source instances (`state.rs`): a `SourceHandle` actor spawns lazily on
  first use (`get_handle`) and lives in a registry keyed by source id;
  concurrent spawns for the same source collapse to the first arrival.
  Instances leave the registry on package update/uninstall
  (`evict_source`) and through an idle sweep (`run_idle_eviction`, wired
  in `build_app` like the update scheduler): every 60s it removes entries
  whose `last_used` — stamped at spawn and refreshed on every `get_handle`
  hit — is older than `source_idle_evict_minutes` (default 30; `0` or
  negative disables the sweep). Idle candidates are filtered under the
  read lock, re-checked and removed under a short write lock, and shut
  down outside the lock via the same `Arc::try_unwrap` courtesy as
  `evict_source`: an in-flight call keeps its instance until the last
  reference drops, while a new lookup respawns immediately. The sweep
  exists because WASM linear memory only grows — eviction is the one
  path that returns an idle instance's memory to the OS, at the price of
  a seconds-long recompile on the next access. Aggregate pressure has
  its own watcher (`run_rss_monitor`, same 60s cadence, spawned only
  when the platform reports RSS — macOS via mach `task_info`, Linux via
  `/proc/self/statm`): above `rss_threshold_mb` (default 1024 MiB; `0`
  disables) it evicts most-idle-first, re-reading RSS after each
  eviction until the reading drops under the threshold or nothing
  evictable remains. The pressure pass ignores the idle threshold —
  responses stream or land on disk and there is no in-memory page cache
  to drop, so resident instances are the only memory that can be
  returned — and skips instances with outside `Arc` references entirely:
  detaching an in-flight instance frees nothing, and the next lookup
  would respawn a second one while the first still runs. When RSS stays
  over the threshold with nothing left to evict, the monitor only logs
  loudly (`tracing::warn!` with before/after/threshold and the resident
  count); restarting the process stays an operator decision.
  Per-instance `Limits`
  (`crates/wasm/src/runtime/limits.rs`) are server-configured at spawn:
  `wasm_memory_max_mb` (default 128 — Aidoku sources rarely exceed 64
  MiB, but a tighter cap makes `memory.grow` fail and kills the source),
  `wasm_fuel`, `wasm_call_timeout_secs` and `net_body_max_mb`; the
  sleep/net-timeout/js/browser knobs stay hardcoded. Every export call also passes two admission
  semaphores (`crates/wasm/src/runtime/actor.rs:call`): the global
  `call_concurrency` cap, and — only when the source manifest declares
  `config.maximumParallelRequests` — a per-source cap of
  `max(1, declared)` permits built at spawn and held on the
  `SourceEntry`. Aidoku applies the same declaration as
  `max(1, min(config, default))` to page-download groups; stacking the
  two semaphores reproduces the `min(declared, global)` bound while
  undeclared sources keep the global cap alone.
- Outbound requests pass a `NetPolicy` (`source/routed.rs:SumidokuNetPolicy`):
  direct, per-domain relay, or a global proxy, decided per host. A direct
  request answered 403/503 with Cloudflare headers first triggers a clearance
  solve through the trawl scrape API (`net.browser_trawl`,
  `browser/trawl.rs:solve_clearance`), ahead of the 429/5xx backoff — in the
  source net imports (`crates/wasm/src/imports/net.rs:perform`) and in the
  server's own fetches alike. The answer's `cf_clearance` and the solving
  browser's user agent are cached per host and replayed on plain direct
  requests: CF binds the cookie to that UA, so the pair travels together and
  the replayed request drops the Chrome-only suite members (Firefox has no
  `sec-ch-ua`) and carries Firefox's navigation `Accept` (no
  `signed-exchange`). A refused clearance is dropped and re-solved once per
  request. Only when no solver is configured or the solve fails does the host
  fall back to the trawl forward proxy (MITM CA in `net.trawl_ca`) and stay
  marked "challenged" — the proxy's browser tiers answer HTML with the
  serialized DOM, which mangles fragment responses that declare no charset,
  so it is the fallback, not the default. `net.direct_domains` opts a host
  out of solving and escalation, `net.relay_domains` routes a host through
  trawl from the first request.
  Set-Cookie on the solved response still lands in the source's cookie jar.
  These server-side fetches — covers, page images and zip pages
  (`fetch_remote` / `http_get`) — bypass the wasm net host, so they attach
  the same per-source `Accept-Language` default themselves (via
  `RoutedClients::get_with` / the image-request builder); a source that set
  its own `Accept-Language` on an image request keeps it, and a source-set
  `User-Agent` likewise wins over the global one
  (`RoutedClients::request_with`, insert-only-if-absent) rather than being
  appended as a second value — Cloudflare reads only the first and 403s a
  duplicated header. The same insert-only-if-absent paths fill the browser
  header suite (`aidoku_wasm::BROWSER_HEADERS`: the `sec-ch-ua` trio,
  `accept`, the `sec-fetch-*` quartet and `upgrade-insecure-requests`), and
  both reqwest builders negotiate HTTP/2 — the user agent claims Chrome,
  and passive Cloudflare checks exactly that coherence of protocol and
  header suite rather than the TLS fingerprint. The suite sits behind a
  per-policy knob (`NetPolicy::browser_headers`, config
  `net.browser_headers`, default on): `sec-fetch-*`'s navigate semantics
  are a mild lie for API/XHR requests, so the knob keeps the honest
  option. The lie also has one hard failure mode, repaired in the source
  net imports (`crates/wasm/src/imports/net.rs:perform`): Django REST
  framework negotiates on `Accept`, and the suite's document-class
  `Accept` turns an API call into DRF's browsable-API HTML page, which
  breaks the source's JSON parse. When a direct 2xx `text/html` response
  carries the `Django REST framework` marker
  (`traits.rs:is_drf_browsable_page` — the marker, never "is HTML", so
  legitimate page fetches are not double-fetched), the request is resent
  once with exactly the headers the host injected stripped from it — a
  source-declared `Accept` stays — which reproduces what AidokuRunner
  sends, since it injects no headers at all. Their response bodies are
  capped as well: covers and
  page images share the net host's `net_body_max_mb`, while zip pages —
  a whole chapter packed into one archive — fall under the higher
  `net_zip_body_max_mb` (default 128 MiB).

## WASM sources (`crates/wasm`)

Aidoku source packages (`.aix`, zip) run in wasmi. One actor per source
(`runtime/actor.rs`) owns the store (`runtime/store.rs`), per-source rate
limiting (`runtime/rate_limit.rs`), and the host functions sources import
(`imports/`), including the HTML DOM helper (`html_dom.rs`) and the Boa
JavaScript engine (`js_engine.rs`) behind the `js` context functions — one
isolated context per ABI descriptor, time- and loop-limited, with no host
access beyond what the ABI exposes. Each instance runs under the `Limits`
from its `SourceConfig` (`runtime/limits.rs`): a fuel budget per
export call, a per-call timeout that fuses and restarts the instance, and
a linear-memory cap enforced through wasmi `StoreLimits` — the server
sets these from its own config when it
spawns an instance, defaulting the memory cap to 128 MiB.
Every export call funnels through the actor's private `call`, which logs
one structured line per outcome — `source`/`op`/`dur_ms`, plus the entry
count on success at info or a sanitized error at warn; guest and reqwest
error text can embed full URLs, so payloads are scrubbed down to the host
and capped before they reach the log.
The `abort`, `print`, `sleep` and `send_partial_result` host functions are
registered under both the legacy `env` module and the newer `std` module
(`imports/mod.rs`).
The net host (`imports/net.rs`) aligns request behavior with the
Aidoku/URLSession host. Response bodies are buffered under a
configurable cap (`net_body_max_mb`, default 32 MiB, decoded bytes —
gzip is counted after decompression): a declared content-length over
the cap fails fast, otherwise streaming reads stop at the cap, and an
oversized body fails the request with the ABI's generic request error
while naming the limit in the source log. `html()` decodes the body by BOM, then
a declared UTF-16 charset, then strict full-document UTF-8 validation, then the
Content-Type charset, then a `<meta charset>` sniff of the first 1024 bytes,
falling back to UTF-8 with replacement (encoding_rs). The UTF-8 validation
outranks a declared single-byte charset on purpose: servers frequently serve
UTF-8 pages under a lying label such as `iso-8859-1`, which would otherwise
decode as windows-1252 mojibake, and the Aidoku host decodes bodies as UTF-8
outright; genuinely non-UTF-8 pages (GBK/Big5) fail the validation and follow
the declared or sniffed charset as before. When decoding still leaves U+FFFD
replacement chars, the document's own charset declaration is recovered from the
decoded text (ASCII-compatible encodings keep the `<meta>` tag readable) and
the raw bytes re-decode once — covering declarations past the sniff window; a
non-UTF-8 page decodes instead of failing the call. gzip, br and deflate
are negotiated and decoded; a source may still override `Accept-Encoding`
itself. Stored jar cookies merge in front of a source-set `Cookie` header
(`jar; source`) instead of being dropped, and a default `Accept-Language`
goes out when the source set none — derived from the source's language by
`default_accept_language` (e.g. `zh` → `zh-CN,zh;q=0.9`, unknown → `en-US`),
standing in for the device locale URLSession supplies on Aidoku; a
multi-language source takes its seeded `languages`/`language` setting first
(the user's pick), then the manifest's list.
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
`webview_get_cookies`, `webview_delete_cookie` — run on a trawl browser
backend (`browser.rs`, `browser/trawl.rs`; trawl is an external HTTP
sidecar, Sumidoku ships no browser). With `net.browser_trawl` unset they
return `-1` (`JsError::MissingResult`; the ABI has no "unsupported" code)
and log the function name and reason, so a source relying on them sees the
calling operation fail with a diagnosable line in the source log. With
`net.browser_trawl` pointing at a trawl API URL, every webview
rid maps to one keepPage session named `sumidoku-<source>-<rid>`, created
lazily on first use and deleted (`DELETE /sessions/:id`) eagerly on destroy
— trawl caps concurrent sessions per instance (`BROWSER_SESSION_MAX_ENTRIES`,
default 4), so sessions are never pooled. `webview_load` navigates via
`POST /scrape` (GET only) and waits for the page load event — the
same completion signal as WKWebView's `didFinish` on iOS — and
`webview_get_cookies` reads `GET /sessions/:id/cookies` (HttpOnly included).
`webview_eval` /
`webview_eval_async` both run through `POST /sessions/:id/eval`, which
evaluates in the kept page's main JS context and awaits returned promises —
scalar results come back in their JS string form, objects/arrays as compact
JSON. A 404/410 (session expired, its browser replaced, or retired by an
eval timeout — trawl sessions are in-memory and lost on restart) recreates
the session under the same id and retries the operation once; a duplicate
id on create is adopted (a restarted Sumidoku reuses rid numbers while
trawl may still hold the old session); 409 busy, 429 (registry full) and
503 are single-operation failures like a navigation timeout.
`webview_delete_cookie` has no trawl endpoint and is explicitly
unsupported.
Cookies from the browser session flow into
the source's session jar after every load and on every `get_cookies`,
keyed by each cookie's own `Domain` attribute — that is what makes
webview login work: the source loads the login page, the login completes
in the browser, and the source's plain API requests then carry the
session cookie. `webview_load` only navigates GET requests (trawl's
scrape API is GET-only; iOS would load the full URLRequest) and a non-GET
request fails with `-1` and a log line rather than silently degrading.
`webview_set_rule_list` (content blocking) stays explicitly unsupported.
An invalid
webview rid maps to `-4` (`InvalidHandler`), and webview and js-context
rids are not interchangeable.
`send_partial_result` payloads are decoded per in-flight call and pushed
to streaming callers (`SourceHandle::get_home_streaming` /
`get_search_manga_list_streaming` / `get_manga_update_streaming`); home
partials pass through the upstream title-upsert accumulator (`partial.rs`)
so consumers receive index-addressed events, and an empty final layout
falls back to the accumulated partials, while search and manga-update
partials forward each decoded `Manga` as-is (a full snapshot per event,
matching upstream's `partialMangaPublisher`). Calls without a streaming
caller discard partials, as before. The `canvas` ABI (`imports/canvas.rs`)
is rendered by
tiny-skia: a context is a premultiplied-RGBA pixmap plus a current
transform, and images live in the same store as straight RGBA8 buffers
(`runtime/store.rs:ImageData`), converted at the boundary. `fill` /
`stroke` take postcard-encoded `aidoku::canvas::Path` / `StrokeStyle`
payloads, `set_transform` reassembles the source-side decomposition as
translate·scale·rotate (the ABI's radians become tiny-skia's degrees),
and `draw_image` / `copy_image` blit through a bilinear pattern fill
whose transform maps image space onto the destination rect. Text is laid
out glyph by glyph from `ttf-parser` outlines with no shaping or kerning,
placed as AidokuRunner places it (`CTLineGetImageBounds`): `(x, y)` is the
top of the text, so the baseline moves down by the rendered line's
glyph-bounding-box height and a string with descenders sits one descender
lower. Fonts come from a process-global `fontdb`
(`fonts.rs`): an embedded DejaVu Sans (`assets/`, Bitstream Vera license)
is the fallback that keeps Docker/headless hosts working, system font
directories are scanned once for coverage (notably CJK), and `load_font`
fetches over the source's own net policy into the same database.
`new_font` matches AidokuRunner's `UIFont(name:)`: an exact PostScript-name
or family match wins, and a name matching nothing fails with `invalidFont`
(-10) instead of silently falling back; `system_font` always resolves,
falling back to the embedded face when no match exists. Canvas RIDs follow
the normal `destroy` lifecycle, and the host
consumes them eagerly wherever a result must outlive the actor call:
`SourceHandle::take_page_image` encodes an image rid to PNG and drops it.
The `std` host's `parse_date` (`imports/std.rs`) follows Swift's
DateFormatter as AidokuRunner drives it: the format string is translated
token-wise (including `'…'` quoted literals, `S` fractional seconds and
the `X`/`x` ISO-8601 zone offsets real sources use), a date-only string
parses as midnight, an offset carried in the string wins over the timezone
argument, and the timezone defaults to the host's current zone
(`iana-time-zone` → `chrono-tz`) when absent, `current` or unparseable.
chrono parses English month names only, so locale support is a
substitution table covering the locales sources actually pass (es, it, pt,
tr, fr, id, ar and the CJK numeric 月/월 forms); month names in other
locales, localized digits such as Eastern Arabic numerals, and localized
weekday names stay English-only.
`source.rs`
loads and validates packages; `package.rs` reads the `.aix` zip layout
(`Payload/source.json` manifest, `Payload/main.wasm` payload, optional
`Payload/icon.png`) into memory without extracting. The server scans the
`sources/` directory under `config_dir` for `.aix` files and extracted
source directories alike; both register the same way, with `wasm_path`
pointing at the `.aix` or the directory's `main.wasm`. An `.aix` icon is
written next to the package as `<name>.png` so installed sources serve
`/sources/{id}/icon` the same way the directory scan's `icon.png` does.
Uninstall (`DELETE /api/v1/sources/{id}`) removes the on-disk package
with the database row — the source directory, or the `.aix` plus its
`<name>.png` sidecar, only ever paths inside `sources/` — so a later
directory scan cannot re-register a deleted source; OPDS instances have
no package and only lose the row.
Registration also resolves the source's dynamic base URL, mirroring the
`try? await runner.getBaseUrl()` in AidokuRunner's `Source.init`: when the
payload exports `get_base_url` a throwaway instance answers it (carrying
the stored defaults and cookies, empty on a fresh install — the Runner
resolves before `register(defaults:)`, so the source sees the previously
registered values) and the result lands in `source.base_url`; a missing
export, a failed call or an unparsable URL only logs a warning and the
manifest's static `info.url` stands instead. That column is the truth
source every consumer reads — the synthesized base-URL setting's
candidates and seeded default, the login-host fallback, the `baseUrl`
DTO — so the settings list and the fallback never disagree.
The `languages` column stores language codes only: object-form manifest
entries (`{"code", "isDefault"}`, accepted by `source_languages`) are
normalized at registration — every consumer (the source DTO,
`Accept-Language` derivation) decodes `[String]`, while `isDefault` is read
from the manifest text by the synthesis/seeding path.
Static package files (`source.json`, `settings.json`, `filters.json`) are
read by the same helper: directory sources from next to `main.wasm`, `.aix`
sources from `Payload/` inside the zip without extracting. Only declared
dynamic exports are called: when `get_settings`, `get_filters` or
`get_listings` is absent, the corresponding merge uses static package data
without a source call. Source settings
(`GET/PUT /api/v1/sources/{id}/settings`) merge the static `settings.json`
with the dynamic `get_settings` export — static first, dynamic appended,
matching AidokuRunner's `staticSettings + getSettings()` — deduplicated by
leaf key with the first occurrence kept (AidokuRunner shows duplicates;
Sumidoku's rule is the same one the synthesized settings use). The static
Aidoku schema (flat, camelCase) is normalized
to the nested shape the dynamic export produces (`api::settings`), so the
API and web UI see a single contract.
`GET /api/v1/sources/{id}/filters` (`api/mod.rs:get_filters`) merges the same
way — static entries from the manifest's `filters` array (or `filters.json`
when the manifest declares none) first, the dynamic `get_filters` export
appended — and the static filter schema (flat, camelCase, `id` optional) is
normalized to the flat snake_case shape the dynamic export produces
(`api::filters`): `id` derives as `id ?? title ?? type`, Aidoku's static-filter
decode (without it every id-less filter shares an empty id, and the web UI's
id-keyed state and React keys overwrite each other); the capability keys map
camelCase to snake_case (`canAscend`→`can_ascend`, `canExclude`→`can_exclude`,
`isGenre`→`is_genre`, `usesTagStyle`→`uses_tag_style`,
`hideFromHeader`→`hide_from_header`, `defaultIncluded`→`default_included`,
`defaultExcluded`→`default_excluded`); and each variant's absent fields come
back null. Dynamic exports pass through untouched — the server is the single
authority for the shape. Defaults are seeded into
`source_setting` with UserDefaults `register(defaults:)` semantics: every
seed also records the default in the row's `seeded_value`, and a later seed
(a source update, or the next `get_settings` fetch) rewrites the live value
only while it still equals the previously seeded default — user-set values
are never overwritten, while untouched defaults (including the base URL)
track the new package. Rows written before the column existed carry a NULL
`seeded_value` and are treated as user values; a live value that merely
equals the seeded default counts as untouched (a materialized row cannot
tell an explicit set-to-default apart from never-touched; AidokuRunner's
registered-defaults model can). Seeding happens
for the language, base-URL and `settings.json` defaults at install/reset, and a
dynamic source's `get_settings` defaults on every successful fetch,
mirroring AidokuRunner's `register(defaults:)`. The rules match
`loadSettingsDefaults`: recurse `group` and `page`, fall back to the first
value for `select` and `picker`, and treat a `switch` without a default as
`false`. A source whose `source.json` sets
`config.allowsBaseUrlSelect` and ends up with more than one base-URL
candidate gets a synthesized base-URL `select` (key `url`) prepended to the
list, mirroring AidokuRunner's extra settings: the candidates are the
persisted `source.base_url` prepended to the manifest's `info.url` +
`info.urls` (skipped when already present, the Runner's dedup), so a
resolved dynamic value leads the list and is the default, and an untouched
`url` value tracks each re-resolution while a user-picked mirror survives
updates; a source declaring
more than one language gets a synthesized language setting (key
`languages`, or `language` when `config.languageSelectType` is `single`)
  prepended ahead of it, with the default from each language's `isDefault`
  (AidokuRunner picks the device-language intersection; the server has no
  device locale, so it falls back to the first). A synthesized setting is
  skipped when the source already declares the same key, so the list never
  shows two entries keyed alike.
Saving settings restarts a running instance with the
new defaults and then delivers every repeated `?notify=` query parameter to
`handle_notification` in order; the web UI collects the `notification`
string of each changed setting that declares one, mirroring Aidoku firing
it on every change. `POST .../notify` delivers a notification without a
restart — the `Button` setting's press semantics — and the web UI renders
Button settings as working buttons wired to it (a Button without a
`notification` string has no action and renders disabled, same as Aidoku's
no-op). A changed setting's `refreshes` entries map to client-side query
invalidations, Aidoku's `refresh-*` notifications: `settings` refetches the
source settings, `filters` and `listings` the respective queries, and
`content` the source home. In the web UI a `Link` opens in a new tab when
`external` is set and in the current tab otherwise (Aidoku's system browser
vs in-app Safari view), and a `Page` renders its `info` line and URL `icon`
alongside the title.
`POST .../settings/reset` clears the
overrides and re-seeds package defaults; `POST .../login/basic` runs the
source's `handle_basic_login` and, on success, persists the credentials and
a `logged_in` marker into `source_setting`; `POST .../login/web`
(`api/login.rs:web_login`) takes the login host's cookies pasted by the user
(a browser SPA cannot read third-party cookies, so there is no automatic
path), persists `<key>.keys` / `<key>.values` / `<key>.ls.<lsKey>` first —
Aidoku's store-first order, and the wasm host pre-seeds the same pair into
the running instance so the source can read them back via
`defaults_get_map` during the call — then runs the source's
`handle_web_login` when exported (a veto answers 502, a source without the
export simply stores), marks `<key>="logged_in"` (empty cookies clear the
marker and the host's cookie rows instead), upserts the cookies into
`source_cookie` under the login page's host (resolved from the setting's
`url`, the default pointed at by `url_key`, or the source `base_url`) and
restarts the instance. Persistence is synchronous SQL rather than the sink
channel: the restart right after would read stale rows from the
not-yet-flushed batch writer. `POST .../login/logout` prefix-deletes
`<key>` and all `<key>.*` children and clears the login host's cookie rows
only when the setting declares `clear_cookies_on_log_out` or the request
passes `clearCookies` (falling back to the whole jar when the host cannot
be resolved), then restarts. `POST .../cache/clear` drops the
source's cookies and restarts the instance. Per-source state spans two
tables: `source_setting` (defaults and login credentials) and
`source_cookie` (cookies keyed `(source_id, domain, name)`). A running
instance's cookie jar reports changes through a `CookieSink` to a batch
writer, so a login session survives the restart that follows a settings
change.

`method: "oauth"` logins (`api/oauth.rs`) reproduce Aidoku's app-layer flow
with a manual callback step — a browser SPA cannot intercept the
custom-scheme redirect sources bake into `redirect_uri` /
`callback_scheme`. `POST .../login/oauth/start` resolves the authorization
URL from the setting's `url` (or the default pointed at by `url_key`) and,
when `pkce`, persists a 128-char unreserved-set verifier at
`<key>.codeVerifier` and appends `code_challenge` (unpadded base64url
SHA-256 of the verifier) / `code_challenge_method=S256` /
`response_type=code` to the query; the web UI opens it in a new tab.
`POST .../login/oauth/complete` parses the pasted callback URL: with `pkce`
and `token_url` it POSTs
`grant_type=authorization_code&code&code_verifier[&redirect_uri][&client_id]`
— parameters not percent-encoded, `redirect_uri` / `client_id` captured
from the authorization URL's query — to `token_url` and stores the raw
response body at `<key>` regardless of status (Aidoku stores whatever the
endpoint answers); otherwise it stores the callback URL string at `<key>`
and fires the setting's notification (best-effort, matching Aidoku's
log-and-continue). Either way the instance restarts so the source reads the
new value, and the shared logout's prefix delete already covers
`<key>.codeVerifier`.

`vendor/aidoku` is the vendored upstream `aidoku` crate with the `imports`
feature. It tracks upstream; local fixes don't belong there.

## Web UI (`webui`)

Vite 8 + React 19 SPA, TypeScript. `src/pages` are the routes,
`src/components` shared pieces, `src/api` the REST client,
`@tanstack/react-query` for server state. `npm run check` is
`tsc --noEmit`, and `npm test` runs the Node test suite. The build lands in
`webui/build` and the server embeds it (see above). The installable PWA shell
uses `vite-plugin-pwa`'s `generateSW` strategy. Precaching covers only the built
JavaScript, CSS and HTML plus the web manifest and install icons; API requests
and manga media are network-only at the service-worker layer, and `/api` and
`/opds` navigations bypass the SPA fallback. Both disk and embedded serving
return `Cache-Control: no-cache` for `/sw.js` and `/manifest.webmanifest`, while
hashed assets and other responses keep their existing cache policies. The
Workbox MIT notice is served at `/licenses/workbox.txt`. When a
new service worker is waiting, the UI offers Reload and Later: Reload activates
it and reloads the page, while Later only dismisses the prompt; there is no
automatic reload. This provides installation and an application shell, not
offline chapters or offline reading progress.
Typography uses system stacks through `--font-sans`, `--font-display`,
`--font-brand` and `--font-mono`; brand marks use the rounded brand stack,
page-header titles use display serif, and content names and controls stay sans.
Route handles provide translated fallback document titles, and
`DocumentTitleProvider` appends ` · Sumidoku`. Source, manga and reader pages
replace the fallback with content names through `useDocumentTitle`; overrides
are scoped to the current pathname so a previous page cannot overwrite the next
page's title.

When authentication is on, the
login page takes the admin username + password with an initially empty username
field, browser API requests use the login cookie, CLI clients may send a
bearer header, and any `401` lands on `/login`. Desktop navigation keeps library,
browse and updates at the top level; activity and settings start collapsed and
open on an active route. The browse page keeps cross-source search above its
installed/repository views, with `/search` redirecting to its search view.
Remote source and cross-source searches run on explicit submission; local-source
filtering uses a 500ms debounce. Clearing a source's search box resubmits an
empty query so the unsearched results return without waiting for submission.
Aggregate rating/language/source filters can be
changed before the first query, and the search view's URL changes on submission.
Settings sections are linked from the settings group, while mobile keeps the
settings page's own tabs. The settings page shows the admin account and
all management controls; there is no Users page or role-dependent UI. The
reader uses positive known page dimensions (populated during downloads) as
image intrinsic dimensions and continuous/webtoon container aspect ratios,
reserving space before lazy loads; text, unsupported and unknown-dimension
pages get no forced ratio, and paged mode keeps its fixed viewport.
Text pages also offer font-family, font-size and line-height controls persisted
in `sumidoku.reader.typography`; the same style applies to paged and continuous
rendering, and typography changes recompute paged text columns.

## Known gaps

### Source login: OAuth callback capture

All three `Login` methods (`basic` / `web` / `oauth`) are implemented
(`api/mod.rs:basic_login`, `api/login.rs`, `api/oauth.rs`) and share the
logout endpoint. The remaining constraint is inherent to a browser SPA: the
OAuth callback lands on a custom scheme (or a cross-origin redirect the page
cannot read), so completing a login means the user pastes the redirected URL
by hand; there is no automatic capture path short of a hosted callback for
the rare rewritable `redirect_uri`.

### Source settings: biometric gates and iOS-only page metadata

`authToOpen` (select / multi-select / page) and `authToDisable` (toggle)
gate the action behind iOS LocalAuthentication (device biometrics) in the
Aidoku app. A browser has no equivalent primitive, so the web UI renders
these settings ungated; the flags pass through the API unchanged. Two
`Page` fields are likewise iOS-only: `inlineTitle` only selects the pushed
page's navigation-bar title mode, which has no analog in the web UI's
inline rendering, and a `system` icon names an SF Symbol — URL icons
render, system icons are ignored.

### HTML DOM fidelity vs SwiftSoup

The wasm host builds its DOM with html5ever — a spec-conformant HTML5 tree
builder — and serializes through scraper (`crates/wasm/src/html_dom.rs`),
while Aidoku's host parses and serializes with SwiftSoup (jsoup lineage).
Well-formed documents behave the same under both, but three divergence
classes are known and accepted rather than chased case by case:

- Serialization: `html()` / `outer_html()` output can differ at the byte
  level — void-element form, attribute quoting and ordering, and which
  non-ASCII characters serialize as named entity references versus literal
  UTF-8.
- Tree construction on malformed markup: foster parenting of text inside
  tables, `<template>` contents, foreign content (SVG/MathML), and
  misnested or unclosed tags — html5ever follows the WHATWG tree-builder
  algorithm exactly, while SwiftSoup follows jsoup's, which resolves some
  of these corners differently.
- Entity tables: the set of named character references each engine decodes
  does not line up one to one, and the host `escape` covers only `&amp;` /
  `&lt;` / `&gt;` where SwiftSoup's `Entities` covers the full table. The host
  `unescape` is aligned: it decodes the full HTML5 named and numeric reference
  set (via `html-escape`), matching what aidoku sources expect from
  `SwiftSoup.Entities.unescape`.

These are class-level differences between two engine families, not bugs
with a bounded fix list: byte parity would mean reimplementing jsoup's
serializer and tree-builder quirks wholesale. What sources actually
consume is selector matching plus the `text()` / `attr()` / `data()`
accessors, and those paths are aligned with jsoup semantics — for
`data()` that means script/style elements return their contents and
other elements return `""`, matching jsoup's `DataNode` model for the
direct call (jsoup additionally recurses into descendants' data nodes,
which the html5ever text-node model does not reproduce).

Source authors should therefore never post-process `html()` /
`outer_html()` output with string operations (regex, splitting,
index-of): extract data with selectors and the text/attr accessors
instead. When a concrete source does break on one of these differences,
open an issue naming the source and the page; that case gets fixed on
its own.

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
