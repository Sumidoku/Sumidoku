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
- Built-in local source (`local/mod.rs`, migration `0011_source_kind.sql`):
  `source.kind` is `aidoku` for every installed package and `local` for the
  built-in `local.local` row the migration seeds; the row cannot be deleted
  (400), is excluded from repo management and metadata backfill, and every
  wasm-capability endpoint (filters, listings, home, deep link, settings,
  login) answers 404 `unsupported` for it. There is no index table:
  browsing walks the local roots recursively in real time — config
  `local_dirs` (`SUMIDOKU_LOCAL_DIRS`, semicolon-separated with empty
  segments ignored; the single `SUMIDOKU_LOCAL_DIR` / `local_dir`
  spelling is a one-item fallback, default `[<config_dir>/local]`),
  overridden at runtime by a non-empty `local.dirs` setting (a JSON
  string array written from the Settings page, no restart needed;
  `state.rs:local_dirs` resolves the two layers, and a wrong-shaped
  stored value counts as unset) — `POST
  /sources/{id}/search` (`api/mod.rs:search_source`) branches for
  `kind = local` to `scan_manga_dirs`, which discovers manga top-down
  with nearest-qualifying-ancestor absorption: a directory directly
  holding chapter archives qualifies (the Komga trigger), one with no
  archive anywhere below qualifies by the backward-compatible relaxed
  rule (nested levels included), anything else is a pure container and
  discovery descends; a claimed manga absorbs its subtree, so nested
  series never appear as manga of their own and a flat single-level
  library scans byte-identically to the pre-nesting behavior. A
  configured oneshots name (config `local_oneshots_dir` /
  `SUMIDOKU_LOCAL_ONESHOTS_DIR`, overridable at runtime by the
  `local.oneshots_dir` setting, blank = off) outranks both passes: a
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
  order (extracted in-memory) → a `series.json` sidecar (flat schema:
  title, author/authors, artist/artists, description, genres, status
  enum; a broken file warns and falls through) → the first EPUB chapter
  whose OPF parses (`dc:title`/`dc:creator`/`dc:description`/`dc:subject`;
  no artist/status counterpart) — and feeds the same
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
  scope) instead takes its page list from the OPF spine restricted to
  image manifest items (`local/epub.rs`: the package document is
  located through `META-INF/container.xml`, hrefs resolve against the
  OPF location), falling back to the natural-sorted archive images when
  the OPF is missing or unparseable, and a page's mime is the
  OPF-declared media type with the extension mapping as fallback.
  `load_pages`
  (`source/mod.rs`) lists the chapter from the filesystem for local manga
  and stores `{"Local": entry}` page rows; the reader (`api/page.rs`) and
  OPDS-PSE read image files directly and extract single archive entries
  (`by_name`) through a 16-handle `ZipArchive` LRU (`local/zip_cache.rs`),
  with the downsample pipeline untouched. Covers reuse the disk cache with
  the byte source switched to the fallback chain (`local::resolve_cover`):
  `cover.*` sidecar in the manga directory → first page of the chapter
  list's last chapter (Suwayomi's `chapters.last()`, the lowest-numbered
  one) → 404 placeholder. `?refresh=1` on the chapters endpoint re-scans;
  the library update scheduler treats the local scan as the source call.
  Auto-download skips local manga (files are already local), and the web
  UI hides the download buttons for `kind = local`, shows a manual refresh
  button on the chapter list, and edits the directory list in Settings:
  `GET /api/v1/settings` injects the resolved list as `localDirs`
  (camelCase, distinct from the `setting` table's dotted keys), `PUT`
  skips that injected key, and `PUT` on `local.dirs` stores the override
  (a string array of non-empty paths, otherwise 400) which takes effect
  without a restart. The oneshots name follows the same pattern:
  `GET` injects the effective value as `localOneshotsDir` (null when
  neither layer names one) and `PUT` on `local.oneshots_dir` validates
  a string (an empty one clears the override). RAR/CBR, PDF and 7z are
  out of scope (no mature pure-Rust
  readers, or an external runtime library).
- Built-in OPDS source (`opds_source/mod.rs`, registration endpoint
  `api/opds_source.rs`): `source.kind = opds` turns an OPDS catalog —
  Komga, Kavita, Calibre-Web — into a first-class source, multi-instance
  from day one. Both wire formats are accepted, detected per response by
  Content-Type with a JSON body sniff for mislabeled servers: 1.2 (Atom)
  through `parse_feed`, 2.0 (JSON) through `parse_feed_json`, both filling
  the same `Feed` model. `POST /api/v1/sources/opds` (admin) takes
  name + catalog URL + optional basic credentials; keys are `opds.<slug>`
  with a counter on collision (`opds.komga`, `opds.komga-2`), credentials
  land in `source_setting` (`username`/`password`), instances are
  deletable (cascade) and excluded from repo management and metadata
  backfill (`kind = 'aidoku'` scope), and every wasm-capability endpoint
  answers 404 `unsupported` exactly like the local source. Capability
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
  gets an admin-only registration dialog (`BrowsePage.tsx`
  `OpdsRegisterModal`) and hides the home/settings buttons on opds cards.
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
- Authentication (`auth.rs`): the `user` table is the only credential
  carrier. Login is username + password (`POST /api/v1/auth/login`),
  answered with a per-user session token (`auth_token.user_id`, SHA-256
  at rest, 30-day rolling expiry) and the `{id, username, role}`
  identity. Auth is off until the built-in admin — user 1, seeded as
  `admin` — has a password: seeded from the config (`auth.password`, or
  the legacy `basic_password` alias) on every boot, or stored on the user
  row by a previous boot or password change. With auth off every request
  acts as the built-in admin, so a no-password deployment behaves exactly
  like a single-user one. With a password, `require_auth` guards all of
  `/api/v1` except `/health`, `GET /api/v1/version` (the workspace
  `Cargo.toml` version, for the web UI's About section) and the `auth/*`
  endpoints: the bearer
  credential (header, or `access_token` query for `<img>`/WebSocket
  clients) resolves against the configured `auth.api_keys` (SHA-256
  fingerprints in memory, acting as the built-in admin) or a session
  token, and the resolved `AuthUser` rides request extensions so every
  handler scopes reads and writes by `auth_user.id`. Failures share one
  `401 {"error":{"code":"unauthorized"}}` shape. Passwords are
  argon2-hashed; `PUT /api/v1/auth/password` changes the caller's own
  (current password required) and revokes all of that user's session
  tokens, and the config-set password always re-seeds user 1's stored
  hash at boot, so it wins over UI-changed passwords after a restart.
- User management (`api/users.rs`): admin-only `GET`/`POST /api/v1/users`
  and `PATCH`/`DELETE /api/v1/users/{id}`, part of the `require_admin`
  route group (`api/mod.rs`) that also covers sources and repos, source
  settings and login, and server-side backup restore — `require_admin`
  reads the role `require_auth` injected and answers 403 for a reader.
  Roles are `admin` (everything) and `reader` (own user-state only);
  `PUT /api/v1/settings` additionally gates per key, so a reader can only
  write the user-scoped keys (`USER_SCOPED_SETTING_KEYS`). Create takes
  `{username, password, role}`; PATCH applies the present fields, and a
  password reset revokes that user's session tokens (same semantics as
  self-service change). The list never carries `password_hash`. Delete
  cascades the user's state rows (library, progress, categories,
  downloads, tokens) through foreign keys and removes the `user.<id>.*`
  settings namespace by hand; downloaded chapter files stay — they are
  shared artifacts at deterministic paths. User 1 cannot be deleted
  (400). All four endpoints answer 400 while authentication is disabled:
  multi-user requires a password first.
- OPDS catalogs: 1.2 (Atom) at `/opds/v1.2` (`opds/mod.rs`) and 2.0 (Readium
  Web Publication JSON) at `/opds/v2` (`opds/v2.rs`). The 2.0 feed builds its
  URLs from the 1.2 cover/file/stream endpoints; the two share auth, paging
  and query helpers from `opds/mod.rs`. Basic auth applies to the catalogs
  when authentication is enabled and verifies real usernames and passwords
  (before v0.16.1 any non-empty username passed with the site password);
  the resolved `AuthUser` scopes every feed to that user's own library,
  progress and downloads. Repeated verifications hit a one-entry cache so
  per-page image requests don't pay argon2 each time.
- The web UI is served from the binary: release builds embed `webui/build`
  (`webui_assets.rs`, rust-embed); debug builds read it from disk.
- Library model (`model.rs`, `api/library.rs`): manga → chapter → page rows
  hold shared content; user-state is per-user — `manga_user` (library
  membership, viewer override, incognito, sort key) and `chapter_user`
  (progress, read/bookmark/download flags, history stamp) keyed
  `(row id, user id)` — and every REST query and write binds the
  request's `AuthUser.id`, so one user's library, progress and history
  stay invisible to another. `source_order` is the source's own chapter
  index.   `GET /api/v1/library`
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
  `unreadCount` and the `sort=unread` ordering count with default-unread
  semantics — a chapter with no `chapter_user` row is unread (the ad-hoc
  `filter=unread` keeps its own flagged-rows-only shape).
- Filter groups (`api/filter_groups.rs`, migration `0013_filter_group.sql`):
  Aidoku-style saved rule sets. A `filter_group` row is per-user
  (`unique (user_id, name)`, `sort_order`), its `filter_group_rule` rows
  cascade with it (`unique (group_id, type, value)`; flag rules carry a
  NULL value, so the write path — not the index — dedupes them, SQLite
  treating NULLs as distinct). `GET`/`POST /api/v1/filter-groups`,
  `PATCH`/`DELETE /api/v1/filter-groups/{id}` (PATCH renames and/or
  replaces the rule set wholesale) and `PATCH .../reorder` follow the
  category handlers' shape. Rule types: `downloaded` (EXISTS
  `chapter_user.is_downloaded`), `started` (EXISTS
  `chapter_user.last_read_at`), `completed` (`m.status = 2`),
  `has_unread` (default-unread: EXISTS a chapter with no read
  `chapter_user` row — never the ad-hoc `filter=unread` shape, which
  would drop never-opened and partially-read manga), and the valued
  `source` / `content_rating` / `category` (one id per row). Evaluation
  matches Aidoku's filter loop (`compile_group`): includes OR within one
  valued dimension, excludes apply per value, dimensions and flag rules
  AND — "all rules ANDed" holds only across dimensions.
  `GET /api/v1/library?groupId=` compiles one group into the same WHERE
  clause, ANDed with the ad-hoc parameters; another user's group 404s
  like another user's category. The web UI manages groups in Settings →
  Filter groups and renders them as a single-select chip strip above the
  category chips; two display settings (user-scoped `library.*` keys,
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
  see below), which survive the pass unread. Manga rows
  persist the source `viewer` (aidoku `Viewer` as int) and chapter rows the
  source `thumbnail`; a source declaring exactly one language fills missing
  chapter languages with it on reconcile, and a whitespace-only chapter title
  folds to NULL, matching the Aidoku host's `""` → nil normalization. A non-zero
  `manga_user.viewer_flags` is the user's viewer override: API responses
  report it as the manga `viewer` while the source value stays stored.
- Incognito reading (`api/reader.rs`): "history" is `chapter_user.last_read_at`,
  stamped whenever `lastPageRead` is written; the history page and both OPDS
  history feeds select rows where it is non-null. A per-user
  `incognito.enabled` setting — stored as the namespaced key
  `user.<id>.incognito.enabled`, with `GET`/`PUT /api/v1/settings`
  translating between the bare key and the caller's namespace — or a
  per-manga `manga_user.incognito` flag
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
  The web UI exposes the per-user toggle in Settings (`SettingsPage.tsx`) and
  the per-manga toggle as an icon button in the detail header
  (`MangaDetailPage.tsx`).
- Backup export/import (`api/backup.rs`): `GET /api/v1/backup` answers one
  JSON document (a `Content-Disposition` attachment) scoped to the
  requesting user: their library rows and metadata, categories with
  membership and reading progress, plus source identity (key, name, version
  and a best-effort repo URL). An admin's document also carries app
  settings and source settings; a reader's is trimmed to self-data — only
  their user-scoped settings keys, no source settings. `POST /api/v1/backup`
  restores it into the requesting user's own state, optionally limited by
  `?sections=` to some of `library`, `categories`, `progress`, `settings`,
  `sourceSettings` (the last two require the admin role). Restore upserts by natural key — source
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
  document an admin's GET handler serves.
- Aidoku backup import (`api/aidoku_backup.rs`):
  `POST /api/v1/import/aidoku` accepts a `.aib` upload — a binary plist of
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
  `POST /api/v1/import/suwayomi` accepts a Suwayomi-Server or
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
  export (global settings and source settings included), unaffected by
  the per-role trimming `GET /api/v1/backup` applies to a reader. The
  first tick doubles as the startup overdue check. Pruning keeps the newest
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
  sources, with a page-level fetch cap inside a chapter. Download rows are
  per-user (`download` primary key `(chapter_id, user_id)`): enqueue,
  pause, cancel and the list all bind the requesting user, and WebSocket
  row events are filtered per connection user, so one user's queue stays
  invisible to another. The artifact files stay shared at deterministic
  paths — a second user queueing an already-downloaded chapter lands on
  `finished` directly, no repack. Both normal completion and shared-artifact
  settlement commit the finished `download` row and the user's
  `chapter_user.is_downloaded` flag in one `BEGIN IMMEDIATE` transaction;
  either write failing rolls both back, and the `finished` event is emitted
  only after commit. A row's stored `path` resolves through
  `download::product_path` — absolute is used as-is, relative joins
  `downloads_dir` — and every read/delete site (`api/download.rs`,
  `opds/mod.rs`) goes through it.
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
  content area: two users downloading the same chapter render the same
  absolute file, the later one overwrites with identical content or
  settles onto it without repacking, and deletion removes the file only
  once no row references it (an already-missing file counts as
  deleted). Metadata edits only steer future downloads — earlier
  products stay where they were moved.
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
  alike. The processor input follows AidokuRunner's `store(value:)`
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
- Library updates (`update/mod.rs`): serial per source, at most 3 sources at
  once, broadcasting progress as `UpdateEvent`s. The global
  `library.skip_titles` setting defaults to `hasUnread`, `completed` and
  `notStarted`; `library.refresh_metadata` defaults to false, and
  `library.update_interval_hours` falls back to config
  `update_interval_hours` (default 0 = never). The setting takes precedence
  when present. `GET /api/v1/settings` returns these effective values, and
  only admins can write them. Selection keeps `update_strategy = Never` and
  the category `include_in_update` tri-state (with any INCLUDE, only titles
  under INCLUDE categories run; otherwise everything not in an EXCLUDE
  category runs), Sumidoku's counterpart of Aidoku's
  `excludedUpdateCategories`. The skip predicates are evaluated against each
  user's own `chapter_user` rows: `hasUnread` skips when any chapter has no
  read row for that user (a missing row is unread), `notStarted` skips when
  the user has no read chapter (including a title with no chapters), and
  `completed` skips status 2. A source schedule
  (`next_update_is_fallback = 0`) skips while its stored `next_update_time`
  is in the future. A fallback schedule (`= 1`) instead tests
  `chapters_last_fetched_at + the current effective interval`, so shortening,
  lengthening or disabling the interval takes effect without rewriting old
  manga rows; interval 0 is immediately due. The stored next update time is
  still advanced after success for the next source round trip. A manual
  `POST /api/v1/update/fetch` is owned by the triggering user; the scheduled
  run fetches the union of all users' libraries with one source call per
  manga, filtering auto-download per user.
  The scheduler checks due every 60s and reloads the interval instead of
  capturing it at construction, so setting changes need no restart. Its
  memory-only `last_scheduled_run_at` is stamped only by a scheduled union at
  start and finish; due uses that timestamp, or scheduler construction time
  when no scheduled batch has run. Manual batches still stamp the summary's
  `last_run_at` for reporting, but never the scheduled-only clock, so they
  cannot postpone the union. A restart waits a full configured interval.
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
  and the summary carry the fetch's owner, and both the WebSocket stream and
  `GET /api/v1/update/summary` present them scoped to the requesting user
  (`UpdateSummary::visible_to`): someone else's manual fetch reads as idle,
  the union run is visible to everyone.
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
login page takes username + password, the client (`api/client.ts`) sends
the stored session token as a bearer header, media/websocket URLs carry
it as an `access_token` query (`withToken`), and any `401` clears the
token and lands on `/login`. The settings page shows the current
account, and admins manage users on the Users page
(`webui/src/pages/UsersPage.tsx`).

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
