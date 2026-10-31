use super::{SyncChange, WireChange, changes_after, insert_change, instance_id};
use crate::error::{ApiError, ApiResult};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
	pub origin_sequence: i64,
	pub status: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub reason: Option<&'static str>,
}

impl ApplyResult {
	fn new(origin_sequence: i64, status: &'static str) -> Self {
		Self {
			origin_sequence,
			status,
			reason: None,
		}
	}

	fn skipped(origin_sequence: i64) -> Self {
		Self {
			origin_sequence,
			status: "skipped",
			reason: Some("source_unavailable"),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Disposition {
	Applied,
	Stale,
	Skipped,
}

#[derive(Debug, Clone)]
struct Version {
	modified_at: i64,
	origin: String,
	origin_sequence: i64,
}

impl Version {
	fn cmp(&self, other: &Self) -> Ordering {
		self.modified_at
			.cmp(&other.modified_at)
			.then_with(|| self.origin.cmp(&other.origin))
			.then_with(|| self.origin_sequence.cmp(&other.origin_sequence))
	}
}

#[derive(Debug)]
struct CurrentEvent {
	version: Version,
	deleted: bool,
}

struct Finish {
	modified_at: i64,
	identity: Value,
	snapshot: Value,
	preserve_origin: bool,
}

pub async fn push(
	pool: &SqlitePool,
	envelope_origin: &str,
	authenticated_user_id: i64,
	changes: Vec<WireChange>,
) -> ApiResult<Vec<ApplyResult>> {
	let local_origin = instance_id(pool).await?;
	validate_envelope(&local_origin, envelope_origin, &changes)?;
	let mut results = Vec::with_capacity(changes.len());
	for change in changes {
		results.push(apply_one(pool, authenticated_user_id, change).await?);
	}
	Ok(results)
}

fn validate_envelope(local_origin: &str, envelope_origin: &str, changes: &[WireChange]) -> ApiResult<()> {
	if envelope_origin.is_empty() {
		return Err(ApiError::bad_request("origin must not be empty"));
	}
	if envelope_origin == local_origin {
		return Err(ApiError::bad_request("origin must not be the local instance"));
	}
	for change in changes {
		validate_change(change)?;
	}
	Ok(())
}

fn validate_change(change: &WireChange) -> ApiResult<()> {
	if change.origin.is_empty() || change.entity_key.is_empty() {
		return Err(ApiError::bad_request("change origin and entity key must not be empty"));
	}
	if change.origin_user_id <= 0 || change.origin_sequence <= 0 {
		return Err(ApiError::bad_request("origin user and sequence must be positive"));
	}
	if change.origin_user_id != crate::USER_ID {
		return Err(ApiError::bad_request("non-admin sync is unsupported"));
	}
	if change.modified_at < 0 {
		return Err(ApiError::bad_request("modifiedAt must not be negative"));
	}
	if !change.identity.is_object() || !change.snapshot.is_object() {
		return Err(ApiError::bad_request("identity and snapshot must be objects"));
	}
	let snapshot_time = integer(&change.snapshot, "lastModifiedAt")?;
	if change.deleted {
		if change.modified_at <= snapshot_time {
			return Err(ApiError::bad_request("tombstone modifiedAt must follow the snapshot version"));
		}
	} else if change.modified_at != snapshot_time {
		return Err(ApiError::bad_request("non-delete modifiedAt must equal snapshot lastModifiedAt"));
	}
	match change.entity_kind.as_str() {
		"manga" => {
			validate_manga_identity(&change.identity)?;
			validate_manga_snapshot(&change.snapshot)?;
			if string(&change.snapshot, "key")? != identity_string(&change.identity, "mangaKey")? {
				return Err(ApiError::bad_request("snapshot key must match identity mangaKey"));
			}
		}
		"manga_user" => {
			validate_manga_identity(&change.identity)?;
			validate_identity_user(change)?;
			validate_manga_user_snapshot(&change.snapshot)?;
		}
		"category" => {
			identity_id(&change.identity, "categoryId")?;
			validate_identity_user(change)?;
			validate_category_snapshot(&change.snapshot)?;
		}
		"category_manga" => {
			validate_manga_identity(&change.identity)?;
			identity_id(&change.identity, "categoryId")?;
			category_parent_origin(&change.identity)?;
			validate_identity_user(change)?;
		}
		"chapter" => {
			validate_chapter_identity(&change.identity)?;
			validate_chapter_snapshot(&change.snapshot)?;
			if string(&change.snapshot, "key")? != identity_string(&change.identity, "chapterKey")? {
				return Err(ApiError::bad_request("snapshot key must match identity chapterKey"));
			}
		}
		"chapter_user" => {
			validate_chapter_identity(&change.identity)?;
			validate_identity_user(change)?;
			validate_progress_snapshot(&change.snapshot)?;
		}
		_ => return Err(ApiError::bad_request("unsupported entity kind")),
	}
	Ok(())
}

fn validate_identity_user(change: &WireChange) -> ApiResult<()> {
	let user_id = identity_id(&change.identity, "userId")?;
	if user_id != change.origin_user_id {
		return Err(ApiError::bad_request("identity userId must match originUserId"));
	}
	Ok(())
}

fn validate_manga_identity(identity: &Value) -> ApiResult<()> {
	identity_string(identity, "sourceKey")?;
	identity_string(identity, "mangaKey")?;
	Ok(())
}

fn validate_chapter_identity(identity: &Value) -> ApiResult<()> {
	validate_manga_identity(identity)?;
	identity_string(identity, "chapterKey")?;
	Ok(())
}

fn validate_manga_snapshot(snapshot: &Value) -> ApiResult<()> {
	string(snapshot, "key")?;
	string(snapshot, "title")?;
	strings(snapshot, "authors")?;
	strings(snapshot, "artists")?;
	optional_string(snapshot, "description")?;
	strings(snapshot, "tags")?;
	integer(snapshot, "status")?;
	integer(snapshot, "contentRating")?;
	integer(snapshot, "viewer")?;
	optional_string(snapshot, "coverUrl")?;
	optional_string(snapshot, "url")?;
	flag(snapshot, "initialized")?;
	integer(snapshot, "updateStrategy")?;
	Ok(())
}

fn validate_manga_user_snapshot(snapshot: &Value) -> ApiResult<()> {
	flag(snapshot, "inLibrary")?;
	optional_integer(snapshot, "inLibraryAt")?;
	integer(snapshot, "viewerFlags")?;
	string(snapshot, "sortKey")?;
	flag(snapshot, "incognito")?;
	Ok(())
}

fn validate_category_snapshot(snapshot: &Value) -> ApiResult<()> {
	string(snapshot, "name")?;
	integer(snapshot, "sortOrder")?;
	flag(snapshot, "isDefault")?;
	if snapshot.get("includeInDownload").is_some() {
		integer(snapshot, "includeInDownload")?;
	}
	Ok(())
}

fn validate_chapter_snapshot(snapshot: &Value) -> ApiResult<()> {
	string(snapshot, "key")?;
	optional_string(snapshot, "title")?;
	optional_number(snapshot, "number")?;
	optional_number(snapshot, "volume")?;
	strings(snapshot, "scanlators")?;
	optional_string(snapshot, "language")?;
	optional_string(snapshot, "thumbnail")?;
	optional_string(snapshot, "url")?;
	optional_integer(snapshot, "dateUploaded")?;
	integer(snapshot, "sourceOrder")?;
	integer(snapshot, "pageCount")?;
	flag(snapshot, "locked")?;
	Ok(())
}

fn validate_progress_snapshot(snapshot: &Value) -> ApiResult<()> {
	integer(snapshot, "lastPageRead")?;
	flag(snapshot, "isRead")?;
	flag(snapshot, "isBookmarked")?;
	optional_integer(snapshot, "lastReadAt")?;
	Ok(())
}

fn field<'a>(value: &'a Value, key: &str) -> ApiResult<&'a Value> {
	value
		.get(key)
		.ok_or_else(|| ApiError::bad_request(format!("missing field {key}")))
}

fn integer(value: &Value, key: &str) -> ApiResult<i64> {
	field(value, key)?
		.as_i64()
		.ok_or_else(|| ApiError::bad_request(format!("field {key} must be an integer")))
}

fn optional_integer(value: &Value, key: &str) -> ApiResult<Option<i64>> {
	match value.get(key).unwrap_or(&Value::Null) {
		Value::Null => Ok(None),
		Value::Number(number) => number
			.as_i64()
			.map(Some)
			.ok_or_else(|| ApiError::bad_request(format!("field {key} must be an integer or null"))),
		_ => Err(ApiError::bad_request(format!("field {key} must be an integer or null"))),
	}
}

fn optional_number(value: &Value, key: &str) -> ApiResult<Option<f64>> {
	match value.get(key).unwrap_or(&Value::Null) {
		Value::Null => Ok(None),
		Value::Number(number) => number
			.as_f64()
			.map(Some)
			.ok_or_else(|| ApiError::bad_request(format!("field {key} must be a number or null"))),
		_ => Err(ApiError::bad_request(format!("field {key} must be a number or null"))),
	}
}

fn string(value: &Value, key: &str) -> ApiResult<String> {
	field(value, key)?
		.as_str()
		.map(str::to_string)
		.ok_or_else(|| ApiError::bad_request(format!("field {key} must be a string")))
}

fn optional_string(value: &Value, key: &str) -> ApiResult<Option<String>> {
	match value.get(key).unwrap_or(&Value::Null) {
		Value::Null => Ok(None),
		Value::String(text) => Ok(Some(text.clone())),
		_ => Err(ApiError::bad_request(format!("field {key} must be a string or null"))),
	}
}

fn strings(value: &Value, key: &str) -> ApiResult<Vec<String>> {
	let items = field(value, key)?
		.as_array()
		.ok_or_else(|| ApiError::bad_request(format!("field {key} must be an array")))?;
	items
		.iter()
		.map(|item| {
			item.as_str()
				.map(str::to_string)
				.ok_or_else(|| ApiError::bad_request(format!("field {key} must contain only strings")))
		})
		.collect()
}

fn flag(value: &Value, key: &str) -> ApiResult<bool> {
	match field(value, key)? {
		Value::Bool(flag) => Ok(*flag),
		Value::Number(number) => match number.as_i64() {
			Some(0) => Ok(false),
			Some(1) => Ok(true),
			_ => Err(ApiError::bad_request(format!("field {key} must be boolean"))),
		},
		_ => Err(ApiError::bad_request(format!("field {key} must be boolean"))),
	}
}

fn identity_string(identity: &Value, key: &str) -> ApiResult<String> {
	let value = string(identity, key)?;
	if value.is_empty() {
		return Err(ApiError::bad_request(format!("identity {key} must not be empty")));
	}
	Ok(value)
}

fn identity_id(identity: &Value, key: &str) -> ApiResult<i64> {
	let id = integer(identity, key)?;
	if id <= 0 {
		return Err(ApiError::bad_request(format!("identity {key} must be positive")));
	}
	Ok(id)
}

fn optional_identity_key(identity: &Value, key: &str) -> Option<String> {
	identity.get(key).and_then(|value| match value {
		Value::Number(number) => number.as_i64().map(|id| id.to_string()),
		Value::String(text) => Some(text.clone()),
		_ => None,
	})
}

async fn apply_one(pool: &SqlitePool, user_id: i64, change: WireChange) -> ApiResult<ApplyResult> {
	let mut tx = crate::db::begin_write(pool).await?;
	let prior: Option<i64> = sqlx::query_scalar(
		"SELECT origin_sequence FROM sync_origin_state
		 WHERE origin = ? AND origin_user_id = ? AND user_id = ? AND entity_kind = ? AND origin_entity_key = ?",
	)
	.bind(&change.origin)
	.bind(change.origin_user_id)
	.bind(user_id)
	.bind(&change.entity_kind)
	.bind(&change.entity_key)
	.fetch_optional(&mut *tx)
	.await?;
	if prior == Some(change.origin_sequence) {
		return Ok(ApplyResult::new(change.origin_sequence, "duplicate"));
	}
	if prior.is_some_and(|sequence| sequence > change.origin_sequence) {
		return Ok(ApplyResult::new(change.origin_sequence, "stale"));
	}
	let start_sequence: i64 = sqlx::query_scalar(
		"SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)",
	)
	.fetch_one(&mut *tx)
	.await?;
	let disposition = match change.entity_kind.as_str() {
		"manga" => apply_manga(&mut tx, start_sequence, user_id, &change).await?,
		"manga_user" => apply_manga_user(&mut tx, start_sequence, user_id, &change).await?,
		"category" => apply_category(&mut tx, start_sequence, user_id, &change).await?,
		"category_manga" => apply_category_manga(&mut tx, start_sequence, user_id, &change).await?,
		"chapter" => apply_chapter(&mut tx, start_sequence, user_id, &change).await?,
		"chapter_user" => apply_progress(&mut tx, start_sequence, user_id, &change).await?,
		_ => unreachable!("validated entity kind"),
	};
	if disposition == Disposition::Skipped {
		tx.rollback().await?;
		return Ok(ApplyResult::skipped(change.origin_sequence));
	}
	sqlx::query(
		"INSERT INTO sync_origin_state (origin, origin_user_id, user_id, entity_kind, origin_entity_key, origin_sequence)
		 VALUES (?, ?, ?, ?, ?, ?)
		 ON CONFLICT (origin, origin_user_id, user_id, entity_kind, origin_entity_key)
		 DO UPDATE SET origin_sequence = MAX(origin_sequence, excluded.origin_sequence)",
	)
	.bind(&change.origin)
	.bind(change.origin_user_id)
	.bind(user_id)
	.bind(&change.entity_kind)
	.bind(&change.entity_key)
	.bind(change.origin_sequence)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	let status = match disposition {
		Disposition::Applied => "applied",
		Disposition::Stale => "stale",
		Disposition::Skipped => unreachable!(),
	};
	Ok(ApplyResult::new(change.origin_sequence, status))
}

fn incoming_version(change: &WireChange) -> Version {
	Version {
		modified_at: change.modified_at,
		origin: change.origin.clone(),
		origin_sequence: change.origin_sequence,
	}
}

fn is_stale(change: &WireChange, current: &Option<CurrentEvent>) -> bool {
	current
		.as_ref()
		.is_some_and(|current| incoming_version(change).cmp(&current.version) != Ordering::Greater)
}

async fn upsert_alias(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	change: &WireChange,
	local_key: &str,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO sync_alias (origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key)
		 VALUES (?, ?, ?, ?, ?, ?)
		 ON CONFLICT (origin, origin_user_id, user_id, entity_kind, origin_entity_key)
		 DO UPDATE SET entity_key = excluded.entity_key",
	)
	.bind(&change.origin)
	.bind(change.origin_user_id)
	.bind(user_id)
	.bind(&change.entity_kind)
	.bind(&change.entity_key)
	.bind(local_key)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

async fn record_mapping(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	change: &WireChange,
	already_mapped: bool,
	local_key: Option<&str>,
) -> ApiResult<()> {
	let (false, Some(local_key)) = (already_mapped, local_key) else {
		return Ok(());
	};
	upsert_alias(tx, user_id, change, local_key).await
}

async fn mapped_key(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
	kind: &str,
	origin_entity_key: &str,
	shared: bool,
) -> ApiResult<Option<String>> {
	let sql = if shared {
		"SELECT entity_key FROM sync_alias
		 WHERE origin = ? AND origin_user_id = ? AND entity_kind = ? AND origin_entity_key = ? LIMIT 1"
	} else {
		"SELECT entity_key FROM sync_alias
		 WHERE origin = ? AND origin_user_id = ? AND user_id = ? AND entity_kind = ? AND origin_entity_key = ? LIMIT 1"
	};
	let mut query = sqlx::query_scalar(sql)
		.bind(&change.origin)
		.bind(change.origin_user_id);
	if !shared {
		query = query.bind(user_id);
	}
	Ok(query
		.bind(kind)
		.bind(origin_entity_key)
		.fetch_optional(&mut **tx)
		.await?)
}

async fn current_event(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	kind: &str,
	entity_key: &str,
	shared: bool,
) -> ApiResult<Option<CurrentEvent>> {
	let alias_aware = kind == "category_manga" && !shared;
	let sql = if alias_aware {
		"SELECT modified_at, origin, origin_sequence, deleted FROM sync_change sc
		 WHERE sc.user_id = ? AND sc.entity_kind = ?
		  AND (sc.entity_key = ? OR EXISTS (
		   SELECT 1 FROM sync_alias sa
		   WHERE sa.origin = sc.origin AND sa.origin_user_id = sc.origin_user_id AND sa.user_id = sc.user_id
		    AND sa.entity_kind = sc.entity_kind AND sa.origin_entity_key = sc.origin_entity_key AND sa.entity_key = ?
		  ))
		 ORDER BY modified_at DESC, origin DESC, origin_sequence DESC, sequence DESC LIMIT 1"
	} else if shared {
		"SELECT modified_at, origin, origin_sequence, deleted FROM sync_change
		 WHERE entity_kind = ? AND entity_key = ? ORDER BY sequence DESC LIMIT 1"
	} else {
		"SELECT modified_at, origin, origin_sequence, deleted FROM sync_change
		 WHERE user_id = ? AND entity_kind = ? AND entity_key = ? ORDER BY sequence DESC LIMIT 1"
	};
	let mut query = sqlx::query(sql);
	if !shared {
		query = query.bind(user_id);
	}
	query = query.bind(kind).bind(entity_key);
	if alias_aware {
		query = query.bind(entity_key);
	}
	let row = query.fetch_optional(&mut **tx).await?;
	Ok(row.map(|row| CurrentEvent {
		version: Version {
			modified_at: row.get(0),
			origin: row.get(1),
			origin_sequence: row.get(2),
		},
		deleted: row.get(3),
	}))
}

async fn finish(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
	local_key: &str,
	stubs: HashSet<(String, String)>,
	finish: Finish,
) -> ApiResult<()> {
	let automatic = changes_after(tx, start_sequence).await?;
	sqlx::query("DELETE FROM sync_change WHERE sequence > ?")
		.bind(start_sequence)
		.execute(&mut **tx)
		.await?;
	if !change.deleted {
		set_final_version(tx, &change.entity_kind, local_key, finish.modified_at).await?;
	}
	let shared = matches!(change.entity_kind.as_str(), "manga" | "chapter");
	let mut users = BTreeSet::new();
	for event in &automatic {
		if event.entity_kind == change.entity_kind && event.entity_key == local_key && event.deleted == change.deleted {
			users.insert(event.user_id);
		}
	}
	if shared && !change.deleted {
		users.extend(fanout_users(tx, user_id, &change.entity_kind, local_key).await?);
	}
	users.insert(user_id);
	for mut event in automatic {
		if stubs.contains(&(event.entity_kind.clone(), event.entity_key.clone())) {
			continue;
		}
		if event.entity_kind == change.entity_kind && event.entity_key == local_key && event.deleted == change.deleted {
			continue;
		}
		event.sequence = 0;
		event.origin.clear();
		event.origin_user_id = 0;
		event.origin_entity_key.clear();
		event.origin_sequence = 0;
		insert_change(tx, &event).await?;
	}
	for event_user in users {
		let (origin, origin_user_id, origin_entity_key, origin_sequence) = if finish.preserve_origin {
			(
				change.origin.clone(),
				change.origin_user_id,
				change.entity_key.clone(),
				change.origin_sequence,
			)
		} else {
			(String::new(), 0, String::new(), 0)
		};
		let event = SyncChange {
			sequence: 0,
			user_id: event_user,
			entity_kind: change.entity_kind.clone(),
			entity_key: local_key.to_string(),
			origin,
			origin_user_id,
			origin_entity_key,
			origin_sequence,
			deleted: change.deleted,
			modified_at: finish.modified_at,
			identity: finish.identity.clone(),
			snapshot: finish.snapshot.clone(),
		};
		upsert_alias(tx, event_user, change, local_key).await?;
		insert_change(tx, &event).await?;
	}
	Ok(())
}

async fn fanout_users(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	kind: &str,
	local_key: &str,
) -> ApiResult<BTreeSet<i64>> {
	let id: i64 = local_key
		.parse()
		.map_err(|_| ApiError::bad_request("invalid local entity key"))?;
	let mut users = BTreeSet::from([user_id]);
	let rows: Vec<i64> = match kind {
		"manga" => sqlx::query_scalar(
			"SELECT user_id FROM manga_user WHERE manga_id = ?
			 UNION SELECT cu.user_id FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id WHERE c.manga_id = ?
			 UNION SELECT user_id FROM category_manga WHERE manga_id = ?",
		)
		.bind(id)
		.bind(id)
		.bind(id)
		.fetch_all(&mut **tx)
		.await?,
		"chapter" => sqlx::query_scalar(
			"SELECT mu.user_id FROM chapter c JOIN manga_user mu ON mu.manga_id = c.manga_id WHERE c.id = ?
			 UNION SELECT user_id FROM chapter_user WHERE chapter_id = ?
			 UNION SELECT cm.user_id FROM chapter c JOIN category_manga cm ON cm.manga_id = c.manga_id WHERE c.id = ?",
		)
		.bind(id)
		.bind(id)
		.bind(id)
		.fetch_all(&mut **tx)
		.await?,
		_ => Vec::new(),
	};
	users.extend(rows);
	Ok(users)
}

pub(crate) async fn set_final_version(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	kind: &str,
	local_key: &str,
	modified_at: i64,
) -> ApiResult<()> {
	match kind {
		"manga" => sqlx::query("UPDATE manga SET last_modified_at = ? WHERE id = ?")
			.bind(modified_at)
			.bind(local_key.parse::<i64>().map_err(|_| ApiError::bad_request("invalid manga key"))?)
			.execute(&mut **tx)
			.await?,
		"chapter" => sqlx::query("UPDATE chapter SET last_modified_at = ? WHERE id = ?")
			.bind(modified_at)
			.bind(local_key.parse::<i64>().map_err(|_| ApiError::bad_request("invalid chapter key"))?)
			.execute(&mut **tx)
			.await?,
		"category" => {
			let id = local_key.parse::<i64>().map_err(|_| ApiError::bad_request("invalid category key"))?;
			crate::store::category::set_modified_at(&mut **tx, id, modified_at).await?
		}
		"manga_user" => {
			let (manga_id, user_id) = relation_key(local_key, 2)?;
			sqlx::query("UPDATE manga_user SET last_modified_at = ? WHERE manga_id = ? AND user_id = ?")
				.bind(modified_at)
				.bind(manga_id[0])
				.bind(user_id[0])
				.execute(&mut **tx)
				.await?
		}
		"chapter_user" => {
			let (chapter_id, user_id) = relation_key(local_key, 2)?;
			sqlx::query("UPDATE chapter_user SET last_modified_at = ? WHERE chapter_id = ? AND user_id = ?")
				.bind(modified_at)
				.bind(chapter_id[0])
				.bind(user_id[0])
				.execute(&mut **tx)
				.await?
		}
		"category_manga" => {
			let (category_id, remainder) = relation_key(local_key, 3)?;
			crate::store::category::set_manga_modified_at(&mut **tx, category_id[0], category_id[1], remainder[0], modified_at).await?
		}
		_ => return Err(ApiError::bad_request("unsupported entity kind")),
	};
	Ok(())
}

fn relation_key(key: &str, parts: usize) -> ApiResult<(Vec<i64>, Vec<i64>)> {
	let ids: Vec<i64> = key
		.split(':')
		.map(|part| {
			part.parse::<i64>()
				.map_err(|_| ApiError::bad_request("invalid relation entity key"))
		})
		.collect::<ApiResult<_>>()?;
	if ids.len() != parts {
		return Err(ApiError::bad_request("invalid relation entity key"));
	}
	Ok((ids[..ids.len() - 1].to_vec(), ids[ids.len() - 1..].to_vec()))
}

async fn source_id(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, key: &str) -> ApiResult<Option<i64>> {
	Ok(sqlx::query_scalar("SELECT id FROM source WHERE key = ?")
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?)
}

async fn row_exists(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	table: &str,
	id: i64,
) -> ApiResult<bool> {
	let sql = match table {
		"manga" => "SELECT EXISTS(SELECT 1 FROM manga WHERE id = ?)",
		"chapter" => "SELECT EXISTS(SELECT 1 FROM chapter WHERE id = ?)",
		"category" => "SELECT EXISTS(SELECT 1 FROM category WHERE id = ?)",
		_ => return Err(ApiError::bad_request("invalid table")),
	};
	Ok(sqlx::query_scalar(sql).bind(id).fetch_one(&mut **tx).await?)
}

async fn mark_quiet_stub(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	kind: &str,
	id: i64,
	stubs: &mut HashSet<(String, String)>,
) -> ApiResult<()> {
	let table = match kind {
		"manga" => "manga",
		"chapter" => "chapter",
		"category" => "category",
		_ => return Err(ApiError::bad_request("invalid stub kind")),
	};
	let version: i64 = sqlx::query_scalar(&format!("SELECT last_modified_at FROM {table} WHERE id = ?"))
		.bind(id)
		.fetch_one(&mut **tx)
		.await?;
	if version == 0 {
		let has_event: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sync_change WHERE entity_kind = ? AND entity_key = ?)")
			.bind(kind)
			.bind(id.to_string())
			.fetch_one(&mut **tx)
			.await?;
		if !has_event {
			stubs.insert((kind.to_string(), id.to_string()));
		}
	}
	Ok(())
}

async fn ensure_manga(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	preferred_id: Option<i64>,
	stubs: &mut HashSet<(String, String)>,
) -> ApiResult<Option<i64>> {
	let source_key = identity_string(&change.identity, "sourceKey")?;
	let manga_key = identity_string(&change.identity, "mangaKey")?;
	let Some(source_id) = source_id(tx, &source_key).await? else {
		return Ok(None);
	};
	if let Some(id) = preferred_id
		&& row_exists(tx, "manga", id).await?
	{
		mark_quiet_stub(tx, "manga", id, stubs).await?;
		return Ok(Some(id));
	}
	if let Some(id) = preferred_id {
		sqlx::query("INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, ?, '', 0)")
			.bind(id)
			.bind(source_id)
			.bind(&manga_key)
			.execute(&mut **tx)
			.await?;
		sqlx::query("UPDATE manga SET last_modified_at = 0 WHERE id = ?")
			.bind(id)
			.execute(&mut **tx)
			.await?;
		stubs.insert(("manga".into(), id.to_string()));
		return Ok(Some(id));
	}
	if let Some(id) = sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(&manga_key)
		.fetch_optional(&mut **tx)
		.await?
	{
		mark_quiet_stub(tx, "manga", id, stubs).await?;
		return Ok(Some(id));
	}
	let id: i64 = sqlx::query_scalar(
		"INSERT INTO manga (source_id, key, title, last_modified_at) VALUES (?, ?, '', 0) RETURNING id",
	)
	.bind(source_id)
	.bind(&manga_key)
	.fetch_one(&mut **tx)
	.await?;
	sqlx::query("UPDATE manga SET last_modified_at = 0 WHERE id = ?")
		.bind(id)
		.execute(&mut **tx)
		.await?;
	stubs.insert(("manga".into(), id.to_string()));
	Ok(Some(id))
}

async fn ensure_chapter(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	preferred_manga_id: Option<i64>,
	preferred_chapter_id: Option<i64>,
	stubs: &mut HashSet<(String, String)>,
) -> ApiResult<Option<i64>> {
	let Some(manga_id) = ensure_manga(tx, change, preferred_manga_id, stubs).await? else {
		return Ok(None);
	};
	let chapter_key = identity_string(&change.identity, "chapterKey")?;
	if let Some(id) = preferred_chapter_id
		&& row_exists(tx, "chapter", id).await?
	{
		mark_quiet_stub(tx, "chapter", id, stubs).await?;
		return Ok(Some(id));
	}
	if let Some(id) = preferred_chapter_id {
		sqlx::query("INSERT INTO chapter (id, manga_id, key, last_modified_at) VALUES (?, ?, ?, 0)")
			.bind(id)
			.bind(manga_id)
			.bind(&chapter_key)
			.execute(&mut **tx)
			.await?;
		sqlx::query("UPDATE chapter SET last_modified_at = 0 WHERE id = ?")
			.bind(id)
			.execute(&mut **tx)
			.await?;
		stubs.insert(("chapter".into(), id.to_string()));
		return Ok(Some(id));
	}
	if let Some(id) = sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(&chapter_key)
		.fetch_optional(&mut **tx)
		.await?
	{
		mark_quiet_stub(tx, "chapter", id, stubs).await?;
		return Ok(Some(id));
	}
	let id: i64 = sqlx::query_scalar(
		"INSERT INTO chapter (manga_id, key, last_modified_at) VALUES (?, ?, 0) RETURNING id",
	)
	.bind(manga_id)
	.bind(&chapter_key)
	.fetch_one(&mut **tx)
	.await?;
	sqlx::query("UPDATE chapter SET last_modified_at = 0 WHERE id = ?")
		.bind(id)
		.execute(&mut **tx)
		.await?;
	stubs.insert(("chapter".into(), id.to_string()));
	Ok(Some(id))
}

async fn ensure_category(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
	preferred_id: Option<i64>,
	stubs: &mut HashSet<(String, String)>,
) -> ApiResult<i64> {
	if let Some(id) = preferred_id
		&& row_exists(tx, "category", id).await?
	{
		mark_quiet_stub(tx, "category", id, stubs).await?;
		return Ok(id);
	}
	let natural_id = identity_id(&change.identity, "categoryId")?;
	let id = preferred_id.unwrap_or(natural_id);
	let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM category WHERE id = ?)")
		.bind(id)
		.fetch_one(&mut **tx)
		.await?;
	let id = if occupied {
		crate::store::category::insert_stub(&mut **tx, user_id).await?
	} else {
		crate::store::category::insert_stub_with_id(&mut **tx, id, user_id).await?
	};
	crate::store::category::set_modified_at(&mut **tx, id, 0).await?;
	stubs.insert(("category".into(), id.to_string()));
	Ok(id)
}


async fn parent_manga_id(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
) -> ApiResult<Option<i64>> {
	if let Some(origin_key) = optional_identity_key(&change.identity, "mangaId")
		&& let Some(key) = mapped_key(tx, change, user_id, "manga", &origin_key, true).await?
	{
		return key
			.parse::<i64>()
			.map(Some)
			.map_err(|_| ApiError::bad_request("invalid mapped manga key"));
	}
	let source_key = identity_string(&change.identity, "sourceKey")?;
	let manga_key = identity_string(&change.identity, "mangaKey")?;
	let Some(source_id) = source_id(tx, &source_key).await? else {
		return Ok(None);
	};
	Ok(sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(manga_key)
		.fetch_optional(&mut **tx)
		.await?)
}

async fn parent_chapter_id(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
) -> ApiResult<Option<i64>> {
	if let Some(origin_key) = optional_identity_key(&change.identity, "chapterId")
		&& let Some(key) = mapped_key(tx, change, user_id, "chapter", &origin_key, true).await?
	{
		return key
			.parse::<i64>()
			.map(Some)
			.map_err(|_| ApiError::bad_request("invalid mapped chapter key"));
	}
	let Some(manga_id) = parent_manga_id(tx, change, user_id).await? else {
		return Ok(None);
	};
	let chapter_key = identity_string(&change.identity, "chapterKey")?;
	Ok(sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(chapter_key)
		.fetch_optional(&mut **tx)
		.await?)
}

fn category_parent_origin(identity: &Value) -> ApiResult<Option<(String, i64, String)>> {
	if identity.get("categoryOrigin").is_none() {
		return Ok(None);
	}
	Ok(Some((
		identity_string(identity, "categoryOrigin")?,
		identity_id(identity, "categoryOriginUserId")?,
		identity_string(identity, "categoryOriginEntityKey")?,
	)))
}

async fn category_from_membership(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
) -> ApiResult<Option<i64>> {
	let category_id = identity_id(&change.identity, "categoryId")?;
	let parent_origin = if change.entity_kind == "category" {
		Some((change.origin.clone(), change.origin_user_id, change.entity_key.clone()))
	} else {
		category_parent_origin(&change.identity)?
	};
	let mut key: Option<String> = None;
	if let Some((origin, origin_user_id, origin_entity_key)) = parent_origin {
		key = sqlx::query_scalar(
			"SELECT entity_key FROM sync_change
			 WHERE user_id = ? AND entity_kind = 'category_manga'
			  AND json_extract(identity, '$.categoryOrigin') = ?
			  AND json_extract(identity, '$.categoryOriginUserId') = ?
			  AND json_extract(identity, '$.categoryOriginEntityKey') = ?
			 ORDER BY sequence DESC LIMIT 1",
		)
		.bind(user_id)
		.bind(origin)
		.bind(origin_user_id)
		.bind(origin_entity_key)
		.fetch_optional(&mut **tx)
		.await?;
	}
	if key.is_none() {
		key = sqlx::query_scalar(
			"SELECT entity_key FROM sync_change
			 WHERE origin = ? AND origin_user_id = ? AND user_id = ? AND entity_kind = 'category_manga'
			  AND json_extract(identity, '$.categoryId') = ?
			 ORDER BY sequence DESC LIMIT 1",
		)
		.bind(&change.origin)
		.bind(change.origin_user_id)
		.bind(user_id)
		.bind(category_id)
		.fetch_optional(&mut **tx)
		.await?;
	}
	key
		.as_deref()
		.and_then(|key| key.split(':').next())
		.map(str::parse::<i64>)
		.transpose()
		.map_err(|_| ApiError::bad_request("invalid mapped category key"))
}

async fn parent_category_id(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &WireChange,
	user_id: i64,
) -> ApiResult<Option<i64>> {
	let origin_key = identity_id(&change.identity, "categoryId")?.to_string();
	if let Some(key) = mapped_key(tx, change, user_id, "category", &origin_key, false).await? {
		return key
			.parse::<i64>()
			.map(Some)
			.map_err(|_| ApiError::bad_request("invalid mapped category key"));
	}
	if let Some((origin, origin_user_id, origin_entity_key)) = category_parent_origin(&change.identity)? {
		let key: Option<String> = sqlx::query_scalar(
			"SELECT entity_key FROM sync_alias
			 WHERE origin = ? AND origin_user_id = ? AND user_id = ? AND entity_kind = 'category' AND origin_entity_key = ?",
		)
		.bind(origin)
		.bind(origin_user_id)
		.bind(user_id)
		.bind(origin_entity_key)
		.fetch_optional(&mut **tx)
		.await?;
		if let Some(key) = key {
			return key
				.parse::<i64>()
				.map(Some)
				.map_err(|_| ApiError::bad_request("invalid mapped category key"));
		}
	}
	if let Some(id) = category_from_membership(tx, change, user_id).await? {
		return Ok(Some(id));
	}
	Ok(None)
}

async fn apply_manga(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mapped = mapped_key(tx, change, user_id, "manga", &change.entity_key, true).await?;
	let mapped_id = mapped
		.as_deref()
		.map(str::parse::<i64>)
		.transpose()
		.map_err(|_| ApiError::bad_request("invalid mapped manga key"))?;
	let source_key = identity_string(&change.identity, "sourceKey")?;
	let source_id = source_id(tx, &source_key).await?;
	let natural_id = match source_id {
		Some(source_id) => {
			let manga_key = identity_string(&change.identity, "mangaKey")?;
			sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND key = ?")
				.bind(source_id)
				.bind(manga_key)
				.fetch_optional(&mut **tx)
				.await?
		}
		None => None,
	};
	let id = mapped_id.or(natural_id);
	let exists = match id {
		Some(id) => row_exists(tx, "manga", id).await?,
		None => false,
	};
	let current = match id {
		Some(id) => current_event(tx, user_id, "manga", &id.to_string(), true).await?,
		None => None,
	};
	if is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped_id.is_some(), id.map(|id| id.to_string()).as_deref())
			.await?;
		return Ok(Disposition::Stale);
	}
	if change.deleted && mapped_id.is_none() && source_id.is_none() {
		return Ok(Disposition::Skipped);
	}
	if change.deleted {
		let Some(id) = id else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		sqlx::query("DELETE FROM manga WHERE id = ?")
			.bind(id)
			.execute(&mut **tx)
			.await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&id.to_string(),
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot: change.snapshot.clone(),
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	let mut stubs = HashSet::new();
	let Some(id) = ensure_manga(tx, change, id, &mut stubs).await? else {
		return Ok(Disposition::Skipped);
	};
	let source_id = source_id.ok_or_else(|| ApiError::bad_request("source is unavailable"))?;
	sqlx::query(
		"UPDATE manga SET source_id = ?, key = ?, title = ?, authors = ?, artists = ?, description = ?, tags = ?,
		 status = ?, content_rating = ?, viewer = ?, cover_url = ?, url = ?, initialized = ?, update_strategy = ?
		 WHERE id = ?",
	)
	.bind(source_id)
	.bind(string(&change.snapshot, "key")?)
	.bind(string(&change.snapshot, "title")?)
	.bind(serde_json::to_string(&strings(&change.snapshot, "authors")?)?)
	.bind(serde_json::to_string(&strings(&change.snapshot, "artists")?)?)
	.bind(optional_string(&change.snapshot, "description")?)
	.bind(serde_json::to_string(&strings(&change.snapshot, "tags")?)?)
	.bind(integer(&change.snapshot, "status")?)
	.bind(integer(&change.snapshot, "contentRating")?)
	.bind(integer(&change.snapshot, "viewer")?)
	.bind(optional_string(&change.snapshot, "coverUrl")?)
	.bind(optional_string(&change.snapshot, "url")?)
	.bind(flag(&change.snapshot, "initialized")?)
	.bind(integer(&change.snapshot, "updateStrategy")?)
	.bind(id)
	.execute(&mut **tx)
	.await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&id.to_string(),
		stubs,
		Finish {
			modified_at: change.modified_at,
			identity: change.identity.clone(),
			snapshot: change.snapshot.clone(),
			preserve_origin: true,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}

async fn apply_chapter(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mapped = mapped_key(tx, change, user_id, "chapter", &change.entity_key, true).await?;
	let mapped_id = mapped
		.as_deref()
		.map(str::parse::<i64>)
		.transpose()
		.map_err(|_| ApiError::bad_request("invalid mapped chapter key"))?;
	let source_key = identity_string(&change.identity, "sourceKey")?;
	let source_available = source_id(tx, &source_key).await?.is_some();
	let parent_id = parent_manga_id(tx, change, user_id).await?;
	let natural_id = match parent_id {
		Some(manga_id) => {
			let chapter_key = identity_string(&change.identity, "chapterKey")?;
			sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
				.bind(manga_id)
				.bind(chapter_key)
				.fetch_optional(&mut **tx)
				.await?
		}
		None => None,
	};
	let id = mapped_id.or(natural_id);
	let exists = match id {
		Some(id) => row_exists(tx, "chapter", id).await?,
		None => false,
	};
	let current = match id {
		Some(id) => current_event(tx, user_id, "chapter", &id.to_string(), true).await?,
		None => None,
	};
	if is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped_id.is_some(), id.map(|id| id.to_string()).as_deref())
			.await?;
		return Ok(Disposition::Stale);
	}
	if change.deleted && mapped_id.is_none() && !source_available {
		return Ok(Disposition::Skipped);
	}
	if change.deleted {
		let Some(id) = id else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		sqlx::query("DELETE FROM chapter WHERE id = ?")
			.bind(id)
			.execute(&mut **tx)
			.await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&id.to_string(),
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot: change.snapshot.clone(),
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	let mut stubs = HashSet::new();
	let Some(id) = ensure_chapter(tx, change, parent_id, id, &mut stubs).await? else {
		return Ok(Disposition::Skipped);
	};
	sqlx::query(
		"UPDATE chapter SET key = ?, title = ?, number = ?, volume = ?, scanlators = ?, language = ?,
		 thumbnail = ?, url = ?, date_uploaded = ?, source_order = ?, page_count = ?, locked = ? WHERE id = ?",
	)
	.bind(string(&change.snapshot, "key")?)
	.bind(optional_string(&change.snapshot, "title")?)
	.bind(optional_number(&change.snapshot, "number")?)
	.bind(optional_number(&change.snapshot, "volume")?)
	.bind(serde_json::to_string(&strings(&change.snapshot, "scanlators")?)?)
	.bind(optional_string(&change.snapshot, "language")?)
	.bind(optional_string(&change.snapshot, "thumbnail")?)
	.bind(optional_string(&change.snapshot, "url")?)
	.bind(optional_integer(&change.snapshot, "dateUploaded")?)
	.bind(integer(&change.snapshot, "sourceOrder")?)
	.bind(integer(&change.snapshot, "pageCount")?)
	.bind(flag(&change.snapshot, "locked")?)
	.bind(id)
	.execute(&mut **tx)
	.await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&id.to_string(),
		stubs,
		Finish {
			modified_at: change.modified_at,
			identity: change.identity.clone(),
			snapshot: change.snapshot.clone(),
			preserve_origin: true,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}

async fn derived_membership_event(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<SyncChange> {
	let identity: String = sqlx::query_scalar("SELECT identity FROM sync_manga_state WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_one(&mut **tx)
		.await?;
	let mut identity: Value = serde_json::from_str(&identity)?;
	identity["categoryId"] = json!(category_id);
	identity["userId"] = json!(user_id);
	Ok(SyncChange {
		sequence: 0,
		user_id,
		entity_kind: "category_manga".into(),
		entity_key: format!("{category_id}:{manga_id}:{user_id}"),
		origin: String::new(),
		origin_user_id: 0,
		origin_entity_key: String::new(),
		origin_sequence: 0,
		deleted: false,
		modified_at,
		identity,
		snapshot: json!({
			"categoryId": category_id,
			"mangaId": manga_id,
			"userId": user_id,
			"lastModifiedAt": modified_at,
		}),
	})
}

async fn merge_category_stub(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	stub_id: i64,
	target_id: i64,
) -> ApiResult<bool> {
	// Only a zero-version placeholder can be coalesced into an existing named category.
	let is_stub: bool = sqlx::query_scalar("SELECT last_modified_at = 0 FROM category WHERE id = ?")
		.bind(stub_id)
		.fetch_one(&mut **tx)
		.await?;
	if !is_stub {
		return Ok(false);
	}
	let state = |event: Option<CurrentEvent>, row_version: Option<i64>| {
		let mut state = event.unwrap_or(CurrentEvent {
			version: Version {
				modified_at: 0,
				origin: String::new(),
				origin_sequence: 0,
			},
			deleted: false,
		});
		if let Some(row_version) = row_version
			&& row_version > state.version.modified_at
		{
			state.version = Version {
				modified_at: row_version,
				origin: String::new(),
				origin_sequence: 0,
			};
			state.deleted = false;
		}
		state
	};
	let stub_prefix = format!("{stub_id}:");
	let stub_keys: Vec<String> = sqlx::query_scalar(
		"SELECT entity_key FROM sync_change WHERE entity_kind = 'category_manga' AND entity_key LIKE ?
		 UNION SELECT category_id || ':' || manga_id || ':' || user_id FROM category_manga WHERE category_id = ?
		 ORDER BY 1",
	)
	.bind(format!("{stub_id}:%"))
	.bind(stub_id)
	.fetch_all(&mut **tx)
	.await?;
	let mut derived_events = Vec::new();
	for stub_key in stub_keys {
		let ids = key_ids(&stub_key)?;
		let target_key = format!("{target_id}:{}:{}", ids[1], ids[2]);
		let stub_row_version: Option<i64> = sqlx::query_scalar(
			"SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?",
		)
		.bind(stub_id)
		.bind(ids[1])
		.bind(ids[2])
		.fetch_optional(&mut **tx)
		.await?;
		let target_row_version: Option<i64> = sqlx::query_scalar(
			"SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?",
		)
		.bind(target_id)
		.bind(ids[1])
		.bind(ids[2])
		.fetch_optional(&mut **tx)
		.await?;
		let stub_event = current_event(tx, ids[2], "category_manga", &stub_key, false).await?;
		let target_event = current_event(tx, ids[2], "category_manga", &target_key, false).await?;
		let stub_derived_version = stub_row_version.filter(|row_version| {
			stub_event.as_ref().is_none_or(|event| *row_version > event.version.modified_at)
		});
		let target_derived_version = target_row_version.filter(|row_version| {
			target_event.as_ref().is_none_or(|event| *row_version > event.version.modified_at)
		});
		let stub_state = state(stub_event, stub_row_version);
		let target_state = state(target_event, target_row_version);
		if stub_state.version.cmp(&target_state.version) != Ordering::Greater {
			if let Some(modified_at) = target_derived_version {
				derived_events.push(derived_membership_event(tx, target_id, ids[1], ids[2], modified_at).await?);
			}
			continue;
		}
		if stub_state.deleted {
			crate::store::category::remove_manga(&mut **tx, target_id, ids[1], ids[2]).await?;
		} else {
			crate::store::category::upsert_manga_at(&mut **tx, target_id, ids[1], ids[2], stub_state.version.modified_at).await?;
			if let Some(modified_at) = stub_derived_version {
				derived_events.push(derived_membership_event(tx, target_id, ids[1], ids[2], modified_at).await?);
			}
		}
	}
	crate::store::category::clear_category(&mut **tx, stub_id).await?;
	// Rekeying keeps the original events; trigger-generated membership copies would replace their provenance.
	sqlx::query("DELETE FROM sync_change WHERE sequence > ? AND entity_kind = 'category_manga'")
		.bind(start_sequence)
		.execute(&mut **tx)
		.await?;
	for event in derived_events {
		insert_change(tx, &event).await?;
	}
	sqlx::query(
		"UPDATE sync_change SET entity_key = ? || substr(entity_key, length(?) + 1)
		 WHERE entity_kind = 'category_manga' AND deleted = 0 AND entity_key LIKE ?",
	)
	.bind(format!("{target_id}:"))
	.bind(&stub_prefix)
	.bind(format!("{stub_id}:%"))
	.execute(&mut **tx)
	.await?;
	sqlx::query(
		"UPDATE sync_alias SET entity_key = ? || substr(entity_key, length(?) + 1)
		 WHERE entity_kind = 'category_manga' AND entity_key LIKE ?",
	)
	.bind(format!("{target_id}:"))
	.bind(&stub_prefix)
	.bind(format!("{stub_id}:%"))
	.execute(&mut **tx)
	.await?;
	crate::settings::remove_category_exclusions(tx, stub_id).await?;
	crate::store::category::delete(&mut **tx, stub_id).await?;
	Ok(true)
}

async fn apply_category(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mut snapshot = change.snapshot.clone();
	if let Some(object) = snapshot.as_object_mut() {
		object.insert("includeInDownload".into(), Value::from(0));
	}
	let mapped = mapped_key(tx, change, user_id, "category", &change.entity_key, false).await?;
	let mapped_id = mapped
		.as_deref()
		.map(str::parse::<i64>)
		.transpose()
		.map_err(|_| ApiError::bad_request("invalid mapped category key"))?;
	let child_mapped = category_from_membership(tx, change, user_id).await?;
	let natural_id = sqlx::query_scalar("SELECT id FROM category WHERE user_id = ? AND name = ?")
		.bind(user_id)
		.bind(string(&snapshot, "name")?)
		.fetch_optional(&mut **tx)
		.await?;
	let mut id = mapped_id.or(child_mapped).or(natural_id);
	if let (Some(stub_id), Some(target_id)) = (id, natural_id)
		&& stub_id != target_id
		&& merge_category_stub(tx, start_sequence, stub_id, target_id).await?
	{
		id = Some(target_id);
	}
	let exists = match id {
		Some(id) => row_exists(tx, "category", id).await?,
		None => false,
	};
	let current = match id {
		Some(id) => current_event(tx, user_id, "category", &id.to_string(), false).await?,
		None => None,
	};
	if is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped_id.is_some(), id.map(|id| id.to_string()).as_deref())
			.await?;
		return Ok(Disposition::Stale);
	}
	if change.deleted {
		let Some(id) = id else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		crate::settings::remove_category_exclusions(tx, id).await?;
		crate::store::category::delete(&mut **tx, id).await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&id.to_string(),
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot,
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	let mut stubs = HashSet::new();
	let id = ensure_category(tx, change, user_id, id, &mut stubs).await?;
	crate::store::category::set_fields(
		&mut **tx,
		id,
		&string(&snapshot, "name")?,
		integer(&snapshot, "sortOrder")?,
		flag(&snapshot, "isDefault")?,
	)
	.await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&id.to_string(),
		stubs,
		Finish {
			modified_at: change.modified_at,
			identity: change.identity.clone(),
			snapshot,
			preserve_origin: true,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}


fn key_ids(key: &str) -> ApiResult<Vec<i64>> {
	key.split(':')
		.map(|part| {
			part.parse::<i64>()
				.map_err(|_| ApiError::bad_request("invalid relation entity key"))
		})
		.collect()
}

async fn relation_exists(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	kind: &str,
	key: &str,
) -> ApiResult<bool> {
	let ids = key_ids(key)?;
	let exists = match kind {
		"manga_user" if ids.len() == 2 => {
			sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM manga_user WHERE manga_id = ? AND user_id = ?)")
				.bind(ids[0])
				.bind(ids[1])
				.fetch_one(&mut **tx)
				.await?
		}
		"chapter_user" if ids.len() == 2 => {
			sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chapter_user WHERE chapter_id = ? AND user_id = ?)")
				.bind(ids[0])
				.bind(ids[1])
				.fetch_one(&mut **tx)
				.await?
		}
		"category_manga" if ids.len() == 3 => {
			sqlx::query_scalar(
				"SELECT EXISTS(SELECT 1 FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?)",
			)
			.bind(ids[0])
			.bind(ids[1])
			.bind(ids[2])
			.fetch_one(&mut **tx)
			.await?
		}
		_ => return Err(ApiError::bad_request("invalid relation entity key")),
	};
	Ok(exists)
}

async fn apply_manga_user(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mapped = mapped_key(tx, change, user_id, "manga_user", &change.entity_key, false).await?;
	let mapped_manga_id = mapped.as_deref().map(key_ids).transpose()?.map(|ids| ids[0]);
	let natural_parent = parent_manga_id(tx, change, user_id).await?;
	let parent_id = mapped_manga_id.or(natural_parent);
	let local_key = mapped.clone().or_else(|| parent_id.map(|id| format!("{id}:{user_id}")));
	let exists = match local_key.as_deref() {
		Some(key) => relation_exists(tx, "manga_user", key).await?,
		None => false,
	};
	let current = match local_key.as_deref() {
		Some(key) => current_event(tx, user_id, "manga_user", key, false).await?,
		None => None,
	};
	if is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped.is_some(), local_key.as_deref()).await?;
		return Ok(Disposition::Stale);
	}
	let source_available = source_id(tx, &identity_string(&change.identity, "sourceKey")?)
		.await?
		.is_some();
	if change.deleted && mapped.is_none() && !source_available {
		return Ok(Disposition::Skipped);
	}
	if change.deleted {
		let Some(key) = local_key else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		let ids = key_ids(&key)?;
		sqlx::query("DELETE FROM manga_user WHERE manga_id = ? AND user_id = ?")
			.bind(ids[0])
			.bind(ids[1])
			.execute(&mut **tx)
			.await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&key,
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot: change.snapshot.clone(),
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	let mut stubs = HashSet::new();
	let Some(manga_id) = ensure_manga(tx, change, parent_id, &mut stubs).await? else {
		return Ok(Disposition::Skipped);
	};
	let local_key = format!("{manga_id}:{user_id}");
	sqlx::query(
		"INSERT INTO manga_user
		 (manga_id, user_id, in_library, in_library_at, viewer_flags, sort_key, incognito)
		 VALUES (?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET
		 in_library = excluded.in_library, in_library_at = excluded.in_library_at,
		 viewer_flags = excluded.viewer_flags, sort_key = excluded.sort_key, incognito = excluded.incognito",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(flag(&change.snapshot, "inLibrary")?)
	.bind(optional_integer(&change.snapshot, "inLibraryAt")?)
	.bind(integer(&change.snapshot, "viewerFlags")?)
	.bind(string(&change.snapshot, "sortKey")?)
	.bind(flag(&change.snapshot, "incognito")?)
	.execute(&mut **tx)
	.await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&local_key,
		stubs,
		Finish {
			modified_at: change.modified_at,
			identity: change.identity.clone(),
			snapshot: change.snapshot.clone(),
			preserve_origin: true,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}

async fn apply_category_manga(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mapped = mapped_key(tx, change, user_id, "category_manga", &change.entity_key, false).await?;
	let mapped_ids = mapped.as_deref().map(key_ids).transpose()?;
	let mapped_category_id = mapped_ids.as_ref().map(|ids| ids[0]);
	let mapped_manga_id = mapped_ids.as_ref().map(|ids| ids[1]);
	let natural_category = parent_category_id(tx, change, user_id).await?;
	let natural_manga = parent_manga_id(tx, change, user_id).await?;
	let category_id = mapped_category_id.or(natural_category);
	let manga_id = mapped_manga_id.or(natural_manga);
	let local_key = mapped.clone().or_else(|| {
		category_id
			.zip(manga_id)
			.map(|(category_id, manga_id)| format!("{category_id}:{manga_id}:{user_id}"))
	});
	let exists = match local_key.as_deref() {
		Some(key) => relation_exists(tx, "category_manga", key).await?,
		None => false,
	};
	let current = match local_key.as_deref() {
		Some(key) => current_event(tx, user_id, "category_manga", key, false).await?,
		None => None,
	};
	if is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped.is_some(), local_key.as_deref()).await?;
		return Ok(Disposition::Stale);
	}
	let source_available = source_id(tx, &identity_string(&change.identity, "sourceKey")?)
		.await?
		.is_some();
	if change.deleted && mapped.is_none() && !source_available {
		return Ok(Disposition::Skipped);
	}
	if change.deleted {
		let Some(key) = local_key else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		let ids = key_ids(&key)?;
		crate::store::category::remove_manga(&mut **tx, ids[0], ids[1], ids[2]).await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&key,
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot: change.snapshot.clone(),
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	let mut stubs = HashSet::new();
	let category_id = ensure_category(tx, change, user_id, category_id, &mut stubs).await?;
	let Some(manga_id) = ensure_manga(tx, change, manga_id, &mut stubs).await? else {
		return Ok(Disposition::Skipped);
	};
	let local_key = format!("{category_id}:{manga_id}:{user_id}");
	crate::store::category::add_manga(&mut **tx, category_id, manga_id, user_id).await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&local_key,
		stubs,
		Finish {
			modified_at: change.modified_at,
			identity: change.identity.clone(),
			snapshot: change.snapshot.clone(),
			preserve_origin: true,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}

#[derive(Debug, Clone, PartialEq)]
struct ProgressValues {
	last_page_read: i64,
	is_read: bool,
	is_bookmarked: bool,
	last_read_at: Option<i64>,
}

fn incoming_progress(change: &WireChange) -> ApiResult<ProgressValues> {
	Ok(ProgressValues {
		last_page_read: integer(&change.snapshot, "lastPageRead")?,
		is_read: flag(&change.snapshot, "isRead")?,
		is_bookmarked: flag(&change.snapshot, "isBookmarked")?,
		last_read_at: optional_integer(&change.snapshot, "lastReadAt")?,
	})
}

async fn progress_identity(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	chapter_id: i64,
	user_id: i64,
) -> ApiResult<Value> {
	let identity: String = sqlx::query_scalar("SELECT identity FROM sync_chapter_state WHERE chapter_id = ?")
		.bind(chapter_id)
		.fetch_one(&mut **tx)
		.await?;
	let mut identity: Value = serde_json::from_str(&identity)?;
	identity["userId"] = json!(user_id);
	Ok(identity)
}

async fn apply_progress(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	start_sequence: i64,
	user_id: i64,
	change: &WireChange,
) -> ApiResult<Disposition> {
	let mapped = mapped_key(tx, change, user_id, "chapter_user", &change.entity_key, false).await?;
	let mapped_chapter_id = mapped.as_deref().map(key_ids).transpose()?.map(|ids| ids[0]);
	let natural_chapter = parent_chapter_id(tx, change, user_id).await?;
	let chapter_id = mapped_chapter_id.or(natural_chapter);
	let local_key = mapped.clone().or_else(|| chapter_id.map(|id| format!("{id}:{user_id}")));
	let exists = match local_key.as_deref() {
		Some(key) => relation_exists(tx, "chapter_user", key).await?,
		None => false,
	};
	let current = match local_key.as_deref() {
		Some(key) => current_event(tx, user_id, "chapter_user", key, false).await?,
		None => None,
	};
	let (local, local_version) = if exists {
		let row: (i64, bool, bool, Option<i64>, i64) = sqlx::query_as(
			"SELECT last_page_read, is_read, is_bookmarked, last_read_at, last_modified_at FROM chapter_user
			 WHERE chapter_id = ? AND user_id = ?",
		)
		.bind(chapter_id)
		.bind(user_id)
		.fetch_one(&mut **tx)
		.await?;
		(
			ProgressValues {
				last_page_read: row.0,
				is_read: row.1,
				is_bookmarked: row.2,
				last_read_at: row.3,
			},
			row.4,
		)
	} else {
		(
			ProgressValues {
				last_page_read: 0,
				is_read: false,
				is_bookmarked: false,
				last_read_at: None,
			},
			0,
		)
	};
	let mut current_version = current
		.as_ref()
		.map(|current| current.version.clone())
		.unwrap_or_else(|| Version {
			modified_at: local_version,
			origin: String::new(),
			origin_sequence: 0,
		});
	current_version.modified_at = current_version.modified_at.max(local_version);
	if change.deleted {
		if incoming_version(change).cmp(&current_version) != Ordering::Greater {
			record_mapping(tx, user_id, change, mapped.is_some(), local_key.as_deref()).await?;
			return Ok(Disposition::Stale);
		}
		let source_available = source_id(tx, &identity_string(&change.identity, "sourceKey")?)
			.await?
			.is_some();
		if mapped.is_none() && !source_available {
			return Ok(Disposition::Skipped);
		}
		let Some(key) = local_key else {
			return Ok(Disposition::Applied);
		};
		if !exists {
			return Ok(Disposition::Applied);
		}
		let ids = key_ids(&key)?;
		sqlx::query("DELETE FROM chapter_user WHERE chapter_id = ? AND user_id = ?")
			.bind(ids[0])
			.bind(ids[1])
			.execute(&mut **tx)
			.await?;
		finish(
			tx,
			start_sequence,
			user_id,
			change,
			&key,
			HashSet::new(),
			Finish {
				modified_at: change.modified_at,
				identity: change.identity.clone(),
				snapshot: change.snapshot.clone(),
				preserve_origin: true,
			},
		)
		.await?;
		return Ok(Disposition::Applied);
	}
	if !exists && current.as_ref().is_some_and(|current| current.deleted) && is_stale(change, &current) {
		record_mapping(tx, user_id, change, mapped.is_some(), local_key.as_deref()).await?;
		return Ok(Disposition::Stale);
	}
	let mut stubs = HashSet::new();
	let manga_id = parent_manga_id(tx, change, user_id).await?;
	let Some(chapter_id) = ensure_chapter(tx, change, manga_id, chapter_id, &mut stubs).await? else {
		return Ok(Disposition::Skipped);
	};
	let local_key = format!("{chapter_id}:{user_id}");
	let incoming = incoming_progress(change)?;
	let incoming_wins = incoming_version(change).cmp(&current_version) == Ordering::Greater;
	let merged = ProgressValues {
		last_page_read: local.last_page_read.max(incoming.last_page_read),
		is_read: local.is_read || incoming.is_read,
		is_bookmarked: if incoming_wins {
			incoming.is_bookmarked
		} else {
			local.is_bookmarked
		},
		last_read_at: match (local.last_read_at, incoming.last_read_at) {
			(Some(local), Some(incoming)) => Some(local.max(incoming)),
			(Some(local), None) => Some(local),
			(None, Some(incoming)) => Some(incoming),
			(None, None) => None,
		},
	};
	if exists && merged == local {
		return Ok(Disposition::Applied);
	}
	let modified_at = current_version.modified_at.max(change.modified_at);
	let preserve_origin = merged == incoming && modified_at == change.modified_at;
	let identity = if preserve_origin {
		change.identity.clone()
	} else {
		progress_identity(tx, chapter_id, user_id).await?
	};
	let snapshot = if preserve_origin {
		change.snapshot.clone()
	} else {
		json!({
			"chapterId": chapter_id,
			"userId": user_id,
			"lastPageRead": merged.last_page_read,
			"isRead": merged.is_read,
			"isBookmarked": merged.is_bookmarked,
			"lastReadAt": merged.last_read_at,
			"lastModifiedAt": modified_at,
		})
	};
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, ?, ?, ?, ?, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
		 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(merged.last_page_read)
	.bind(merged.is_read)
	.bind(merged.is_bookmarked)
	.bind(merged.last_read_at)
	.execute(&mut **tx)
	.await?;
	finish(
		tx,
		start_sequence,
		user_id,
		change,
		&local_key,
		stubs,
		Finish {
			modified_at,
			identity,
			snapshot,
			preserve_origin,
		},
	)
	.await?;
	Ok(Disposition::Applied)
}
