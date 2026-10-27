//! Server-synthesized Aidoku HomeLayout and listings for built-in local sources.
//!
//! A local instance has no WASM package, so `get_home` / `get_listings` / `get_manga_list`
//! were 501 for it. This module builds the same section-based home page a WASM source would
//! from the local scan index (`manga` / `chapter` / `chapter_user`), so a local library gets:
//!
//! - `continue-reading` — MangaChapterList (page_size=1), in-progress chapters ordered by last_read_at
//! - `latest-updates` — MangaChapterList (page_size=1), manga whose content changed after first
//!   indexing (`manga.updated_at IS NOT NULL`), ordered by `manga.updated_at`; the displayed chapter
//!   is the newest by `chapter.created_at` and its subtitle time is `manga.updated_at`
//! - `latest-added` — Scroller, manga ordered by first-indexed time (`manga.created_at`)
//! - `recently-read` — MangaChapterList (page_size=1), finished chapters ordered by last_read_at
//!
//! Each section carries a `listing` pointer, so the existing "View all" button on
//! SourceHomePage routes to `/browse/{id}?mode={listingId}`; the listings endpoints below
//! serve the full paginated list for that mode.
//!
//! The wire shape mirrors what WASM sources return: `HomeComponent.title`/`subtitle` and
//! the listing `name` are dotted i18n keys (`sourceHome.*`) that the web UI resolves through
//! its translation layer. The MangaChapterList subtitle is hardcoded on the frontend to
//! `chapterLabel(chapter) · relativeTime(chapter.date_uploaded)`, so for the reading-history
//! sections we stuff `chapter_user.last_read_at` into `chapter.date_uploaded` — the row
//! then reads "Ch.12 · 3 hours ago" with the time being the reading time, not the upload time.
//! For `latest-updates` we stuff `manga.updated_at` in instead, so the time reflects when the
//! series was last updated.

use aidoku::{Chapter, Listing, ListingKind, Manga};
use serde_json::{Value, json};
use sqlx::Row;

use crate::error::{ApiError, ApiResult};
use crate::model::{MangaRow, chapter_from_row, manga_from_row};
use crate::state::AppState;

/// Listing ids exposed on local sources; also the values the web UI puts in `?mode=`
pub(crate) const LISTING_ALL: &str = "all";
pub(crate) const LISTING_CONTINUE_READING: &str = "continue-reading";
pub(crate) const LISTING_LATEST_UPDATES: &str = "latest-updates";
pub(crate) const LISTING_LATEST_ADDED: &str = "latest-added";
pub(crate) const LISTING_RECENTLY_READ: &str = "recently-read";

/// Section length on the home page and per listing page. Aidoku sources do not expose a
/// page-size parameter, so this is a fixed internal constant rather than a wire parameter.
const SECTION_LIMIT: i64 = 20;

/// The admin user id; local reading-progress rows live on user 1
const USER_ID: i64 = 1;

/// The listings a local source exposes, in browse-page segmented-control order.
/// `kind` is `list` for row-shaped sections (MangaChapterList) and `default` for
/// cover-grid sections (Scroller), matching how SourceBrowsePage picks the layout.
pub(crate) fn local_listings() -> Vec<Listing> {
	vec![
		Listing {
			id: LISTING_ALL.into(),
			name: "sourceHome.all".into(),
			kind: ListingKind::Default,
		},
		Listing {
			id: LISTING_CONTINUE_READING.into(),
			name: "sourceHome.continueReading".into(),
			kind: ListingKind::List,
		},
		Listing {
			id: LISTING_LATEST_UPDATES.into(),
			name: "sourceHome.latestUpdates".into(),
			kind: ListingKind::List,
		},
		Listing {
			id: LISTING_LATEST_ADDED.into(),
			name: "sourceHome.latestAdded".into(),
			kind: ListingKind::Default,
		},
		Listing {
			id: LISTING_RECENTLY_READ.into(),
			name: "sourceHome.recentlyRead".into(),
			kind: ListingKind::List,
		},
	]
}

/// Cover URL served to the web UI: the local manga cover endpoint resolves cover.*
/// sidecar → first page of last chapter → placeholder, same as the manga detail page.
fn cover_url_for(manga_id: i64) -> String {
	format!("/api/v1/manga/{manga_id}/cover")
}

/// Project a MangaRow into the SourceManga JSON the web UI expects, with internal id and
/// cover URL resolved to the local cover endpoint.
fn manga_to_json(row: &MangaRow) -> ApiResult<Value> {
	let mut manga: Manga = manga_from_row(row);
	manga.cover = Some(cover_url_for(row.id));
	let mut value = serde_json::to_value(&manga)?;
	let obj = value
		.as_object_mut()
		.ok_or_else(|| ApiError::internal("unexpected manga shape"))?;
	obj.insert("sourceId".into(), json!(row.source_id));
	obj.insert("id".into(), json!(row.id));
	Ok(value)
}

/// Project a ChapterRow into the SourceChapter JSON. `override_date_uploaded` lets the
/// caller substitute a different timestamp into `date_uploaded` (the reading-time trick
/// described at the module header); pass None to use the row's own value.
fn chapter_to_json(row: &crate::model::ChapterRow, override_date_uploaded: Option<i64>) -> ApiResult<Value> {
	let mut ch: Chapter = chapter_from_row(row);
	if override_date_uploaded.is_some() {
		ch.date_uploaded = override_date_uploaded;
	}
	Ok(serde_json::to_value(&ch)?)
}

/// A (MangaRow, ChapterRow, last_read_at) row from a join query.
///
/// sqlx FromRow does not flatten, so we pull both sides with column prefixes and
/// rebuild the two row structs by hand.
struct JoinedRow {
	manga: MangaRow,
	chapter: crate::model::ChapterRow,
	last_read_at: Option<i64>,
}

fn row_to_joined(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<JoinedRow> {
	let manga = MangaRow {
		id: row.try_get("id")?,
		source_id: row.try_get("source_id")?,
		key: row.try_get("key")?,
		title: row.try_get("title")?,
		authors: row.try_get("authors")?,
		artists: row.try_get("artists")?,
		description: row.try_get("description")?,
		tags: row.try_get("tags")?,
		status: row.try_get("status")?,
		content_rating: row.try_get("content_rating")?,
		viewer: row.try_get("viewer")?,
		cover_url: row.try_get("cover_url")?,
		url: row.try_get("url")?,
		initialized: row.try_get("initialized")?,
		update_strategy: row.try_get("update_strategy")?,
		next_update_time: row.try_get("next_update_time")?,
		last_fetched_at: row.try_get("last_fetched_at")?,
		chapters_last_fetched_at: row.try_get("chapters_last_fetched_at")?,
		trashed_at: row.try_get("trashed_at")?,
		scan_mtime: row.try_get("scan_mtime")?,
		created_at: row.try_get("created_at")?,
		updated_at: row.try_get("updated_at")?,
	};
	let chapter = crate::model::ChapterRow {
		id: row.try_get("cid")?,
		manga_id: row.try_get("cmanga_id")?,
		key: row.try_get("ckey")?,
		title: row.try_get("ctitle")?,
		number: row.try_get("cnumber")?,
		volume: row.try_get("cvolume")?,
		scanlators: row.try_get("cscanlators")?,
		language: row.try_get("clanguage")?,
		thumbnail: row.try_get("cthumbnail")?,
		url: row.try_get("curl")?,
		date_uploaded: row.try_get("cdate_uploaded")?,
		source_order: row.try_get("csource_order")?,
		page_count: row.try_get("cpage_count")?,
		locked: row.try_get("clocked")?,
		fetched_at: row.try_get("cfetched_at")?,
		created_at: row.try_get("ccreated_at")?,
		trashed_at: row.try_get("ctrashed_at")?,
	};
	let last_read_at: Option<i64> = row.try_get("last_read_at").unwrap_or(None);
	Ok(JoinedRow { manga, chapter, last_read_at })
}

/// Continue-reading: chapters the user has started but not finished, newest progress first.
async fn run_continue_reading(
	state: &AppState,
	source_id: i64,
	exclusions: &[String],
	limit: i64,
	offset: i64,
) -> ApiResult<Vec<Value>> {
	let mut clauses = String::new();
	for ex in exclusions {
		if !ex.is_empty() {
			clauses.push_str(" AND lower(m.key) NOT LIKE '%' || lower(?) || '%'");
		}
	}
	let sql = format!(
		"SELECT m.id, m.source_id, m.key, m.title, m.authors, m.artists, m.description,
		        m.tags, m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized,
		        m.update_strategy, m.next_update_time, m.last_fetched_at, m.chapters_last_fetched_at,
		        m.trashed_at, m.scan_mtime, m.created_at, m.updated_at,
		        c.id AS cid, c.manga_id AS cmanga_id, c.key AS ckey, c.title AS ctitle,
		        c.number AS cnumber, c.volume AS cvolume, c.scanlators AS cscanlators,
		        c.language AS clanguage, c.thumbnail AS cthumbnail, c.url AS curl,
		        c.date_uploaded AS cdate_uploaded, c.source_order AS csource_order,
		        c.page_count AS cpage_count, c.locked AS clocked, c.fetched_at AS cfetched_at,
		        c.created_at AS ccreated_at, c.trashed_at AS ctrashed_at,
		        cu.last_read_at
		 FROM chapter_user cu
		 JOIN chapter c ON c.id = cu.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 WHERE m.source_id = ? AND m.trashed_at IS NULL{clauses}
		   AND cu.user_id = ? AND cu.last_page_read > 0 AND cu.is_read = 0
		 ORDER BY cu.last_read_at DESC
		 LIMIT ? OFFSET ?"
	);
	let mut query = sqlx::query(&sql).bind(source_id);
	for ex in exclusions {
		if !ex.is_empty() {
			query = query.bind(ex);
		}
	}
	let rows = query.bind(USER_ID).bind(limit).bind(offset).fetch_all(&state.pool).await?;
	let mut out = Vec::new();
	for row in &rows {
		let joined = row_to_joined(row)?;
		// Subtitle reads "Ch.12 · 3 hours ago" where the time is last_read_at (the reading-time trick)
		let m = manga_to_json(&joined.manga)?;
		let c = chapter_to_json(&joined.chapter, joined.last_read_at)?;
		out.push(json!({ "manga": m, "chapter": c }));
	}
	Ok(out)
}

/// Latest chapter per manga, ordered by the manga's last content update time desc.
/// Only manga whose content changed after they were first indexed appear here; newly added manga
/// with no subsequent changes live in the latest-added listing.
async fn run_latest_updates(
	state: &AppState,
	source_id: i64,
	exclusions: &[String],
	limit: i64,
	offset: i64,
) -> ApiResult<Vec<Value>> {
	// For each manga, pick its newest chapter by (created_at DESC, source_order DESC, id DESC).
	// SQLite lacks DISTINCT ON, so we use a correlated subquery against the latest chapter id.
	let mut clauses = String::new();
	for ex in exclusions {
		if !ex.is_empty() {
			clauses.push_str(" AND lower(m.key) NOT LIKE '%' || lower(?) || '%'");
		}
	}
	let sql = format!(
		"SELECT m.id, m.source_id, m.key, m.title, m.authors, m.artists, m.description,
		        m.tags, m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized,
		        m.update_strategy, m.next_update_time, m.last_fetched_at, m.chapters_last_fetched_at,
		        m.trashed_at, m.scan_mtime, m.created_at, m.updated_at,
		        c.id AS cid, c.manga_id AS cmanga_id, c.key AS ckey, c.title AS ctitle,
		        c.number AS cnumber, c.volume AS cvolume, c.scanlators AS cscanlators,
		        c.language AS clanguage, c.thumbnail AS cthumbnail, c.url AS curl,
		        c.date_uploaded AS cdate_uploaded, c.source_order AS csource_order,
		        c.page_count AS cpage_count, c.locked AS clocked, c.fetched_at AS cfetched_at,
		        c.created_at AS ccreated_at, c.trashed_at AS ctrashed_at
		 FROM manga m
		 JOIN chapter c ON c.id = (
		     SELECT id FROM chapter
		     WHERE manga_id = m.id AND trashed_at IS NULL
		     ORDER BY created_at DESC, source_order DESC, id DESC
		     LIMIT 1
		 )
		 WHERE m.source_id = ? AND m.trashed_at IS NULL AND m.updated_at IS NOT NULL{clauses}
		 ORDER BY m.updated_at DESC, m.id DESC
		 LIMIT ? OFFSET ?"
	);
	let mut query = sqlx::query(&sql).bind(source_id);
	for ex in exclusions {
		if !ex.is_empty() {
			query = query.bind(ex);
		}
	}
	let rows = query.bind(limit).bind(offset).fetch_all(&state.pool).await?;
	let mut out = Vec::new();
	for row in &rows {
		let joined = row_to_joined(row)?;
		// Subtitle reads "Ch.12 · 3 hours ago" where the time is the manga's last content update,
		// so the row reflects when the series was updated rather than the chapter's file mtime.
		let m = manga_to_json(&joined.manga)?;
		let c = chapter_to_json(&joined.chapter, joined.manga.updated_at)?;
		out.push(json!({ "manga": m, "chapter": c }));
	}
	Ok(out)
}

/// Recently-added manga rows, by first-indexed time desc.
async fn run_recently_added(
	state: &AppState,
	source_id: i64,
	exclusions: &[String],
	oneshots_dir: Option<&str>,
	limit: i64,
	offset: i64,
) -> ApiResult<Vec<Value>> {
	// Exclusions and the optional oneshots marker are pushed into SQL as LIKE clauses so LIMIT/OFFSET
	// paginates correctly (Rust-side filtering would skip rows inside the page window).
	let mut clauses = String::new();
	for ex in exclusions {
		if ex.is_empty() {
			continue;
		}
		clauses.push_str(" AND lower(m.key) NOT LIKE '%' || lower(?) || '%'");
	}
	if let Some(dir) = oneshots_dir
		&& !dir.is_empty()
	{
		clauses.push_str(" AND lower(m.key) LIKE '%' || lower(?) || '%'");
	}
	let sql = format!(
		"SELECT m.id, m.source_id, m.key, m.title, m.authors, m.artists, m.description,
		        m.tags, m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized,
		        m.update_strategy, m.next_update_time, m.last_fetched_at, m.chapters_last_fetched_at,
		        m.trashed_at, m.scan_mtime, m.created_at, m.updated_at
		 FROM manga m
		 WHERE m.source_id = ? AND m.trashed_at IS NULL{clauses}
		 ORDER BY m.created_at DESC, m.id DESC
		 LIMIT ? OFFSET ?"
	);
	let mut query = sqlx::query(&sql).bind(source_id);
	for ex in exclusions {
		if !ex.is_empty() {
			query = query.bind(ex);
		}
	}
	if let Some(dir) = oneshots_dir
		&& !dir.is_empty()
	{
		query = query.bind(dir);
	}
	let rows = query.bind(limit).bind(offset).fetch_all(&state.pool).await?;
	let mut out = Vec::new();
	for row in &rows {
		let manga = MangaRow {
			id: row.try_get("id")?,
			source_id: row.try_get("source_id")?,
			key: row.try_get("key")?,
			title: row.try_get("title")?,
			authors: row.try_get("authors")?,
			artists: row.try_get("artists")?,
			description: row.try_get("description")?,
			tags: row.try_get("tags")?,
			status: row.try_get("status")?,
			content_rating: row.try_get("content_rating")?,
			viewer: row.try_get("viewer")?,
			cover_url: row.try_get("cover_url")?,
			url: row.try_get("url")?,
			initialized: row.try_get("initialized")?,
			update_strategy: row.try_get("update_strategy")?,
			next_update_time: row.try_get("next_update_time")?,
			last_fetched_at: row.try_get("last_fetched_at")?,
			chapters_last_fetched_at: row.try_get("chapters_last_fetched_at")?,
			trashed_at: row.try_get("trashed_at")?,
			scan_mtime: row.try_get("scan_mtime")?,
			created_at: row.try_get("created_at")?,
			updated_at: row.try_get("updated_at")?,
		};
		let m = manga_to_json(&manga)?;
		out.push(json!({
			"title": manga.title,
			"subtitle": null,
			"image_url": cover_url_for(manga.id),
			"value": { "Manga": m }
		}));
	}
	Ok(out)
}

/// Recently-read (finished) chapters, newest last_read_at first.
async fn run_recently_read(
	state: &AppState,
	source_id: i64,
	exclusions: &[String],
	limit: i64,
	offset: i64,
) -> ApiResult<Vec<Value>> {
	let mut clauses = String::new();
	for ex in exclusions {
		if !ex.is_empty() {
			clauses.push_str(" AND lower(m.key) NOT LIKE '%' || lower(?) || '%'");
		}
	}
	let sql = format!(
		"SELECT m.id, m.source_id, m.key, m.title, m.authors, m.artists, m.description,
		        m.tags, m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized,
		        m.update_strategy, m.next_update_time, m.last_fetched_at, m.chapters_last_fetched_at,
		        m.trashed_at, m.scan_mtime, m.created_at, m.updated_at,
		        c.id AS cid, c.manga_id AS cmanga_id, c.key AS ckey, c.title AS ctitle,
		        c.number AS cnumber, c.volume AS cvolume, c.scanlators AS cscanlators,
		        c.language AS clanguage, c.thumbnail AS cthumbnail, c.url AS curl,
		        c.date_uploaded AS cdate_uploaded, c.source_order AS csource_order,
		        c.page_count AS cpage_count, c.locked AS clocked, c.fetched_at AS cfetched_at,
		        c.created_at AS ccreated_at, c.trashed_at AS ctrashed_at,
		        cu.last_read_at
		 FROM chapter_user cu
		 JOIN chapter c ON c.id = cu.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 WHERE m.source_id = ? AND m.trashed_at IS NULL{clauses}
		   AND cu.user_id = ? AND cu.is_read = 1
		 ORDER BY cu.last_read_at DESC
		 LIMIT ? OFFSET ?"
	);
	let mut query = sqlx::query(&sql).bind(source_id);
	for ex in exclusions {
		if !ex.is_empty() {
			query = query.bind(ex);
		}
	}
	let rows = query.bind(USER_ID).bind(limit).bind(offset).fetch_all(&state.pool).await?;
	let mut out = Vec::new();
	for row in &rows {
		let joined = row_to_joined(row)?;
		let m = manga_to_json(&joined.manga)?;
		let c = chapter_to_json(&joined.chapter, joined.last_read_at)?;
		out.push(json!({ "manga": m, "chapter": c }));
	}
	Ok(out)
}

/// All manga in the source, sorted by title (case-insensitive). This is the default listing
/// third-party clients land on, so it must always render something even with no reading history.
async fn run_all_by_title(
	state: &AppState,
	source_id: i64,
	exclusions: &[String],
	limit: i64,
	offset: i64,
) -> ApiResult<Vec<Value>> {
	let mut clauses = String::new();
	for ex in exclusions {
		if !ex.is_empty() {
			clauses.push_str(" AND lower(m.key) NOT LIKE '%' || lower(?) || '%'");
		}
	}
	let sql = format!(
		"SELECT m.id, m.source_id, m.key, m.title, m.authors, m.artists, m.description,
		        m.tags, m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized,
		        m.update_strategy, m.next_update_time, m.last_fetched_at, m.chapters_last_fetched_at,
		        m.trashed_at, m.scan_mtime, m.created_at, m.updated_at
		 FROM manga m
		 WHERE m.source_id = ? AND m.trashed_at IS NULL{clauses}
		 ORDER BY m.title COLLATE NOCASE ASC, m.rowid ASC
		 LIMIT ? OFFSET ?"
	);
	let mut query = sqlx::query(&sql).bind(source_id);
	for ex in exclusions {
		if !ex.is_empty() {
			query = query.bind(ex);
		}
	}
	let rows = query.bind(limit).bind(offset).fetch_all(&state.pool).await?;
	let mut out = Vec::new();
	for row in &rows {
		let manga = MangaRow {
			id: row.try_get("id")?,
			source_id: row.try_get("source_id")?,
			key: row.try_get("key")?,
			title: row.try_get("title")?,
			authors: row.try_get("authors")?,
			artists: row.try_get("artists")?,
			description: row.try_get("description")?,
			tags: row.try_get("tags")?,
			status: row.try_get("status")?,
			content_rating: row.try_get("content_rating")?,
			viewer: row.try_get("viewer")?,
			cover_url: row.try_get("cover_url")?,
			url: row.try_get("url")?,
			initialized: row.try_get("initialized")?,
			update_strategy: row.try_get("update_strategy")?,
			next_update_time: row.try_get("next_update_time")?,
			last_fetched_at: row.try_get("last_fetched_at")?,
			chapters_last_fetched_at: row.try_get("chapters_last_fetched_at")?,
			trashed_at: row.try_get("trashed_at")?,
			scan_mtime: row.try_get("scan_mtime")?,
			created_at: row.try_get("created_at")?,
			updated_at: row.try_get("updated_at")?,
		};
		let m = manga_to_json(&manga)?;
		out.push(json!({
			"title": manga.title,
			"subtitle": null,
			"image_url": cover_url_for(manga.id),
			"value": { "Manga": m }
		}));
	}
	Ok(out)
}

/// Listing-page endpoints return bare Manga objects. The run_* helpers return home-page shapes:
/// - `{manga: m, chapter: c}` for MangaChapterList sections
/// - `{title, subtitle, image_url, value: {Manga: m}}` for Scroller sections
///
/// Strip the wrapper and project the inner Manga for the listing API.
fn unwrap_manga_entries(rows: Vec<Value>) -> Vec<Value> {
	rows.into_iter()
		.filter_map(|e| {
			if let Some(m) = e.get("manga") {
				return Some(m.clone());
			}
			if let Some(m) = e.pointer("/value/Manga") {
				return Some(m.clone());
			}
			None
		})
		.collect()
}

fn listing_ref(id: &str) -> Value {
	json!({ "id": id, "name": format!("sourceHome.{id}"), "kind": "List" })
}

fn listing_ref_default(id: &str) -> Value {
	json!({ "id": id, "name": format!("sourceHome.{id}"), "kind": "Default" })
}

/// Build the full HomeLayout for a local source. Empty sections are omitted (a source with
/// no reading history does not get an empty "Continue reading" header).
pub(crate) async fn build_local_home(
	state: &AppState,
	source_id: i64,
) -> ApiResult<Value> {
	let limit = SECTION_LIMIT;
	let exclusions = state.local_exclusions(source_id).await?;

	let mut components: Vec<Value> = Vec::new();

	let cr = run_continue_reading(state, source_id, &exclusions, limit, 0).await?;
	if !cr.is_empty() {
		components.push(json!({
			"title": "sourceHome.continueReading",
			"subtitle": null,
			"value": { "MangaChapterList": {
				"ranking": false,
				"page_size": 1,
				"entries": cr,
				"listing": listing_ref(LISTING_CONTINUE_READING)
			}}
		}));
	}

	let lu = run_latest_updates(state, source_id, &exclusions, limit, 0).await?;
	if !lu.is_empty() {
		components.push(json!({
			"title": "sourceHome.latestUpdates",
			"subtitle": null,
			"value": { "MangaChapterList": {
				"ranking": false,
				"page_size": 1,
				"entries": lu,
				"listing": listing_ref(LISTING_LATEST_UPDATES)
			}}
		}));
	}

	let ra = run_recently_added(state, source_id, &exclusions, None, limit, 0).await?;
	if !ra.is_empty() {
		components.push(json!({
			"title": "sourceHome.latestAdded",
			"subtitle": null,
			"value": { "Scroller": {
				"entries": ra,
				"listing": listing_ref_default(LISTING_LATEST_ADDED)
			}}
		}));
	}

	let rr = run_recently_read(state, source_id, &exclusions, limit, 0).await?;
	if !rr.is_empty() {
		components.push(json!({
			"title": "sourceHome.recentlyRead",
			"subtitle": null,
			"value": { "MangaChapterList": {
				"ranking": false,
				"page_size": 1,
				"entries": rr,
				"listing": listing_ref(LISTING_RECENTLY_READ)
			}}
		}));
	}

	Ok(json!({ "components": components }))
}

/// Serve one paginated listing for a local source. Returns `{entries, hasNextPage}`.
///
/// Row-shaped listings (continue-reading, latest-updates, recently-read) return
/// `{manga, chapter}` entries (MangaChapterList shape); grid-shaped listings (latest-added)
/// return `Link` entries (Scroller shape).
pub(crate) async fn local_listing_page(
	state: &AppState,
	source_id: i64,
	listing_id: &str,
	page: i32,
) -> ApiResult<Value> {
	let limit = SECTION_LIMIT;
	let offset = ((page - 1).max(0) as i64) * limit;
	let exclusions = state.local_exclusions(source_id).await?;

	let (entries, has_next) = match listing_id {
		LISTING_ALL => {
			let mut rows = run_all_by_title(state, source_id, &exclusions, limit + 1, offset).await?;
			let has_next = rows.len() as i64 > limit;
			rows.truncate(limit as usize);
			(unwrap_manga_entries(rows), has_next)
		}
		LISTING_CONTINUE_READING => {
			let mut rows = run_continue_reading(state, source_id, &exclusions, limit + 1, offset).await?;
			let has_next = rows.len() as i64 > limit;
			rows.truncate(limit as usize);
			(unwrap_manga_entries(rows), has_next)
		}
		LISTING_LATEST_UPDATES => {
			let mut rows = run_latest_updates(state, source_id, &exclusions, limit + 1, offset).await?;
			let has_next = rows.len() as i64 > limit;
			rows.truncate(limit as usize);
			(unwrap_manga_entries(rows), has_next)
		}
		LISTING_LATEST_ADDED => {
			let mut rows = run_recently_added(state, source_id, &exclusions, None, limit + 1, offset).await?;
			let has_next = rows.len() as i64 > limit;
			rows.truncate(limit as usize);
			(unwrap_manga_entries(rows), has_next)
		}
		LISTING_RECENTLY_READ => {
			let mut rows = run_recently_read(state, source_id, &exclusions, limit + 1, offset).await?;
			let has_next = rows.len() as i64 > limit;
			rows.truncate(limit as usize);
			(unwrap_manga_entries(rows), has_next)
		}
		_ => {
			return Err(ApiError::not_found("unknown listing"));
		}
	};

	Ok(json!({ "entries": entries, "hasNextPage": has_next }))
}
