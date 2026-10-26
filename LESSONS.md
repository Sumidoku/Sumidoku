<!-- reviewed-through: 3ed5f79 (2026-10-26) -->
# Lessons from merged work

Sumidoku's code is written, reviewed and released by agent sessions. What
went wrong is kept here as a rule, then why, then the evidence. Read this
before changing code. A lesson seen again on a later day moves into
[docs/code-standards.md](docs/code-standards.md).

**OAuth acceptance must enable authentication and cross the real site boundary.**
A callback flow tested only with authentication disabled cannot reveal that
SameSite policy or route guards reject the returning browser. Test the cookie
contract and the authenticated start/callback, and exercise a real cross-site
navigation for redirect-based login flows.
- Evidence: 2026-10-26, #454's auth-disabled tracker flow passed while an
  authenticated deployment returned 401 because its Strict cookie was absent
  on the cross-site return. The authenticated regression failed on Strict and
  passed on Lax; a real cross-site click returned 401 with Strict and reached
  nonce handling with Lax. Restoring Strict made the regression fail again.

**Acceptance criteria need a requirement source, not just a plan reference.**
An agent's own staging note is a proposal, not proof that follow-up work is
required. Before treating an optional idea as a release blocker, trace it to
the issue's accepted scope or an explicit new request; otherwise drop it.
- Evidence: 2026-10-26, #35's original tracker scope did not request a
  configurable OAuth relay root. A later agent-written plan mentioned one, and
  follow-up work mistook it for a required gap and began implementation. The
  51 inserted lines were reverted before commit, and the issue was corrected
  to leave only release-artifact credential verification.

**Canonical identity must be derived after canonical normalization.** When
names are meant to identify the same entity, normalize before lowercasing and
separator stripping so equivalent spellings collide by design rather than
receive separate identities.
- Evidence: 2026-10-26, #414's registration treated NFC `Café` and NFD
  `Cafe\u0301` as different slugs, returning `local.cafe` instead of the
  collision key `local.café-2`. Applying NFC before the existing slug loop made
  the registration and counter regression pass without changing CJK behavior.

**Refreshing remote state must not turn unchanged form values into edits.**
Keep only explicit user overrides and derive visible values from the latest
remote state plus those overrides; remove an override when the user restores
the remote value. Diffing the whole form against a moving response makes old
values look like new edits and can regress the remote entry on a binding-only
save.
- Evidence: 2026-10-26, #35's review found the form initialized from one
  response while a background refresh replaced the diff baseline. A save with
  no remote edits could then POST stale progress. The fresh-response merge
  regression passes only explicit edits and leaves a binding-only update empty.

**Read a React event's values before creating a deferred state updater.** A
functional updater may run after the event's `currentTarget` has been cleared;
capture the value in the handler and pass that value into the updater.
- Evidence: 2026-10-26, #35's tracker edit form read `event.currentTarget`
  inside four `setValues` updaters. Changing a remote field crashed the page
  with `Cannot read properties of null (reading 'value')`. The regression
  reported all four old handlers, then passed after they became immediate
  arguments; saving all four fields sent only the edited values.

**Build the Web UI before Rust tests and builds, never concurrently.**
`rust-embed` and `embedded_webui` both read `webui/build`; Vite replacing
hashed assets can make the embedded HTML and the on-disk index disagree.
- Evidence: 2026-10-26, #35 validation ran `npm run build` and `cargo nextest`
  together. `embedded_webui` failed at `library_reader.rs:1282` because the
  served HTML and disk index named different hashed JS bundles. The ordered
  Web UI build → workspace suite → clippy → release build pipeline then passed
  1060 tests with 2 skipped, clippy with `-D warnings`, and the release build.

**Dropping a test fixture's TempDir unlinks its database file; lazy pool
connections then fail at a wandering point.** A `let TestApp { app, .. }`
destructure drops the TempDir immediately: already-open connections keep
working off the unlinked file, but the first query that needs a fresh
connection fails with "unable to open database file" — wherever that happens
to land. Bind the field (`tmp: _tmp`) so the directory outlives the test.
- Evidence: 2026-10-26, #35's tracker API tests. The same test failed at
  three different spots across runs (logout, update, ledger, each a 500) as
  each fresh connection hit the unlinked file; binding `tmp` made all four
  pass deterministically.

**A version bump is incomplete without the lockfile.** `Cargo.lock` records
every workspace member's version, so bumping only `Cargo.toml` leaves the
tree inconsistent and CI's `cargo clippy --locked` rejects it. Sync the
lockfile in the same commit — `cargo metadata` is enough, no build needed.
- Evidence: 2026-10-26, the 1.11.0 version PR (#440) changed only
  `Cargo.toml` and lint failed in 22s with `error: cannot update the lock
  file ... because --locked was passed to prevent this`. Amending with the
  synced `Cargo.lock` (the two workspace version fields) fixed it; the
  1.10.0 (#435) and 1.9.0 (#409) bumps both already carried the lock.

**Filesystem-time fixtures must set the exact mtimes they depend on.** A scan
keyed by directory mtime can pass alone and miss a change under load when
creation and mutation land in the same timestamp tick. Advance the exact file
or directory mtime to known values; do not rely on wall-clock progression or
sleeps.
- Evidence: 2026-10-25, #433's workspace run failed
  `refresh_rescans_added_and_removed_chapters` with `Bool(false)` because the
  refresh could not see the chapter removal. Setting the manga directory to
  successive +2/+4/+6-second mtimes made add, remove and reappear
  deterministic, and all 59 local-source tests passed.

**A dynamic SQL identifier list is an input-size contract; chunk it below the
bound-variable cap.** A batch endpoint cannot let the caller's selection size
become a 500 through one `IN (...)` statement, and the cap depends on the
SQLite build plus every other bind in the statement. Chunk the ids with room
to spare and test above the actual cap with ids that need no fixture rows.
- Evidence: 2026-10-25, #433's manga expansion used one dynamic `IN` list.
  A 40,000-id selection returned 500 with `too many SQL variables`; 500-id
  chunks made progress, download enqueue and download deletion answer their
  documented zero results, and restoring the oversized chunk made all three
  regressions fail with 500 again.

**An authorizing read and the write it authorizes are one atomic transition.**
A row can be revoked between a SELECT that permits an action and the write
that performs it. Fold the predicate into the guarded write (for example
`UPDATE ... WHERE ... RETURNING`) and take the caller's identity from the
returned row; test the revocation exactly in the check/write window.
- Evidence: 2026-10-25, #394's cancel flow had the same read-then-write race
  that #416's regressions now pin. #407 repeated the shape in API-key
  authentication: a revoked key could authenticate before #408 changed the
  lookup to an atomic `UPDATE ... RETURNING`. Restoring the old lookup made
  the deleted key authenticate again, while the fixed identity check returned
  `owner` rather than the stale `admin`.

**State guards belong before optimized content paths.** A cache, download
artifact or other fast path must not return content before the endpoint checks
the entity's current state. Otherwise list and detail routes can disagree, and
the optimized read becomes a bypass around trash, permission or progress
rules. Seed the fast-path artifact in the forbidden state and prove it returns
the guarded error.
- Evidence: 2026-10-25, #424's downloaded-product page read was combined with
  #406's trashed-chapter guard. The single-page route returned 200 from a
  finished product before the guard while the page list returned 404; moving
  the guard first made the regression return the expected `not_found`, and
  restoring the old order made it 200 again.

**Capture the response model before triggering work that may change it.**
When an endpoint's contract is to return the pre-trigger state and then start
background backfill, read the response rows before enqueueing. A fast worker
can commit between enqueue and the response query, making the first response
depend on scheduling rather than the contract; test with the worker forced
through that window.
- Evidence: 2026-10-25, #406 review found first browse enqueueing a local
  scan before selecting the response. A probe restored that order and returned
  the newly indexed `Alpha` in the first response; capturing the snapshot
  first returned it to the accepted empty response, and the regression passed
  again.

**A deduplication key is not an execution identity; carry a generation through
completion and publication.** When a finished task can requeue under the same
id, old and new executions overlap during cleanup even if their handlers never
overlap. Make finish and cancellation cleanup conditional on that execution,
and publish the exact transition instead of re-reading the mutable row;
otherwise old cleanup can disable cancellation for new work and events can
skip or regress states. Test dispatch-versus-cancel, cross-group requeue, and
terminal-versus-requeue interleavings.
- Evidence: 2026-10-24, #394 review forced those interleavings. Pending
  cancellation returned `running` instead of `cancelled`; cross-group requeue
  let old cleanup remove the new execution's flag, producing `task_finishing`;
  terminal requeue emitted `pending → running → pending → pending`, dropping
  `done`. The probes were removed after reproduction.

**A concurrency test must preserve its precondition with explicit gates, not a
guessed duration.** A fixed sleep lets setup latency consume the state the
assertion expects to inspect. Hold the blocking execution with a notification,
release it only after the required competing transition completes, and await
its cleanup.
- Evidence: 2026-10-25, `a_blocked_group_never_starves_runnable_work` failed
  with `left: "done"`, `right: "running"` after the 64 enqueues outlasted its
  300ms blocker. An explicit gate kept the blocker running until the other
  group finished; the regression then passed 20 consecutive runs and the full
  workspace suite passed 916 tests with 2 skipped.

**Commit before anything can move the working tree; a shared checkout is
not a safe place for uncommitted work.** Another session switching the
branch under you — or any rebase/checkout/stash you didn't run — can drop
or strand uncommitted hunks silently, and the damage surfaces far from the
cause (a struct definition gone while its uses remain). Commit completed
units as soon as they compile, and after any unexpected branch state,
inventory `git status` against what you think is uncommitted before
continuing.
- Evidence: 2026-10-25, the working tree moved from `task-system` to
  `feat/local-source` mid-session (the branch was rebased and pushed by a
  concurrent session); the uncommitted `ChapterMeta` struct and a
  `comic_info` helper vanished while their call sites stayed, caught only
  by a targeted re-check of recent edits before the next compile. The same
  day's `reset HEAD~` churn on `feat/local-source` then dropped this very
  lesson's commit (59948ef), and it had to be re-applied from the session
  transcript.

**Migration files must be byte-stable across platforms: force LF in
.gitattributes.** sqlx embeds each migration's bytes at compile time and
validates them against `_sqlx_migrations` at boot, so a CRLF checkout
(Windows autocrlf) embeds different checksums than an LF-built release
binary — a Windows-local build refuses a Linux-created database, and the
checksum pin test fails on CRLF hosts while CI stays green.
`crates/server/migrations/*.sql text eol=lf` settles both.
- Evidence: 2026-10-25, `db::tests::shipped_migrations_are_immutable`
  failed on a WSL/Linux run over the Windows checkout: migration 1 hashed
  F9D6E6… (CRLF) against pin 0A32F7… (LF); the dev database's stored
  checksum matched CRLF because it was applied by a Windows-compiled
  binary.
 (docs: record shared-checkout and migration-eol lessons)

**A missing state row is meaningful: default semantics must be shared by
every read path.** When absence means unread, counts, sorting and
include/exclude filters must all apply that rule. Joining only explicit rows
makes never-opened entries disappear from inclusion and survive exclusion.
Test both no-row and fully-read states against the same API.
- Evidence: 2026-10-24, ad-hoc unread filters required `is_read = 0` while the
  badge counted missing rows as unread. A fresh manga vanished from
  `filter=unread` and survived `filterExclude=unread`; the regression failed
  with `left: []`, `right: [1]`. The shared left-join rule fixed the real API,
  and removing only that fix reproduced the failure.

**Live values need a fixed slot in reading order, not an auto-aligned
position.** Changing counts and percentages must not push related metadata or
jump to the opposite edge. Put progress next to the date or label it describes,
with space reserved for the final value; tri-state labels also need a stable
icon slot rather than a text prefix that changes their width.
- Evidence: 2026-10-24, chapter download progress pushed metadata and then sat
  detached at the far right. The fixed slot now renders
  `2023-08-18 1 / 2 · 50%` in that order, while include/exclude cycles keep the
  same label position in chapter, library and source controls. Desktop and
  mobile browser checks covered live SSE updates and the collapsed sidebar.

**A cached browser session needs a distinct lost-session outcome and a bounded
recovery path.** A generic transport failure cannot distinguish an expired or
replaced session from a failed request, so the caller keeps reusing the dead
id. Preserve that distinction, recreate the session once, and replay the
original method, headers and body.
- Evidence: 2026-10-24, basic login cached a trawl session id and a 404 became
  a generic request error on every later attempt. The regression failed with
  `recreate the expired session: Request` before the lost-session outcome was
  separated and one recreate-and-replay was added. A real wenku8 package then
  logged in again after forced session deletion with a real trawl browser;
  removing the recovery path made the regression fail again.

**Fallback-bearing settings need three states: absent, explicit blank, and a
value.** An empty field is not interchangeable with null when absence inherits
a fallback but blank disables the feature. Preserve that distinction in both
registration and editors, and test with a non-empty fallback so inheritance and
off cannot accidentally pass as the same result.
- Evidence: 2026-10-24, local-source registration discarded blank oneshots and
  the editor cleared them to null; with config set to `oneshots`, both paths
  re-enabled the rule instead of turning it off. The regression failed with the
  fallback string before the fix and passed afterwards; null and reset still
  restore the fallback.
- Seen again: 2026-10-24, the Local settings help described blank as falling
  back to server config even though it disables the rule. Saving blank read
  back `null`, while reset restored `oneshots`; both translations now
  distinguish the two operations.

**Query-string navigation needs query-aware current-page state.** React
Router's `NavLink` derives `isActive` from the pathname, so destinations that
differ only in their query all become active and receive `aria-current`.
Compare the parameters that select the content when deriving both, and test a
non-default section.
- Evidence: 2026-10-24, the settings sidebar marked all five section links
  active at `?section=downloads`. The regression returned all five hrefs before
  the fix and only the downloads href after matching `section`; browser
  acceptance then showed one highlighted item.

**SSR render tests inherit the machine locale; force the language in the
test.** The web UI's i18n falls back to `navigator.language` when no stored
preference exists, so a render-to-static-markup test asserting English aria
labels fails on a zh-CN host while CI (English) stays green — the suite is
only red where a contributor's machine disagrees with CI. Call
`i18n.changeLanguage('en')` right after loading the module, or keep the
assertions language-independent.
- Evidence: 2026-10-24, internal-source work (#257): source-card-mobile and
  sidebar-layout tests failed on a zh-CN workstation with Chinese labels
  while CI was green; both now force English.

**Event-driven state needs one owner for each transition; a request's success
continuation must not reapply optimistic state that an event may already have
superseded.** A mutation accepted by the server is not proof that the work is
still running when the client continuation resumes. Keep the optimistic state
only when the same ordering channel can guarantee it is still current, and
exercise the fast-completion path in a real browser.
- Evidence: 2026-10-24, global SSE browser acceptance: a 6 ms library update
  emitted `updateComplete`, then `runUpdate`'s `await` continuation wrote
  `running: true` and left the page running indefinitely. Removing that write
  made the summary events the sole owner of running transitions; the regression
  failed when the write was restored and passed after its removal, and the next
  browser run returned to the completed state.
- Seen again: 2026-10-24, manga detail treated any download event as a reason
  to skip the REST seed, dropping existing queued rows. Only a real snapshot
  replaces that seed; pending events merge into it. The wiring regression
  failed on the old guard, and a 15-second delayed response made two queued
  rows vanish before the merge was restored.
- Seen again: 2026-10-25, #425 let a retry response write `queued` back over
  rows SSE had already advanced. `applyRetrySuccess` now updates only rows
  still in `error`; removing that guard made the regression flip downloading
  and finished rows back to queued.

**Dynamic fallbacks must treat an empty string as absent, not only null or
undefined.** Content APIs can legitimately return `""`, and `??` preserves it,
suppressing the fallback just as a real title would. Normalize empty values
before applying overrides and include a blank-content case in browser
acceptance.
- Evidence: 2026-10-24, #359's document-title override used `title ?? null`;
  the example manga detail returned `"title":""`, so the route fallback was
  suppressed and the tab became plain `Sumidoku`. Normalizing with `title ||
  null` restored `漫画 · Sumidoku`; sabotaging it failed `blank content titles
  keep the route fallback`, and the test passed again after restoration.

**Derived state must be invalidated when its producer or input changes;
fresh fixtures cannot prove an upgrade.** When a parser changes, rows
written by the old parser need an explicit migration or versioned rebuild,
and caches holding open files need an identity/version check before every
hit. Recomputing only new requests leaves old rows and old inode handles
serving stale data indefinitely. Start regressions with seeded old rows and
with atomic rename/truncate, not only a clean database or initial open.
- Evidence: 2026-10-23/24, #349 review found old EPUB page rows bypassing
  the new detection until migration 0021 invalidated them; its upgrade test
  failed with two stale rows before the fix. #352 review found ZipCache
  returning old bytes after atomic rename and a stale central directory
  after truncation. File-version checks now reopen changed files, and both
  regressions fail again when invalidation is disabled.
- Seen again: 2026-10-25, #403 downsampled local covers only on a cache miss,
  so original-size artifacts written before the transform stayed eligible
  forever. #411 requires a derivative-variant marker on every local cache hit.
  Disabling that check made the seeded-cache regression return `image/png`
  instead of `image/jpeg`; restoring it rebuilt the artifact at 450×600.

**Type-specific state must come from the entity's actual shape, not its
filename suffix.** A directory named `.epub` is still an image-directory
chapter, so deriving `epub_divina_compatible` from `ends_with` marks it true
on first read even though no EPUB analysis happened. Compute and return the
flag from the same code path that reads the chapter, and include
directory-with-suffix fixtures.
- Evidence: 2026-10-24, #352 review caught `load_pages` turning
  `chapter.epub/1.png` into `Some(true)`. `chapter_pages` now returns
  `(pages, Option<bool>)`, with NULL for directories/CBZ and the actual
  result for EPUB files, and the regression holds NULL after page rebuild.
- Seen again: 2026-10-25, #406 review found restore accepting a regular
  `Alpha.cbz` file after the same-named directory was trashed, recreating a
  pseudo-oneshot because the suffix stood in for the original shape. Persisting
  the scanned kind and rejecting unknown history fixed both shape reversals;
  removing only the original-kind predicate made the regression return 200
  instead of 409.

**Mojibake can enter as valid UTF-8 when a middlebox answers with the
browser's serialized DOM instead of the wire bytes — capture the upstream
response before blaming the decoder.** trawl's MITM proxy is byte-faithful on
its direct tier, but its browser tiers answer HTML requests with
`page.content()`: the headless browser decodes the original bytes by HTML5
rules, so a `text/html` fragment with no charset declaration (18mh's
`/manga/get` chapter list) is read as windows-1252 and every CJK title
becomes `ç¬¬1è©±`-style mojibake, then re-encoded as clean UTF-8 that no
host-side `decode_body` heuristic can flag. Full pages survive because their
`<meta charset>` decodes correctly in the browser, so the signature is
"only fragment-sourced fields garble" (chapter titles garbled, manga title
fine, chapter numbers lost because the `第` prefix never survives). The
decoder was exonerated by fetching the real response with the site's own
`cf_clearance` + matching UA and finding zero invalid UTF-8 bytes; the
garble was introduced by the proxy tier in between. The durable fix belongs
upstream (browser tier should return the navigation body bytes or honor the
original charset); Sumidoku's own mitigation is to stop feeding hosts to the
proxy: a challenged host is now solved once through the trawl scrape API and
the (cf_clearance, solver-UA) pair is replayed direct — byte-faithful and at
direct-request speed, with the proxy kept only as the solve-failure fallback.
- Evidence: 2026-10-23, zh.18mh chapter titles on docker 1.2.0 with trawl
  configured: live `/manga/get?mid=5994&mode=all` verified valid UTF-8 with
  `Content-Type: text/html` (no charset); trawl `browserCapture.ts` returns
  `page.content()` and `server.ts:serveViaScrape` documents browser tiers as
  "the rendered solved DOM"; the host stuck on trawl per
  `routed.rs:challenged_host_sticks_to_trawl`; trawl's own `/v1` endpoint
  returned `data-ct="ç¬¬1è©±"` for wire bytes carrying `第1話`. A speculative
  decode-side fix (sparse-UTF-8 outranking single-byte labels) was validated
  and then reverted once the real mechanism was found upstream of the decoder.

**Full-object settings writes need presence and ownership rules.** A required
nullable field must distinguish a missing field (reject) from an explicit null
(clear). A server-managed field must not be written from a cached full-object
save, or an old snapshot can erase newer state. Legacy partial writes can keep
their own clear semantics, but structured full writes must not replay
scheduler-owned timestamps. Test missing fields, explicit null, and a stale
cached timestamp.
- Evidence: 2026-10-22, #315's structured settings accepted a missing
  `backup.dir` as `None` and could delete the existing override; writing
  `backup.last_run` from the settings cache could clear a newer backup
  timestamp. #317 required nullable fields, ignored `lastRun` on structured
  writes, and added API regressions for missing fields, explicit clears, and
  preservation. Sabotaging each behavior failed its target assertion before
  restoration.
- Seen again: 2026-10-24, `library.last_scheduled_run_at` traveled in the
  structured `extra` map, so saving a stale snapshot could move the scheduler
  clock. `scheduled_library_clock_is_server_owned` exposed the key before the
  guard, and removing only the guard let both write forms replace `456` with
  `123`. The same key also leaked into backup documents and could be restored;
  backup export now omits it and import ignores it.

**Changing sync ownership is a versioned migration, not an origin rewrite.**
Preserve latest tombstones before deleting old origins, then append current
rows and tombstones as local events with versions strictly above every prior
event. Update live row and snapshot versions together, preserve sequence
history, and clear the retry marker in the same transaction. Verify equal-time
conflicts and old-live/new-tombstone replay, not only that a full pull exists.
- Evidence: 2026-10-22, #306's review caught reader-originated shared updates
  and owner-state tombstones being removed by the single-owner migration.
  Removing the version bump failed `shared.modified_at > 5000`; excluding
  owner-state tombstones failed `owner_tombstone.deleted`. Both passed after
  restoration, as did the existing compaction and sync-protocol suites.

**Removing a tenant filter changes reference semantics; it is not a mechanical
cleanup.** When a query distinguishes the current row from other rows, replace
tenant exclusion with the actual entity exclusion needed by the operation.
Deleting a user predicate can make the row being deleted count as its own
reference, or hide references that still need protection. Exercise the actual
delete path, not only the shared-artifact happy path.
- Evidence: 2026-10-22, #306's download cleanup counted the current download
  row as a reference after its user filter was removed, leaving artifacts on
  disk. Four download tests failed until the orphan check excluded the current
  chapter while preserving references from other chapters.

**Bundling a UI library does not make it offline or license-complete.** Check
runtime defaults for outbound requests and every upstream notice, not only the
package's top-level license. Disable remote validation, pin the assets, serve
the notices with the page, and assert those paths in tests.
- Evidence: 2026-10-22, #301's review found that Swagger UI's default validator
  would send the spec URL to validator.swagger.io, and two JavaScript notices
  were missing. `validatorUrl: null` and both notices were added after the new
  assertions failed with a missing setting and a 404; the release browser then
  rendered using local assets only.

**Rekeying an entity is a versioned merge, not a row move.** Aliases locate an
entity but do not decide which state wins. When a stub or target merge changes a
composite key, compare live and tombstone versions on both sides before applying
the winning state. If the winning physical row is newer than its latest event,
write a local event at that same version; otherwise a later intermediate
tombstone can delete the row while the log says it is older. Keep tombstones at
their original keys and include them in alias-linked comparisons. Tests must
continue beyond the rekey with updates on both sides and row-only versions.
- Evidence: 2026-10-22, #298's review found a v600 stub row resurrected over a
  v1000 target tombstone, and a v1000 row with only a v600 event later deleted
  by a v800 tombstone. The regressions failed with `1 != 0` and
  `None != Some(1000)` before per-relation version merging and derived events;
  both passed afterward.

**Moving work to the background changes the consumer contract, not just the
runtime cost.** If the API and UI neither await the result nor subscribe for
it, the first response can stay empty or stale forever. Eventual persistence
in the database does not repair that first render, and polling the database in
tests proves nothing about the API-to-reader path. Background work also needs a
generation guard for refreshes; otherwise keep the producer synchronous.
- Evidence: 2026-10-22, #293's background measurement returned no dimensions
  on the first pages response while the reader had no refetch, and an old task
  could write into refreshed rows. Restoring synchronous measurement and API
  assertions made the contract and retry test deterministic again.

**Tenant-isolation claims start with the schema's key scope, not with an
assumed per-user id.** A row can belong to a user while its id is globally
unique, making the same id impossible to collide across users. Check the
primary key before constructing a failure case, then test the actual isolation
boundary instead of an impossible collision.
- Evidence: 2026-10-22, #287's earlier review used a shared category id across
  two users as the collision example. `category.id INTEGER PRIMARY KEY` rules
  that case out; the corrected coverage verifies independent per-user settings.

**Replacing a persisted flag needs live-database and portable-backup migration
in the same change.** A new setting can work on fresh installs and still drop
existing users' state. Convert old flags in an append-only migration, keep
instance-local ids out of backup payloads, and restore through natural keys.
Tests must replay an older migration state and restore onto a target whose ids
differ, not only an empty instance with matching ids.
- Evidence: 2026-10-22, #287's follow-up found no live migration for legacy
  exclusions and raw category ids in backups. Breaking the migration returned
  `[]` instead of `[1, 2]`; breaking the backup marker returned `1` instead of
  `2`. The fixed round-trip restores the target id when a placeholder already
  occupies the source id.

**A checked marker belongs to the bytes that were checked, not to the logical
page.** A fresh download is a new artifact version and must refresh derived
metadata even when the page row was previously checked. Lazy backfill can stay
unchecked-only, but the download write path must overwrite stale values;
otherwise same-URL image replacements keep the old dimensions forever.
- Evidence: 2026-10-21, #20's review seeded a checked 99x99 value before
  re-download. The unchecked-only path kept it and the regression test failed
  with left=99/right=1; the download-specific overwrite path then restored the
  actual 1x1 dimensions.

**A status and the flags that make it visible are one public transition:
commit them together and fault-inject the later write.** Separate queries
let readers observe the status before its dependent flag, and a test that
waits only for the status becomes flaky. A transaction fixes the production
race; forcing the flag write to fail proves that no partial status commits,
without relying on timing or adding retries to the original test.
- Evidence: 2026-10-21, #281's CI failure read `finished` followed by
  `isDownloaded = false`. Trigger-injected flag failures left a finished row
  before the fix; afterwards the update path stayed `downloading` and the
  shared-artifact upsert left no row.

**An echoed value is not a new value: every upsert path must preserve its
provenance when the value is unchanged.** A derived timestamp and a
source-owned timestamp can have identical values but different scheduling
semantics. Resetting the provenance on every detail upsert lets an ordinary
echo turn a derived deadline into a source deadline, and later interval
changes stop affecting it. Preserve the flag on an exact (including NULL)
echo and reset it only when the value changes; cover both the scheduler's
write path and ordinary detail refreshes.
- Evidence: 2026-10-21, #45's review caught the ordinary-refresh path after
  the scheduler path was fixed. `upsert_echoed_details_preserve_fallback_provenance`
  failed with `(Some(111), 0)` instead of `(Some(111), 1)` before the shared
  upsert preserved the flag.

**`DefaultBodyLimit` does not reject an over-limit multipart body at the
extractor — the limit error bubbles up inside the handler's field reads,
and a blanket `bad_request` mapping silently turns the 413 into a 400.**
axum 0.8's `Multipart` calls `req.with_limited_body()`, so the route's
`DefaultBodyLimit::max` wraps the body stream; the `LengthLimitError`
only surfaces as a `multer::Error::StreamReadFailed` when the handler
drives `next_field()`/`field.bytes()`, and axum maps it to
`StatusCode::PAYLOAD_TOO_LARGE` on the `MultipartError`. A handler that
converts every multipart error with `ApiError::bad_request(...)` answers
400 to an oversized upload. Map by `e.status()` (413 keeps 413, the rest
become 400) on any upload route, and pin it with an over-limit test.
- Evidence: 2026-10-21, #250's `.aix` import (the first multipart
  surface): verified in axum-0.8.9's `extract/multipart.rs`
  (`status_code_from_multer_error`); sabotaging `multipart_err` to always
  `bad_request` turned `import_body_over_limit_is_413` red with
  left=400/right=413.

**The raw chapter API returns newest-first (source_order ASC of a
newest-first source list); never index the raw array as if chapter 1 came
first, and take selection/navigation semantics from Aidoku's source, not
from the array's head or tail.** `GET /manga/{id}/chapters` orders by
`source_order ASC`, and sources (Aidoku wasm and OPDS feeds alike) list
newest chapters first, so the first element is the latest chapter and
the last element is chapter 1. The manga detail read button used
`chapters.find(c => !c.isRead)`, which always returned the newest
unreadable-adjacent chapter: a fresh manga opened its last chapter, and
each re-entry after reading it walked one chapter down from the newest
end. The fix mirrors Aidoku's `MangaManager.getNextChapter` on the
prefs-sorted+filtered list (resume the most recent unfinished chapter,
else scan from the reading-order end, which stays the sort-key minimum
under either display direction), and the 'source' sort branch's
asc/desc semantics were themselves inverted relative to Aidoku — the
"newest on top" comment described the opposite of the code. When a
feature is declared to match Aidoku, open Aidoku's source and transcribe
the algorithm; do not infer it from local array order.
- Evidence: 2026-10-21, continue-reading fix (d9395e8): the button's
  target was wrong for every read state except "exactly one chapter
  unread" and "all read"; the inverted 'source' asc/desc branch had the
  detail page defaulting to oldest-first against its own comment.

**Reimplementing an Aidoku host import means matching the reference behavior
on the exact inputs sources produce, not just the common path — entity
coverage and charset detection both bite.** Sumidoku's `html.unescape`
decoded only `&amp;`/`&lt;`/`&gt;` while sources expect SwiftSoup's full
HTML5 table (`&quot;`, `&#39;`, `&nbsp;` came through as literal text), and
the net host's body decoding trusted the Content-Type charset ahead of
UTF-8, so a UTF-8 page served under a lying `iso-8859-1` label decoded as
windows-1252 — chapter lists showed `ç¬¬1è©±` where Aidoku showed `第1話`.
The HTML `text()`/`attr()` extraction path was already equivalent
(html5ever decodes entities at tokenize time), which hid the gap: it only
surfaces for sources calling `unescape` explicitly or for servers that lie
about the charset. When a host import reimplements an Aidoku/SwiftSoup
behavior, compare against the reference on adversarial inputs (full entity
table, lying or missing charset declarations) before merging.
- Evidence: 2026-10-21, chapter-title garble report: `ç¬¬1è©±` byte-decodes
  to `第1話`; `unescape_str` matched 3 entities; the decode-order fix was
  validated against a lying-header page, a GBK page with its meta past the
  1024-byte sniff window, and BOM-less declared UTF-16.

**`cargo test` cannot link any test binary that pulls the vendored aidoku
rlib on the windows-gnu toolchain (pre-existing, unrelated to local
changes): its `#[link(wasm_import_module)]` externs (`html.text`,
`std.current_date`, `defaults.get`, …) stay unresolved natively — only
`std.destroy` has a host stub, and it hits `aidoku-wasm` as well as
`sumidoku-server` lib tests. `cargo check --all-targets` and clippy still
type-check everything, release binaries build fine (binaries never
reference those symbols), and CI's nextest runs on Linux where the link
succeeds; on Windows, verify behavior-sensitive changes with a scratch
crate against the same dependency versions, or run the tests under WSL.**
- Evidence: 2026-10-21, chapter-title fixes: `cargo test -p aidoku-wasm
  --lib` and `-p sumidoku-server` both failed at link with undefined
  `text`/`get`/`current_date` on a clean tree (git-stash confirmed); the
  new `decode_body`/`unescape_str` logic was validated in a scratch crate
  copied verbatim, and the same sources compiled clean under `cargo
  check`/`clippy`.

**When research shows an issue diverges from upstream, take the
divergence back to the issue before implementing — the issue text can
be wrong.** An implementation merged exactly per the issue text, with
the deviations from upstream flagged in the PR as deliberate
(case-insensitive vs exact, union vs single marker, gates dropped
entirely); the issue author then corrected the issue text itself and
re-fixed the code. The divergence was known before implementation —
one comment on the issue would have let the author correct the spec
before code existed. When the issue author is an active collaborator,
"the issue says X but upstream does Y" goes on the issue and waits,
not in a PR footnote after the fact.
- Evidence: 2026-10-19/20, #183's tap-to-search: research flagged three
  divergences from AidokuRunner (exact case-sensitive match, `is_genre`
  only, declared-flag-wins); #187 merged per the issue text, the author
  corrected the issue description, and #193 reverted all three to the
  upstream semantics.

**Never edit UTF-8 source files through a legacy Windows shell — it
re-encodes them as ANSI and silently corrupts every non-ASCII byte, and
shell mojibake is not proof of damage.** PowerShell 5.1's
`Set-Content`/redirection writes with the system ANSI codepage, so a bulk
regex edit across a few Rust sources turned Chinese comments into garbage
and even joined lines (18 lines changed in a file that needed 1); the
recovery was `git checkout` of the touched files and redoing the edits
with a UTF-8-safe editor. Conversely, a non-UTF-8 console displaying a
healthy UTF-8 file prints the same kind of mojibake, so before reverting
anything on "corruption" evidence, verify bytewise (dump `git show
HEAD:file` through a byte-preserving redirect and read the dump with a
UTF-8-aware reader). Prefer the dedicated edit tool for file edits; if a
shell loop is unavoidable, inspect `git diff` immediately for
unrelated-line churn.
- Evidence: 2026-10-19, source-filter persistence work: one
  `Get-Content -Raw | Set-Content` batch edit corrupted three Rust files;
  a later mojibake scare on committed files was cleared by dumping the
  committed bytes and finding valid UTF-8 — display-only artifact.

**Dropdown menus inside a horizontally scrolling chip bar get clipped and
over-painted — portal them to body, and never leave a second copy of the
pattern unfixed.** An `overflow-x: auto` container computes `overflow-y`
to `auto` too, so an absolutely positioned menu inside it is clipped at
the bar's bounds and painted under later grid items with their own
stacking contexts. Rendering the menu through a portal with `fixed`
positioning computed from the anchor's bounding rect (clamped to the
viewport, closing on Escape and on any scroll) fixes it for good. The
corollary that cost a second bug: fixing the original instance does not
fix the copy — the global-search page had duplicated the pre-fix dropdown
and shipped the same occlusion until both pages pointed at one shared
component.
- Evidence: 2026-10-19/20, Aidoku-style filter chips: the source-browse
  chip dropdown rendered under the cover grid; after the portal fix the
  global-search `TriDropdown` (same copied code) still had the bug and
  was repaired by extracting `ChipDropdown` into `components/`.

**A leftover smoke Chrome owns the debug port, and a string-prefix URL
match hands you its tabs.** When a second headless Chrome launches with
the same `--remote-debugging-port`, the port stays with the first
instance, so `/json/list` answers with the old instance's targets; and
matching them with `url.startsWith('http://127.0.0.1:4567')` also
matches `127.0.0.1:45672` — the driver then "passes" against a previous
session's page. Before launching, kill any Chrome bound to the port
(`lsof -nP -iTCP:<port> -sTCP:LISTEN`), and match targets by parsed
`new URL(t.url).host`.
- Evidence: 2026-10-20, PR #181's library-filter smoke: the previous
  session's Chrome (pid from an older run) still held 127.0.0.1:9222, and
  the prefix match picked its `127.0.0.1:45672/browse/3` tab; the driver
  failed on a missing chip until both were fixed, then ran 30/30 green.

**Destructuring `TestApp` without keeping `tmp` deletes the fixture
directory mid-test.** `tempfile::TempDir` removes its tree on drop, so
`let TestApp { app, .. } = test_app().await` wipes the sources directory
the scan endpoint is about to read; the failure surfaces far from the
cause (an empty source list, then a confusing "source id for X" panic in
the lookup helper). Bind the field — `tmp: _tmp` when unused — in every
test that builds a `TestApp`.
- Evidence: 2026-10-20, PR #179's aggregate-search tests: the two cases
  written with `let TestApp { app, .. }` failed at `source_id_of` with an
  empty scan while the three that kept `tmp` passed; binding `tmp: _tmp`
  (what every existing test already does) turned them green.

**Before writing a file, confirm it does not exist — truncated listings
lie.** A `find | head -20` that cuts off before the target makes an
existing file look absent; writing then silently replaces work that was
never read. Check the exact path (`test -e` / `ls <path>`) before Write,
and when a write does replace something, the diff self-review is the
backstop that catches it.
- Evidence: 2026-10-19, trawl upstream PR germondai/trawl#236 (Sumidoku
  #144): a truncated `find | head` hid an existing 411-line integration
  test file; the agent overwrote it, caught the loss only in the diff
  self-review, restored from git and appended the new tests instead.

**On a machine with an active user, drive smoke UIs by AX action, not
coordinates — and bring your own browser profile.** A synthetic coordinate
click is aborted ("user intervened") whenever the physical cursor moves
mid-action; `AXPress` on the snapshot index lands regardless. The desktop
browser tool is also unavailable from subagent runs ("requires a running
turn"), so a subagent smoke launches a throwaway Chrome with
`--user-data-dir=/tmp/...` against the local server — fully drivable,
nothing in the user's real tabs, killed whole afterwards.
- Evidence: 2026-10-19, PR #150's partial-stream smoke: two coordinate
  clicks on the ⋯ menu were swallowed by real-cursor movement while the
  AX index press opened it first try; the smoke ran in an isolated-profile
  Chrome on 127.0.0.1:3100 and caught the 2-chapter placeholder state
  mid-stream.

**Lazy `<img>` loads never fire in the embedded smoke browser until a
scroll nudge — bisect the environment before suspecting the component.**
The Kimi Code desktop browser's lazy-load intersection observer does not
trigger on its own: `loading="lazy"` images sit silent (no
`onLoad`/`onError`) while eager ones load fine, so a page full of lazy
covers looks broken even when the app is right. Bisect with a static
probe page (plain / lazy / referrerPolicy / container-type), nudge a
scroll to flush lazy loads, and when a failure does survive that,
instrument the component (`document.title` from `onError`) instead of
theorizing. A sibling rule from the same smoke: appending query params
must pick `?`/`&` from whether the base already has a query —
`withToken`'s conditional means the base is bare exactly when auth is
off, so a `&v=` append 404s only in no-auth setups and passes every type
check.
- Evidence: 2026-10-19, issue #135's picker smoke: bisect probes showed
  only the lazy imgs silent; after a scroll the title probe printed
  `COVER ERR /api/v1/manga/1/cover&v=…` — `coverUrl` built an invalid
  URL whenever no token made `withToken` return the bare path, while
  logged-in setups were accidentally correct. The same smoke caught the
  Cover component's `failed` latch (fixed in a0c9722).

**A "black page" in a reader smoke can be occlusion by fixed chrome, not
a missing render — rule that out before suspecting the pipeline.** In
paged mode the first text line starts 32px from the top, directly under
the ~45px translucent (92% + backdrop-blur) fixed toolbar, so a one-line
text or description page looks entirely black while its DOM, layout and
computed styles are all correct. Before treating "nothing visible" as a
render bug, hide the overlay (mid-zone tap) or swap in long content, and
compare against `main` the same way; a DOM text snapshot plus a static
same-CSS probe page bisects CSS-vs-app-state faster than reading styles.
- Evidence: 2026-10-19, issue #134's reader smoke: one-line fixture pages
  looked black in paged mode on the branch AND with main's ReaderPage
  (swapped in via vite HMR); hiding the toolbar revealed the text and a
  200-line probe rendered fully — occlusion, not a regression. Headless
  Chrome showed computed styles all-visible while painting black, which
  only the long-text swap explained.


**`window.open` must fire synchronously inside the click handler; after an
`await` it is popup-blocked.** Browsers grant `window.open` only during
transient user activation — Safari denies it outright once the gesture's
synchronous run ends, so `onClick={() => void fetch().then(d =>
window.open(d.url))}` silently loses the tab in exactly the flow that
fetches a URL before opening it. Open the tab in the handler
(`const tab = window.open('', '_blank')`) and navigate it when the fetch
resolves (`tab.location.href = url`), with a plain `window.open(url)`
fallback when the synchronous open itself was blocked. Only an exercised
browser catches this: type checking passes, and the failure mode is
environment-dependent (one embedded browser mapped the blocked open to a
same-tab navigation, which looked almost correct).
- Evidence: 2026-10-19, PR #132's OAuth login smoke: the 登录 button's
  async `window.open` navigated the SPA tab itself to the authorization
  URL instead of opening a new tab; the sync-open-then-navigate pattern
  is the fix, and the same smoke verified it end-to-end.

**A CDP connection is bound to the runtime that first polled it; drive the
connect on the host runtime.** tokio I/O handles register with the reactor
active at creation/poll time, so a `Browser::connect` driven inside a
client-owned `current_thread` runtime cannot be driven from anywhere
else — when the handler and screencast pump were moved to the server
runtime (needed because a login session is idle between calls, and a
current_thread runtime is only polled during `block_on`), frames arrived
exactly once inside a `block_on` window and every later command hung with
no error. When a connection must outlive `block_on` windows, spawn the
connect itself on the host runtime. Two neighbours of the same bug: a
client that owns a `Runtime` panics in tokio's shutdown assertion if
dropped in async context, so every exit path must release it inside
`spawn_blocking`; and headless Chromium only emits screencast frames on
damage, so starting a screencast does not guarantee an initial frame —
nudge one screenshot.
- Evidence: 2026-10-19, issue #115's real-browser smoke: the pump logged
  one frame (produced by a command inside a `block_on` window), then
  `screencastFrameAck` and every later command hung; moving the connect
  onto the app runtime delivered frames end-to-end. The runtime-drop panic
  surfaced in the same probe (tokio shutdown.rs:51).

**`tokio::time::pause` breaks sqlx pool acquisition — age test data
directly instead of freezing the clock.** With `start_paused`,
`SqlitePool::connect` fails with `Db(PoolTimedOut)` before the first
query, so a paused-clock test cannot even build its database. When a
test needs an aged in-memory timestamp, set it through the struct
(same-module tests can reach private fields) and keep loop-level tests
on a short real tick.
- Evidence: 2026-10-19, PR #128's idle-evict tests: all five
  `start_paused` state tests died at `db::create_pool` with
  `Db(PoolTimedOut)`; rewinding `SourceEntry.last_used` via a
  `make_idle` helper made them deterministic with no clock control.

**Expected values come from running the reference engine, not from
reasoning about it.** When the contract is "match engine X" (for
html_dom: jsoup/SwiftSoup), an expected value produced from docs or
memory can be wrong in either direction — here an issue's carefully
researched expectation still didn't survive contact with the real
engines. Before asserting against the Aidoku contract, run the actual
jsoup/SwiftSoup on the fragment and pin what they print; when the two
engines disagree, Aidoku's engine (SwiftSoup) wins and the divergence
gets a pin test naming the choice.
- Evidence: 2026-10-18, issue #83's B2 quoted `"xy"` for
  `x<script>…</script>y`; jsoup 1.15.3 and SwiftSoup 2.13.7 both produce
  `"x y"` (the script element is a block separator; its content is what
  is dropped), and `"xy"` is that fragment's `ownText()`. PR #95 asserts
  the measured value.

**A workaround names its cause and gets retested when the toolchain moves.**
`strip` was disabled for every release build with no reason on record. The
real cause was rust-lang/rust#157750 (mis-aligned LINKEDIT crashing macOS
binaries, rust 1.95–1.97), fixed upstream in 1.98. The workaround outlived
the bug by two toolchain releases, and only a session asking why the
binaries were unstripped found it. A workaround without its issue number is
invisible to the person who could remove it.
- Evidence: PR #2 restored `strip = true` after the toolchain moved to
  1.99.1; the release binary runs and proc-macro dlopen works.

**A runner label is a dependency; track its retirement.**
The first v0.1.0 release run queued its Intel macOS leg on `macos-13` for
over 30 minutes until GitHub cancelled it — the image was already retired —
taking the docker and release jobs with it. The fix (cross-compile
x86_64-apple-darwin on macos-latest) landed in the same window, and the
next push's run carried the release. When GitHub announces a retirement on
actions/runner-images, grep the workflows for the label that day.
- Evidence: run 39078788016 cancelled on the queued macos-13 leg; PR #2's
  comment in release.yml now names macos-13 as deprecated.

**A rustup directory override silently pins the toolchain.**
`rustup update` reported stable current, but a directory override kept this
checkout on 1.97.1 — the release profile change was being verified against
a compiler two releases old. `rustc -vV` in the project directory is the
only reading that counts; `rustup default` says nothing when an override
exists.
- Evidence: 2026-10-18 session, `rustup show` listed the override; removing
  it moved the build to 1.99.1.

**A cancelled release run leaves no partial state — check before redoing
anything by hand.**
The version job skips only when the release already exists, so a later
green run picks the release up on its own. `git ls-remote --tags` and
`gh release list` give the real state; a cancelled run has created neither
tag nor release.
- Evidence: run 39078788016 cancelled with zero tags and zero releases
  created; run 39080251858 then ran the full pipeline for the same version.

**`gh run watch --exit-status` exits 1 on `cancelled`, not only on
`failure`.** A watcher that treats nonzero as "the build failed" misreads a
cancelled run. Read the per-job conclusion before deciding what happened.
- Evidence: the run-1 watcher exited 1 while four of five build legs were
  green; the queued macos-13 leg's cancellation was the only red.

**Write every GitHub body to a file; never inline it.**
Backticks inside a double-quoted `--body`/`--comment` string are command
substitution to the shell: the text between them ran as a command and the
posted comment had holes where `` `IN (...)` `` used to be. Same class as
the long-standing body-file rule for merge requests, and it bites on plain
issue comments too.
- Evidence: 2026-10-18, the claim comment on issue #33 went out mangled;
  fixed via comment PATCH with `--body-file`.

**A 200 on a config write proves nothing; read the parsed value back.**
A settings PUT stored the whole `{"values": …}` envelope as the values
map, so the consumer's top-level key never existed and the knob silently
stayed at its default — every "progressive" screenshot then showed the
final state. The read-back (`values.values.partial_delay`) was the only
place the bug was visible. Write, then parse what the consumer will parse.
- Evidence: 2026-10-18, the #50 stream measured 0ms until the flat re-PUT;
  after it the same request took the expected 16s.

**Write contract docs from the pipeline file, not from memory of the last
run.** *(recurrence of the standing "copy facts from the authoritative
source" rule — noted here so the daily review can count it.)*
The release docs said the floating image tag was `X.Y` because that is how
the previous verification was summarized; the workflow has always
published `<major>.x`. Three documents and the patrol cron's check
inherited the wrong tag, and it survived a whole release cycle.
- Evidence: PR #51 (2026-10-18) corrected all three after the v0.2.0
  manifest query showed `0.x`, not `0.2`.

**A smoke server started from the wrong directory serves stale assets.**
`webui_dir` resolves `./webui/build` against the process's cwd; starting a
PR worktree's binary from the main checkout's directory serves the main
checkout's old UI, and the page looks plausibly "done". Before trusting a
smoke page, curl the served asset hash and compare it with the build you
mean to test.
- Evidence: 2026-10-18, the #38 smoke served `index-BWzzabJ3.js` while the
  worktree build was `index-BnNwpN81.js`; restarting with the worktree cwd
  served the right one.

**Count sqlx statements from the event's `summary` field, and filter
PRAGMA.** sqlx emits one `sqlx::query` event per statement with the
summary in the `summary` field (not `message`), and connection setup emits
PRAGMA events that fall into the measurement window on slow machines. A
counter reading `message` and keeping PRAGMA was green locally and flaky
red in CI.
- Evidence: PR #39's first CI run red while local was green; the fixed
  counter ran 60/60 locally and green in CI (2026-10-18).

**The build matrix is CI's job; never rehearse it locally.**
A dispatch prompt asked the subagent to prove the four release targets
locally, so it burned ~50 minutes on a `cross` + docker compile that the
CI legs already cover — on a machine with a history of memory-pressure
OOM kills. Before writing a verification requirement into a prompt, ask
whose job it is: anything the build/docker/release legs already answer
belongs to CI; local work is tests plus the host build. And when a
dependency choice carries build risk, prefer the zero-risk option (pure
Rust) so cross uncertainty never materializes at all.
- Evidence: 2026-10-18, the #21 dispatch's local cross build was killed
  on the maintainer's ask; the instruction was corrected to CI-owned
  verification and pure-Rust-first selection.

**Decide an issue only after reading the whole thread, not just the
body.** `gh issue view --json body` returns no comments, and a decision
made from the body alone can contradict what the maintainer already said
in the thread. Here a references comment (two headless-browser options)
sat unread for five hours while the issue was decided and merged the
other way; a collaborator had to reopen the issue to get it re-examined.
Read `gh issue view --comments` before deciding or closing anything, and
treat a reopen as new input, not noise.
- Evidence: 2026-10-18, issue #26 was decided "sandbox + explicitly
  unsupported" and merged while everpcpc's chromiumoxide/moli references
  sat unread in the thread; dyphire reopened it the same day.

**A rewritten branch gets its test count re-checked, not just its CI.**
A collaborator consolidated their branch into one commit before merge;
the rewrite kept the code fixes but silently dropped two tests added
during review. CI went green because the suite got smaller, and
`--match-head-commit` matched the new head while the content had
changed. After any force-push that rewrites history, diff the test list
(`cargo nextest list` or the suite count) against the reviewed state
before merging — the commit-sha pin proves where the head points, never
what it contains.
- Evidence: 2026-10-18, PR #61's rewrite into `24971ab5` dropped the two
  jar sink tests from the review fix; re-added in PR #64.

**A probe on the app's own port answers the dev server, not your
container.** The main checkout's dev server listens on :3000, so a test
sidecar published with `docker run -p 127.0.0.1:3000:3000` looks healthy
(docker prints a container id) while every probe to 127.0.0.1:3000 lands
on the dev server. Verify a probe against what the sidecar should
actually say — a CDP websocket upgrade answers 101, not the webui's
HTML — and pick a host port that is really free before blaming the
client library.
- Evidence: 2026-10-18, PR #62's e2e setup: browserless on :3000
  answered every handshake with the Sumidoku webui ("HTTP error: 200
  OK" from tungstenite); republishing on 127.0.0.1:3300 made the CDP
  handshake succeed on the first try.

**`&Request` held across `.await` fails the `FromFn: Service` bound without
naming `Send`.** axum's `Body` wraps `UnsyncBoxBody` — it is `Send` but
not `Sync` — so a `&Request` captured by an `async` middleware's future
makes that future non-`Send`, and rustc reports the whole
`FromFn<…>: Service<_>` bound as unsatisfied with no pointer to the real
cause. Extract owned data from the request (headers, tokens, query)
*before* the first `.await`; borrows of `HeaderMap` are fine because it is
`Sync`. When a middleware that "looks just like the working one" fails
this bound, strip the body to `next.run(req).await` and re-add pieces —
that bisect lands on the non-`Send` capture in minutes.
- Evidence: 2026-10-18, the auth middleware kept `&req` alive across
  `bearer_granted(...).await`; two identical-looking signatures (working
  `basic_auth` vs failing `require_auth`) differed only in that borrow,
  and the fix was passing an owned `Option<String>` token instead.

**A new field on a public struct breaks every out-of-crate struct literal;
list them before pushing.**
`SourceConfig.accept_language` compiled inside the crate and broke all
seven `crates/wasm/tests/*.rs` integration tests that build
`SourceConfig { … }` by hand. A parse-only check scoped to the files you
edited cannot see that, and no local compile was possible
(wasmer-compiler-cranelift is a `compile_error!` on Windows), so the red
landed in CI instead of the session. Before pushing a struct change, grep
the whole workspace — `crates/*/tests/` included — for `Name {` literals
and update every one.
- Evidence: 2026-10-18, branch `net`, the "send the source language's
  Accept-Language" commit: CI's `cargo test --workspace` reported `E0063:
  missing field accept_language` in rekey_host, js_host, webview_host,
  canvas_host, html_mutation, partial_result and smoke; the follow-up
  commit adds the field to each.

**Never run `cargo fmt` in this repo; the tree is hand-formatted with tabs.**
There is no `rustfmt.toml`, so default rustfmt expands every tab to spaces
and rewrites the whole workspace — 108 files, `vendor/aidoku` included,
which `AGENTS.md` says must not be restyled. A single `cargo fmt --all`
produced a 31k-insertion/28k-deletion diff over tracked files; only
`git reset --hard` undid it. Match the surrounding indentation by hand;
CI checks clippy and tests, not formatting.
- Evidence: 2026-10-18 session, `cargo fmt --all` reformatted 108 files
  (including `vendor/aidoku`) before the fix was re-applied on the reset
  tree.

**The pre-merge re-check is its own step; the merge runs only after the
result is read.** A fresh-look that shares a shell command with the merge
(`check && merge`) is a log line, not a gate — the merge fired seven
minutes after a collaborator's explicit hold, un-read. Run the check,
look at the output, then decide; never chain the two.
- Evidence: 2026-10-18, PR #103 (release 0.5.0) merged at 15:50:13 with
  dyphire's "wait until the work is finished" (15:43:15) sitting in the
  thread; the release run was cancelled 16s in — no tag, no release —
  and 0.5.0 is held for his fixes.

**A Cloudflare clearance is bound to the client that solved it; a bypass
that replays it from another client silently re-challenges.**
`cf_clearance` is tied to the solving browser's user agent (and IP), so a
solver API's response must be replayed with that same user agent — the
relay and Suwayomi both do. Sumidoku routed a challenged request through
trawl once and then sent the cookie back over its own reqwest client with a
different (Chrome) user agent against trawl's Camoufox Firefox, so every
later request re-challenged. The fix keeps a challenged host on trawl, which
holds the clearance and user agent together. Check how a solver's output is
consumed before trusting a "transparent" handoff.
- Evidence: 2026-10-19, bakamh's managed challenge worked through
  aidoku-cf-relay and Suwayomi but not Sumidoku; `routed.rs:trawl_escalation`
  now marks the host and `route_for` keeps it on trawl.
- Superseded: 2026-10-23, the proxy-sticky fix traded the UA mismatch for two
  new failures — trawl's browser tiers answer HTML with the serialized DOM
  (charset-less fragments arrive as windows-1252 mojibake, see the zh.18mh
  entry below), and a browser runs per request. The pairing rule itself was
  right; the conclusion "so trawl must hold the pair" was not. Sumidoku now
  holds the pair itself: `solve_clearance` takes the solver's userAgent from
  the scrape answer together with cf_clearance and replays both over direct
  connections (relay.py's shape), with the proxy kept as the solve-failure
  fallback.

**A change event's `currentTarget` is dead by the time a deferred state
updater runs — read the value eagerly.** React nulls `currentTarget` when
the dispatch ends, so `setState((cur) => … e.currentTarget.value …)`
crashes with "Cannot read properties of null (reading 'value')" whenever
the updater is not eagerly evaluated during dispatch (batching defers
it). Grab `const v = e.currentTarget.value` first and close over `v`;
sites that survive on React's eager-state optimization are still
landmines. Type checking cannot see this — only an exercised UI catches
it, so UI changes get a real-browser smoke, not just `tsc`.
- Evidence: 2026-10-19, PR #112's localStorage input crashed the source
  settings modal on its first keystroke in real-browser smoke (fixed in
  ec3a91b); the same latent pattern in `SettingsPage.tsx` and
  `MangaDetailPage.tsx` filed as #113.

**Local UI smoke runs the dev loop, not the embedded build.** The vite
dev server proxies `/api` and `/opds` to a debug backend (README
documents it), giving hot reload and readable stacks; the embedded
production build costs a rebuild per edit, fights browser caches, and
hides stack traces behind minification — and its serving path is
already smoked by CI's docker leg on every PR. Reach for the release
binary only when the change touches the embed/serving layer itself.
- Evidence: 2026-10-19, PR #112's acceptance burned cycles on a stale
  embed (the disk branch wins over embed when `./webui/build` exists
  under the server's cwd), on cache busting, and on hand-decoding a
  minified `currentTarget` stack that dev-server source maps would have
  named directly; README.md:101 had the dev loop documented all along.

**chrono's `NaiveDateTime::parse_from_str` silently drops a parsed zone
offset — route offset-bearing formats through `DateTime`.** A format
containing `%z` parses fine into a `NaiveDateTime`; the offset is
discarded and the timestamp comes out wrong by however much the string's
zone differed from the one you attach afterwards. Any layer translating
format strings with zone tokens must branch those formats to
`DateTime::parse_from_str`. Two sibling traps from the same probe: `%f`
parses a nanosecond *count* while `%.f` parses a fraction of a second,
and only `%#z` accepts `Z`, `+09`, `+0900` and `+09:00` alike (`%z` and
`%:z` reject `Z` and the hours-only form).
- Evidence: 2026-10-19, issue #110's parse_date work; the chrono 0.4.45
  probe printed `Ok(2024-01-15T12:00:00)` for input `…+0900` under
  `%…%z` (the correct reading needs the −9h shift), and `500` under `%f`
  became 500ns rather than half a second; PR #124 routes zone-token
  formats through `DateTime::parse_from_str` and maps `S` to `%.f`.

**Re-check a same-repo PR is still open before pushing a fix to its head
branch.** A collaborator can merge the PR — and GitHub delete the head
branch — while the review is still running; `git push` then recreates
the deleted branch disconnected from the PR, and the fix has to be
rescued onto a fresh branch anyway. One `gh pr view <N> --json state`
before pushing costs a call and saves the detour.
- Evidence: 2026-10-19, PR #165: dyphire merged it mid-review and the
  head branch `webui` was deleted before the fix push, which recreated
  it as "[new branch]"; the fix was cherry-picked onto
  `fix/browse-updates-header` (PR #166) and the stray branch deleted.
- Seen again: 2026-10-19, PR #170 — the rule was applied before the
  fix push (state read OPEN), yet the author merged one minute before
  the merge command; no harm because the fix was already on the branch.

**Piping cargo output hides its exit code — check `${PIPESTATUS[0]}`
after the pipe.** `cargo clippy ... | tail -5` prints the errors and
still exits 0, because the pipeline's status is `tail`'s. A gate run
this way can report green while clippy is failing on screen. Read the
status of the left side, or drop the pipe.
- Evidence: 2026-10-19, #164's implementation session: a clippy gate
  piped to `tail` showed error output with `EXIT:0`; caught when the
  same gate was re-run unpiped before pushing.

**A compiler-cache wrapper does its own work in the project's name —
suspect the wrapper when build memory or disk goes wrong.** The ~2 GB
dsymutil processes behind a build's memory peak had the cache wrapper
(kache) as their parent, not cargo, and the wrapper ran them even when
the wrapped rustc carried `-C split-debuginfo=off` (kache never parses
that flag — kunobi-ninja/kache#1625). The same hiding covers the store
on disk: 177 GB of cached dSYM bundles filled the disk twice while
build targets got the blame. Put `RUSTC_WRAPPER` (kache/sccache) on the
suspect list for build-resource anomalies, put the cache store in every
disk sweep, and use the one-command bisect (`KACHE_DISABLED=1` or
unsetting the wrapper) before theorizing about the project.
- Evidence: 2026-10-19, pid pairings tied every dsymutil to the kache
  wrapper with `split-debuginfo=off` in the rustc line; 44 `.dSYM`
  dirs appeared under `off` vs 0 with `KACHE_DISABLED=1`; the store
  measured 177 GB; the affected builds moved to sccache (same full
  build: 636 s → 100 s, zero dsymutil).

**Install software through the machine's native package manager first;
the skill's generic static-binary fallback is a fallback, not the
default.** Check the distro and its repos (pacman/brew/apt) before
reaching for a release tarball — a repo-managed install gets updates
and clean removal for free.
- Evidence: 2026-10-19, a sccache install went in as a manually
  copied static binary pulled from upstream releases until the
  maintainer asked whether the native package manager carried it — it
  did; reinstalled through the package manager and removed the manual
  copy.

**`--match-head-commit` takes the sha from a live `gh pr view`, never
from memory.** A remembered sha fails loudly when it is not a real
object (GraphQL "Could not coerce … to GitObjectID"), and one that
happened to collide with a real object would pin the wrong head. The
standing rule lives in the patrol prompt; anchored here after a
same-day slip.
- Evidence: 2026-10-19, the first merge attempt on PR #171 was rejected
  for a from-memory sha; the real head (`288121b4dcb…`) fetched via
  `gh pr view` differed and merged.
- Seen again: 2026-10-19, PR #188 — passed the short sha from the local
  `git commit` output instead of the full sha in the `gh pr view` result
  fetched seconds earlier; same GraphQL rejection, retried with the
  fetched full sha.

**Contract compatibility claims get verified against the upstream
implementation, never against a second-hand quote.** A behavior copied
from one doc line can invert the real contract, and a fixture written
from the same reading then "confirms" the wrong behavior with a green
test — the strongest kind of false reassurance, since the pinned test
punishes the later correct fix. When emulating a host's behavior, read
the upstream host source; treat any fixture born from a quote as
evidence of nothing.
- Evidence: 2026-10-20, zh.jmcomic chapter images failed with
  `invalid image rid -1`: the processor input had been raw bytes since
  #119, implemented from a one-line doc quote, while AidokuRunner's
  `store(value:)` decodes when possible (pixels for canvas
  descrambling) and only falls back to raw bytes when undecodable; the
  rawimg fixture had pinned the misreading. Fixed in #223 after
  reading the upstream Swift host; two sabotage cycles confirmed the
  realigned tests catch each bug.

**A green pipeline that never executes the artifact proves nothing about
the artifact.** Every gate exercises the source tree — build, clippy,
tests — while the shipped binary can crash deterministically on a path
no test drives; only running the artifact on that path catches it. And
when a deployment reports 502s, read the process state before the app
logic: restart count, `journalctl` `traps:` lines and coredumps say
"crash-looping" faster than any handler theory.
- Evidence: 2026-10-20, linux x86_64 binaries ≤0.17.1 SIGSEGV'd on
  every async HTTPS fetch (`movaps` #GP in the TLS path) while CI
  stayed green for two releases; the 502 a user saw was nginx answering
  during container restart gaps, with four identical-offset cores in
  the journal. Follow-ups tracked in #234.

**A migration file is a shipped artifact, not source text — editing one
without an explicit compatibility alias breaks existing deployments at boot.**
sqlx embeds each migration's SHA-384 at compile time and validates it against
`_sqlx_migrations` at startup, so a comment translation has the same blast
radius as a schema change (a `VersionMismatch` panic). Restyling or "tidying"
an applied migration is a breaking change; schema work lands as a new numbered
file. Canonical files use English comments and LF. Compatibility requires a
closed list of exact historical checksums: deriving a CRLF alias from the
current file would silently extend acceptance with every later edit.
- Evidence: 2026-10-21, the comment-translation sweep (#236) touched 10
  applied migrations and 0.19.0 refused to boot on upgraded databases
  (`Migrate(VersionMismatch(1))`); the restore was verified by matching
  the restored file's SHA-384 against a pre-sweep database's recorded
  checksum.
- Seen again: 2026-10-25, #418 restored the reviewed English comments across
  all 24 migrations and registered the historical Chinese and CRLF SHA-384
  values explicitly. The English/LF guards and startup regressions live in
  `db.rs:shipped_migrations_use_ascii_comments_and_lf` and the migration tests.

**quick-xml's serde matches elements by local name — the namespace prefix is
stripped, so `dc:title` must be renamed to the bare `title`, and only
attributes take the `@` prefix.** A `rename = "dc:title"` never matches
anything, silently yielding `None`/empty vecs with no error. When parsing
namespaced XML (OPF, RSS, Atom), rename to the local name and verify with a
fixture that actually carries the prefix.
- Evidence: 2026-10-21, #272's OPF metadata parsing: `rename = "dc:title"`
  would never hit; quick-xml 0.42's `de/key.rs` matches on the local name.
  The same wrong mental model sits in `comic_info.rs`'s "matches literally"
  comment (masked by its merge logic).

**When an issue cites a reference implementation, the reference's code is
the spec — the issue text is only its summary.** "照 X 配" means reading
X's actual files before designing, not just the paragraphs that mention
X; edge semantics (field mappings, profile detection, fallback rules)
live in the reference's source and almost never survive into the issue
text. The recon checklist for such an issue names the exact files in the
reference and what each one pins down.
- Evidence: 2026-10-21, three local-source PRs built from issue scopes
  (#269 multi-dir, #272 EPUB, #274 metadata) were all reopened the same
  day with "refer to kmrs for re-improvement" — the kmrs clone sat in
  `~/src` unread while the implementations guessed at field mappings and
  profile detection that kmrs already pins down.
