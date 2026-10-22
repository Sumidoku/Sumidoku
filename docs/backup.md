# Backup and imports

All state portability flows through one versioned JSON document and two
import paths for migrating from other readers. Every import is
transactional and reports what it skipped and why.

## Full backup (export / import)

`GET /api/v1/backup` downloads the built-in admin's complete state —
library membership and manga metadata, categories, reading progress, app
settings and source settings — as one JSON document with a `version` field
(currently `1`; unknown versions are rejected with `400`).
`POST /api/v1/backup` imports it into the same owner state, optionally
limited with `?sections=library,categories,progress,settings,sourceSettings`.
Import
upserts by natural key (source key + manga key, chapter key, category
name) inside a single transaction, so a failed or invalid backup leaves
the database untouched; entries whose source is not installed are skipped
and reported in the response `warnings`. Category update exclusions use the
backup-only `includeInUpdate` field (`2` for excluded categories, `0` for all
others). The raw `library.excluded_update_categories` setting is omitted from
exports and ignored on import; category restoration maps names to target-local
category ids before writing that setting. The settings page of the web UI has
download and restore buttons. The file contains source settings, which may
include site passwords — store it accordingly.

## Scheduled backups

The server can also write backups on its own schedule, so a corrupted
database or a bad change between manual exports stays recoverable. It is
configured through the `setting` table (the web UI's settings page exposes
all of it under "自动备份"), and each value can be pinned by an env
override for headless deployments:

| Setting key | Env override | Default | Purpose |
| --- | --- | --- | --- |
| `backup.enabled` | `SUMIDOKU_BACKUP_ENABLED` | `false` | master switch |
| `backup.interval_hours` | `SUMIDOKU_BACKUP_INTERVAL_HOURS` | `24` | hours between runs (minimum 1) |
| `backup.retention_count` | `SUMIDOKU_BACKUP_RETENTION_COUNT` | `10` | how many newest files to keep |
| `backup.dir` | `SUMIDOKU_BACKUP_DIR` | `<config>/backups` | destination directory |

When enabled, a background task writes
`sumidoku-backup-<yyyymmdd-HHMMSS>.json` (UTC) every interval — a
server-side full export built as the built-in admin, the same document an
admin's `GET /api/v1/backup` serves — stamps `backup.last_run`, and
deletes files beyond the retention count, oldest first; the newest file is
never deleted, even with `backup.retention_count = 0`. At startup, if more
than one interval elapsed since `backup.last_run` (or no backup was ever
taken), one backup runs immediately, so downtime does not leave a gap.
`GET /api/v1/backups` lists the files (name, size, creation time, newest
first); `POST /api/v1/backups/restore` with `{"name": "<file>"}` restores
one through the same validated import path as the upload endpoint, and the
settings page lists them with a restore action.

## Importing from Aidoku

`POST /api/v1/import/aidoku` imports an Aidoku app backup (`.aib`, a
binary plist) into the library: library manga, reading progress (chapter
read state and last page read) and categories with membership. Aidoku and
Sumidoku share the same source ecosystem, so entries match on source key
and manga/chapter key; entries whose source is not installed are skipped
and reported with a reason in the response `skipped` list, never
half-imported silently. The import is additive — existing library entries
and categories keep their current state, and progress merges by taking the
larger last-page-read, OR-ing the read flag and keeping the newer read
timestamp. `POST /api/v1/import/aidoku?dryRun=true` runs the same
transaction and rolls it back, returning the would-import report
(per-section counts plus the skipped list) without writing anything. The
web UI's settings page has an "从 Aidoku 导入" button under the backup
section that walks upload → dry-run preview → confirm. Aidoku settings,
source configs and trackers are out of scope.

## Importing from Suwayomi/Mihon

`POST /api/v1/import/suwayomi` imports a Suwayomi-Server or
Mihon/Tachiyomi backup (`.tachibk`, `.proto.gz` — both are gzip around one
protobuf `Backup` message; raw proto and zip containers are sniffed too)
into the library: library manga (`favorite` entries), chapter read history
(read state, last page read and the history timeline; chapters present
only in history are kept), and categories with membership. The merge is
additive, same as the Aidoku import.

Backups reference Mihon extension sources by id and name, not Aidoku
source keys, so every backup source group is resolved first: it
auto-matches only when exactly one installed source has the same
normalized name — multiple hits are never guessed — and the response
`sources` list reports each group's state and manga count. Unmatched
groups need an explicit `?sourceMapping={"<mihonSourceId>":
"<aidokuSourceKey>"}` or they skip wholesale into the `skipped` list.
`?dryRun=true` previews the same transaction without writing. The web UI's
settings page has a "从 Suwayomi 导入" button under the backup section
that walks upload → grouped preview → per-source mapping → confirm.

One caveat: a backup's manga `url` is the Mihon source's internal key,
which does not always match the corresponding Aidoku source's manga key;
such entries import with their progress but a later source refresh may not
resolve them. Tracking, extension settings, saved searches and server
preferences are out of scope.
