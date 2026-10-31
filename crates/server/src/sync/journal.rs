//! Writes the local `sync_change` rows peers pull. Stores call this inside the transaction that
//! changes the row, so a row and its journal entry always commit together. The JSON is built in SQL
//! so the text matches events already published.

use sqlx::SqliteConnection;

use crate::error::ApiResult;

pub(crate) const INSERT_EVENT: &str =
	"INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)";

/// Each manga's identity and snapshot as peers receive them. Read as a subquery, `FROM {MANGA_STATE} sm`.
pub(crate) const MANGA_STATE: &str = "(SELECT
	m.id AS manga_id,
	m.source_id,
	m.last_modified_at,
	json_object(
		'mangaId', m.id,
		'sourceId', m.source_id,
		'sourceKey', s.key,
		'mangaKey', m.key
	) AS identity,
	json_object(
		'id', m.id,
		'sourceId', m.source_id,
		'key', m.key,
		'title', m.title,
		'authors', json(m.authors),
		'artists', json(m.artists),
		'description', m.description,
		'tags', json(m.tags),
		'status', m.status,
		'contentRating', m.content_rating,
		'viewer', m.viewer,
		'coverUrl', m.cover_url,
		'url', m.url,
		'initialized', m.initialized,
		'updateStrategy', m.update_strategy,
		'lastModifiedAt', m.last_modified_at
	) AS snapshot
	FROM manga m
	JOIN source s ON s.id = m.source_id)";

const MANGA_USER_SNAPSHOT: &str = "json_object(
	'mangaId', mu.manga_id,
	'userId', mu.user_id,
	'inLibrary', mu.in_library,
	'inLibraryAt', mu.in_library_at,
	'viewerFlags', mu.viewer_flags,
	'sortKey', mu.sort_key,
	'incognito', mu.incognito,
	'lastModifiedAt', mu.last_modified_at
)";

const CHAPTER_USER_SNAPSHOT: &str = "json_object(
	'chapterId', cu.chapter_id,
	'userId', cu.user_id,
	'lastPageRead', cu.last_page_read,
	'isRead', cu.is_read,
	'isBookmarked', cu.is_bookmarked,
	'lastReadAt', cu.last_read_at,
	'lastModifiedAt', cu.last_modified_at
)";

const MEMBERSHIP_SNAPSHOT: &str = "json_object(
	'categoryId', cm.category_id,
	'mangaId', cm.manga_id,
	'userId', cm.user_id,
	'lastModifiedAt', cm.last_modified_at
)";

/// The manga's identity, falling back to its last event once the manga row is gone
fn manga_identity_or_history(manga: &str, user: &str) -> String {
	format!(
		"COALESCE(
	(SELECT sm.identity FROM {MANGA_STATE} sm WHERE sm.manga_id = {manga}),
	(SELECT sc.identity FROM sync_change sc
	 WHERE sc.user_id = {user} AND sc.entity_kind = 'manga' AND sc.entity_key = CAST({manga} AS TEXT)
	 ORDER BY sc.sequence DESC LIMIT 1)
)"
	)
}

pub(crate) fn now_ms() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// The newest version this entity published, or 0
pub(crate) async fn latest_version(conn: &mut SqliteConnection, user_id: Option<i64>, kind: &str, key: &str) -> ApiResult<i64> {
	let latest: Option<i64> = sqlx::query_scalar(
		"SELECT MAX(modified_at) FROM sync_change
		 WHERE (?1 IS NULL OR user_id = ?1) AND entity_kind = ?2 AND entity_key = ?3",
	)
	.bind(user_id)
	.bind(kind)
	.bind(key)
	.fetch_one(conn)
	.await?;
	Ok(latest.unwrap_or(0))
}

/// The version for a write: the clock, pushed past anything this entity already published
pub(crate) async fn next_version(
	conn: &mut SqliteConnection,
	user_id: Option<i64>,
	kind: &str,
	key: &str,
	floor: i64,
) -> ApiResult<i64> {
	let latest = latest_version(conn, user_id, kind, key).await?;
	Ok(now_ms().max(floor).max(latest + 1))
}

/// The version a delete publishes
pub(crate) fn delete_version(last_modified_at: i64) -> i64 {
	now_ms().max(last_modified_at + 1)
}

/// Stamps new rows with this instance as their origin and records the key alias peers map through
pub(crate) async fn localize(conn: &mut SqliteConnection, sequences: &[i64]) -> ApiResult<()> {
	if sequences.is_empty() {
		return Ok(());
	}
	let instance_id = crate::server_state::get(&mut *conn, crate::server_state::INSTANCE_ID).await?;
	for sequence in sequences {
		sqlx::query(
			"UPDATE sync_change
			 SET origin = ?,
			     origin_user_id = user_id, origin_entity_key = entity_key, origin_sequence = sequence
			 WHERE sequence = ?",
		)
		.bind(&instance_id)
		.bind(sequence)
		.execute(&mut *conn)
		.await?;
		sqlx::query(
			"INSERT INTO sync_alias (origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key)
			 SELECT origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key
			 FROM sync_change WHERE sequence = ?
			 ON CONFLICT (origin, origin_user_id, user_id, entity_kind, origin_entity_key)
			 DO UPDATE SET entity_key = excluded.entity_key",
		)
		.bind(sequence)
		.execute(&mut *conn)
		.await?;
	}
	Ok(())
}

macro_rules! append {
	($conn:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
		let sql = format!("{} {} RETURNING sequence", $crate::sync::journal::INSERT_EVENT, $sql);
		let sequences: Vec<i64> = sqlx::query_scalar(&sql)$(.bind($bind))*.fetch_all(&mut *$conn).await?;
		$crate::sync::journal::localize($conn, &sequences).await
	}};
}

pub(crate) use append;

pub(crate) async fn manga_for_user(conn: &mut SqliteConnection, user_id: i64, manga_id: i64) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT ?1, 'manga', CAST(sm.manga_id AS TEXT), 0, sm.last_modified_at, sm.identity, sm.snapshot
			 FROM {MANGA_STATE} sm JOIN user u ON u.id = ?1 WHERE sm.manga_id = ?2"
		),
		user_id,
		manga_id,
	)
}

pub(crate) async fn manga_for_library(conn: &mut SqliteConnection, manga_id: i64) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT mu.user_id, 'manga', CAST(sm.manga_id AS TEXT), 0, sm.last_modified_at, sm.identity, sm.snapshot
			 FROM {MANGA_STATE} sm
			 JOIN manga_user mu ON mu.manga_id = sm.manga_id
			 JOIN user u ON u.id = mu.user_id
			 WHERE sm.manga_id = ?"
		),
		manga_id,
	)
}

const CHAPTER_FANS: &str = "(
	SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = ?2
	UNION
	SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = ?1
)";

pub(crate) async fn manga_for_chapter_fans(conn: &mut SqliteConnection, chapter_id: i64, manga_id: i64) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT fans.user_id, 'manga', CAST(sm.manga_id AS TEXT), 0, sm.last_modified_at, sm.identity, sm.snapshot
			 FROM {MANGA_STATE} sm JOIN {CHAPTER_FANS} fans ON 1 = 1 JOIN user u ON u.id = fans.user_id
			 WHERE sm.manga_id = ?2 ORDER BY fans.user_id"
		),
		chapter_id,
		manga_id,
	)
}

pub(crate) async fn chapter_for_fans(conn: &mut SqliteConnection, chapter_id: i64, manga_id: i64) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT fans.user_id, 'chapter', CAST(scs.chapter_id AS TEXT), 0,
			 CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER), scs.identity, scs.snapshot
			 FROM sync_chapter_state scs JOIN {CHAPTER_FANS} fans ON 1 = 1 JOIN user u ON u.id = fans.user_id
			 WHERE scs.chapter_id = ?1 ORDER BY fans.user_id"
		),
		chapter_id,
		manga_id,
	)
}

/// A progress change re-emits the parents for that user so a peer can attach it
pub(crate) async fn parents_of_progress(conn: &mut SqliteConnection, user_id: i64, chapter_id: i64) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT ?1, 'manga', CAST(sm.manga_id AS TEXT), 0, sm.last_modified_at, sm.identity, sm.snapshot
			 FROM sync_chapter_state scs
			 JOIN {MANGA_STATE} sm ON sm.manga_id = scs.manga_id
			 JOIN user u ON u.id = ?1
			 WHERE scs.chapter_id = ?2"
		),
		user_id,
		chapter_id,
	)?;
	append!(
		conn,
		"SELECT ?1, 'chapter', CAST(scs.chapter_id AS TEXT), 0,
		 CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER), scs.identity, scs.snapshot
		 FROM sync_chapter_state scs JOIN user u ON u.id = ?1 WHERE scs.chapter_id = ?2",
		user_id,
		chapter_id,
	)
}

pub(crate) async fn progress(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, deleted: Option<i64>) -> ApiResult<()> {
	append!(
		conn,
		format!(
			"SELECT cu.user_id, 'chapter_user', cu.chapter_id || ':' || cu.user_id, ?3 IS NOT NULL,
			 COALESCE(?3, cu.last_modified_at), json_set(scs.identity, '$.userId', cu.user_id), {CHAPTER_USER_SNAPSHOT}
			 FROM chapter_user cu
			 JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
			 JOIN user u ON u.id = cu.user_id
			 WHERE cu.chapter_id = ?1 AND cu.user_id = ?2"
		),
		chapter_id,
		user_id,
		deleted,
	)
}

pub(crate) async fn library_entry(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, deleted: Option<i64>) -> ApiResult<()> {
	let identity = manga_identity_or_history("mu.manga_id", "mu.user_id");
	append!(
		conn,
		format!(
			"SELECT mu.user_id, 'manga_user', mu.manga_id || ':' || mu.user_id, ?3 IS NOT NULL,
			 COALESCE(?3, mu.last_modified_at), json_set({identity}, '$.userId', mu.user_id), {MANGA_USER_SNAPSHOT}
			 FROM manga_user mu JOIN user u ON u.id = mu.user_id
			 WHERE mu.manga_id = ?1 AND mu.user_id = ?2"
		),
		manga_id,
		user_id,
		deleted,
	)
}

pub(crate) async fn membership(
	conn: &mut SqliteConnection,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
	deleted: Option<i64>,
) -> ApiResult<()> {
	let identity = manga_identity_or_history("cm.manga_id", "cm.user_id");
	append!(
		conn,
		format!(
			"SELECT cm.user_id, 'category_manga', cm.category_id || ':' || cm.manga_id || ':' || cm.user_id,
			 ?4 IS NOT NULL, COALESCE(?4, cm.last_modified_at),
			 json_set({identity}, '$.categoryId', cm.category_id, '$.userId', cm.user_id), {MEMBERSHIP_SNAPSHOT}
			 FROM category_manga cm JOIN user u ON u.id = cm.user_id
			 WHERE cm.category_id = ?1 AND cm.manga_id = ?2 AND cm.user_id = ?3"
		),
		category_id,
		manga_id,
		user_id,
		deleted,
	)
}

/// A delete cascades through these without firing anything, so their tombstones are written first
#[derive(sqlx::FromRow)]
struct LibraryKey {
	user_id: i64,
	last_modified_at: i64,
}

#[derive(sqlx::FromRow)]
struct MembershipKey {
	category_id: i64,
	user_id: i64,
	last_modified_at: i64,
}

/// Tombstones for the rows a manga delete cascades to, while they can still be read
pub(crate) async fn cascade_library_rows(conn: &mut SqliteConnection, manga_id: i64) -> ApiResult<()> {
	let memberships: Vec<MembershipKey> =
		sqlx::query_as("SELECT category_id, user_id, last_modified_at FROM category_manga WHERE manga_id = ? ORDER BY category_id, user_id")
			.bind(manga_id)
			.fetch_all(&mut *conn)
			.await?;
	for row in memberships {
		membership(conn, row.category_id, manga_id, row.user_id, Some(delete_version(row.last_modified_at))).await?;
	}
	let entries: Vec<LibraryKey> =
		sqlx::query_as("SELECT user_id, last_modified_at FROM manga_user WHERE manga_id = ? ORDER BY user_id")
			.bind(manga_id)
			.fetch_all(&mut *conn)
			.await?;
	for row in entries {
		library_entry(conn, manga_id, row.user_id, Some(delete_version(row.last_modified_at))).await?;
	}
	Ok(())
}
