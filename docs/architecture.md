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
  `POST /api/v1/sources/{id}/deep-link` (`api/mod.rs:deep_link`) hands a URL
  to the source's `handle_deep_link`. `Manga`/`Chapter` results are resolved
  to internal ids before answering — the manga is found or registered through
  the same find-or-register path a browse-page click takes
  (`find_or_register_manga`), and the chapter id is looked up by key (null
  when the source's chapter list lacks it, the web UI then lands on the manga
  page) — so the web UI navigates to `/manga/:id` or `/reader/:chapterId`
  with no second lookup; `Listing` passes through and the source browse page
  switches to that listing, and an unmatched URL answers `{"result": null}`.
  A source without the export answers 404 `unsupported`, same as `get_home`.
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
  search (owning `sourceId`, plus the internal `id` when registered). A
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
  web UI's `/search` page (`webui/src/pages/SearchPage.tsx`) renders each
  arriving section as a horizontal cover scroller and offers tri-state
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
  query). The manga detail page derives its tap-to-search capabilities
  from the served filter list (`GET /sources/{id}/filters`), the way
  Aidoku does: a tag is clickable when a genre-style filter (`is_genre`
  multi-select, or a select marked `is_genre`/`uses_tag_style`) has an
  option matching it case-insensitively (by option label, falling back to
  `ids`) and submits that filter with the matched option included; a
  creator name is clickable when a text filter with id `author`/`artist`
  exists and submits a `Text` filter value — Aidoku's query-less filter
  search in both cases.
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
- Cross-source migration (`api/migrate.rs`): moves library entries between
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
  `source_order` is the source's own chapter index. `GET /api/v1/library`
  filters by `search`, `categoryId`, `filter` and the tri-state pairs
  `contentRating`/`contentRatingExclude` (comma-separated manga-level
  ratings 1..=3, `IN` / `NOT IN` on `manga.content_rating`) and
  `source`/`sourceExclude` (source ids, `IN` / `NOT IN` on
  `manga.source_id`): within one dimension the include set is an OR and the
  exclude set a NOT-OR, dimensions combine with the other filters via AND
  (Aidoku's library filter semantics). Empty or whitespace-only values are
  treated as absent; invalid tokens answer 400. Chapter reconcile
  (`source/mod.rs:reconcile_chapters`) keeps a chapter the source stopped
  listing when it has download rows, and deletes it otherwise. Manga rows
  persist the source `viewer` (aidoku `Viewer` as int) and chapter rows the
  source `thumbnail`; a source declaring exactly one language fills missing
  chapter languages with it on reconcile. A non-zero
  `manga_user.viewer_flags` is the user's viewer override: API responses
  report it as the manga `viewer` while the source value stays stored.
- Incognito reading (`api/reader.rs`): "history" is `chapter_user.last_read_at`,
  stamped whenever `lastPageRead` is written; the history page and both OPDS
  history feeds select rows where it is non-null. The global
  `incognito.enabled` setting or a per-manga `manga_user.incognito` flag
  (Aidoku's pause-history analogue) pauses the stamp: a progress write still
  saves `last_page_read`/`is_read` but leaves `last_read_at` untouched — new
  reads never enter history and existing stamps are kept. `GET`/`PATCH
  /api/v1/manga/{id}` carries `incognito` (the PATCH upserts the `manga_user`
  row, so the flag also works for manga not in the library), and the backup
  document round-trips it, defaulting to off for backups predating the field.
  The web UI exposes the global toggle in Settings (`SettingsPage.tsx`) and
  the per-manga toggle as an icon button in the detail header
  (`MangaDetailPage.tsx`).
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
  falls back to the original bytes, so a request without `width` — and the
  downloads and OPDS-PSE paths, which never pass one — stays byte-identical
  to the unmodified pipeline. Decode+resize runs on a blocking thread with
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
  as a text page (`webui/src/pages/ReaderPage.tsx:pageText`); OPDS and
  downloads keep reading only the page `content`.
- Image processors: when a source exports `process_page_image` /
  `process_cover_image`, `fetch_page_image` / `fetch_cover_image`
  (`source/mod.rs`) pass the fetched response through the actor
  (`SourceHandle::process_page_image`) and replace the bytes with the
  returned PNG — for the reader, OPDS, downloads and the cover endpoint
  alike. Per the Aidoku contract the actor hands the processor the raw
  response body without decoding it first (`ImageRef.data()` returns those
  raw bytes), so sources with encrypted-at-rest images can decrypt them;
  width/height imports on that raw image report invalid-image, as Aidoku
  has no pixels either. Cover caching is unchanged: the processed bytes
  are what the on-disk cache stores.
- Browse/home covers are proxied through
  `GET /api/v1/sources/{id}/image?url=…` (`source/cover.rs:get_source_image`):
  only http/https URLs are accepted and the response goes through
  `fetch_cover_image` (so a declared cover processor runs) without a disk
  cache, so a hotlink-protected host sees the source's `get_image_request`
  headers exactly as on the detail and reader paths.
- Manga covers (`source/cover.rs`): `GET /api/v1/manga/{id}/cover` serves a
  disk cache (`covers_dir/<id>` plus an `<id>.content-type` sidecar) and
  refetches `manga.cover_url` through `fetch_cover_image` only when the
  file is missing or `last_fetched_at` is newer than its mtime; OPDS reads
  the same cache via `source/mod.rs:cover_bytes`.
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
- Library updates (`update/mod.rs`): same scheduling shape as downloads —
  serial per source, at most 3 sources at once — broadcasting progress as
  `UpdateEvent`s. Category `include_in_update` is a three-state filter.
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
  exists because wasmer linear memory only grows — eviction is the one
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
  direct, per-domain relay, or a global proxy, decided per host. When
  `net.trawl_proxy` is set, a direct request answered 403/503 with Cloudflare
  headers retries once through the trawl forward proxy (MITM CA in
  `net.trawl_ca`), ahead of the 429/5xx backoff — in the source net imports
  (`crates/wasm/src/imports/net.rs:perform`) and in the server's own fetches
  alike; the challenged host is then remembered and every later request for it
  also goes through trawl, because a `cf_clearance` is bound to the user agent
  that solved it and Sumidoku's own client would replay it with a different one
  (trawl keeps the clearance and its browser user agent together and replays
  both over plain HTTP). `net.direct_domains` opts a host out of escalation,
  `net.relay_domains` routes a host through trawl from the first request.
  Set-Cookie on the solved response still lands in the source's cookie jar.
  These server-side fetches — covers, page images and zip pages
  (`fetch_remote` / `http_get`) — bypass the wasm net host, so they attach
  the same per-source `Accept-Language` default themselves (via
  `RoutedClients::get_with` / the image-request builder); a source that set
  its own `Accept-Language` on an image request keeps it, and a source-set
  `User-Agent` likewise wins over the global one
  (`RoutedClients::request_with`, insert-only-if-absent) rather than being
  appended as a second value — Cloudflare reads only the first and 403s a
  duplicated header. Their response bodies are capped as well: covers and
  page images share the net host's `net_body_max_mb`, while zip pages —
  a whole chapter packed into one archive — fall under the higher
  `net_zip_body_max_mb` (default 128 MiB).

## WASM sources (`crates/wasm`)

Aidoku source packages (`.aix`, zip) run in wasmer. One actor per source
(`runtime/actor.rs`) owns the store (`runtime/store.rs`), per-source rate
limiting (`runtime/rate_limit.rs`), and the host functions sources import
(`imports/`), including the HTML DOM helper (`html_dom.rs`) and the Boa
JavaScript engine (`js_engine.rs`) behind the `js` context functions — one
isolated context per ABI descriptor, time- and loop-limited, with no host
access beyond what the ABI exposes. Each instance runs under the `Limits`
from its `SourceConfig` (`runtime/limits.rs`): a metering fuel budget per
export call, a per-call timeout that fuses and restarts the instance, and
a linear-memory cap enforced through custom wasmer tunables
(`LimitedTunables`) — the server sets these from its own config when it
spawns an instance, defaulting the memory cap to 128 MiB.
The net host (`imports/net.rs`) aligns request behavior with the
Aidoku/URLSession host. Response bodies are buffered under a
configurable cap (`net_body_max_mb`, default 32 MiB, decoded bytes —
gzip is counted after decompression): a declared content-length over
the cap fails fast, otherwise streaming reads stop at the cap, and an
oversized body fails the request with the ABI's generic request error
while naming the limit in the source log. `html()` decodes the body by BOM, then the
Content-Type charset, then a `<meta charset>` sniff of the first 1024
bytes, falling back to UTF-8 with replacement (encoding_rs) — a
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
`webview_get_cookies`, `webview_delete_cookie` — have two real-browser
backends (`browser.rs`, `browser/cdp.rs`, `browser/trawl.rs`), mutually
exclusive (configuring both fails startup). With neither configured they
return `-1` (`JsError::MissingResult`; the ABI has no "unsupported" code)
and log the function name and reason, so a source relying on them sees the
calling operation fail with a diagnosable line in the source log. With
`net.browser_ws` pointing at a user-supplied Chromium CDP endpoint (e.g. a
browserless container; Sumidoku ships no browser) they drive that browser:
`webview_load` navigates a page and waits for the page load event — the
same completion signal as WKWebView's `didFinish` on iOS — and the cookie
functions read and edit the cookie store. Every webview rid maps to one
isolated browser context, created on first use and disposed when the rid
is destroyed, so sessions never bleed across webviews; a dead connection
is retried on the next call and the browser-side contexts it orphaned are
disposed after the reconnect. `webview_eval` keeps the Boa page model on
this backend.
With `net.browser_trawl` pointing at a trawl API URL instead, every webview
rid maps to one keepPage session named `sumidoku-<source>-<rid>`, created
lazily on first use and deleted (`DELETE /sessions/:id`) eagerly on destroy
— trawl caps concurrent sessions per instance (`BROWSER_SESSION_MAX_ENTRIES`,
default 4), so sessions are never pooled. `webview_load` navigates via
`POST /scrape` (GET only, same as the CDP backend), `webview_get_cookies`
reads `GET /sessions/:id/cookies` (HttpOnly included), and `webview_eval` /
`webview_eval_async` both run through `POST /sessions/:id/eval`, which
evaluates in the kept page's main JS context and awaits returned promises —
scalar results come back in their JS string form, objects/arrays as compact
JSON. A 404/410 (session expired, its browser replaced, or retired by an
eval timeout — trawl sessions are in-memory and lost on restart) recreates
the session under the same id and retries the operation once; a duplicate
id on create is adopted (a restarted Sumidoku reuses rid numbers while
trawl may still hold the old session); 409 busy, 429 (registry full) and
503 are single-operation failures like a navigation timeout.
`webview_delete_cookie` has no trawl endpoint and is explicitly unsupported
on this backend.
Cookies from the browser session flow into
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
sources from `Payload/` inside the zip without extracting. Source settings
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

With `net.browser_ws` configured, `method: "web"` logins also have an
interactive path that needs no devtools (`api/login.rs` browser endpoints +
`browser_login.rs`). `GET .../settings` reports it as
`capabilities.browserLogin`, and the web UI then shows "open the login page
in the browser" next to the paste box. `POST .../login/web/browser`
validates the setting and host exactly like the paste path, then opens an
isolated browser context on the sidecar (one CDP connection per session),
sets a fixed 1280×800 viewport and navigates to the resolved login URL; the
response is a random session handle plus the URL and expiry. The user
watches and drives the page inside the web UI itself:
`GET .../login/web/browser/{session}/stream` relays CDP screencast frames
(jpeg, latest-wins over a watch channel) out and mouse/keyboard/text input
back in over one authenticated WebSocket — the sidecar therefore needs no
UI of its own and stays on the private network; any CDP-capable Chromium
works (the browserless OSS `/debugger/` is a code workbench whose session
links open plain DevTools, and its LiveURL is a premium feature, so neither
is used). Sessions live in memory only, pinned to the resolved host and the
setting key, and are reaped after 10 minutes (the sidecar's own connection
timeout must be at least that); a server restart orphans the browser-side
contexts, which the sidecar reclaims. `POST .../login/web/browser/complete`
harvests the context's cookies for the login host (sub- and parent-domain
matches included; cross-domain SSO cookies stay out and keep needing the
paste path) plus the setting's declared `local_storage_keys` from the live
page (best-effort), then runs the exact paste pipeline
(`run_web_login_pipeline`: `<key>.keys/.values` → `handle_web_login` →
marker → `source_cookie` → restart) and disposes the context; completing
with no cookie for the host answers 400 instead of marking anything.
`POST .../login/web/browser/cancel` disposes and is idempotent. Without
`net.browser_ws` all three POST endpoints answer 501 `unsupported` and the
capability flag is false — `net.browser_trawl` does not count as a browser
here: the session API has no screencast or input surface, so interactive
login stays CDP-only. The interactive machinery (viewport, screencast,
input dispatch, page localStorage read) extends the same `CdpClient` trait
the webview backend uses (`crates/wasm/src/browser.rs`), so tests drive it
with the same kind of mock connector, injected via
`build_app_with(config, connector)`.

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

Vite 7 + React 19 SPA, TypeScript. `src/pages` are the routes,
`src/components` shared pieces, `src/api` the REST client,
`@tanstack/react-query` for server state. `npm run check` is
`tsc --noEmit`; there is no test suite. The build lands in `webui/build`
and the server embeds it (see above). When authentication is on, the
client (`api/client.ts`) sends the stored session token as a bearer
header, media/websocket URLs carry it as an `access_token` query
(`withToken`), and any `401` clears the token and lands on `/login`.

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
  does not line up one to one, and the host `escape` / `unescape`
  functions cover only `&amp;` / `&lt;` / `&gt;` where SwiftSoup's
  `Entities` covers the full table.

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
