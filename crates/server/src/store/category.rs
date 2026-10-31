//! Writes to `category` and `category_manga`.

use sqlx::sqlite::SqliteQueryResult;
use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::sync::journal;

/// The row as peers see it. The JSON is built in SQL so the text matches events already published.
const CATEGORY_IDENTITY: &str = "json_object('categoryId', c.id, 'userId', c.user_id)";
const CATEGORY_SNAPSHOT: &str = "json_object(
	'id', c.id,
	'userId', c.user_id,
	'name', c.name,
	'sortOrder', c.sort_order,
	'isDefault', c.is_default,
	'includeInDownload', 0,
	'lastModifiedAt', c.last_modified_at
)";

async fn journal(conn: &mut SqliteConnection, id: i64, deleted: bool, modified_at: Option<i64>) -> ApiResult<()> {
	journal::append!(
		conn,
		format!(
			"SELECT c.user_id, 'category', CAST(c.id AS TEXT), ?, COALESCE(?, c.last_modified_at),
			 {CATEGORY_IDENTITY}, {CATEGORY_SNAPSHOT}
			 FROM category c JOIN user u ON u.id = c.user_id WHERE c.id = ?"
		),
		deleted,
		modified_at,
		id,
	)
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let Some(row) = published(conn, id).await? else { return Ok(()) };
	let key = id.to_string();
	if row.last_modified_at == 0 || row.last_modified_at == journal::latest_version(conn, Some(row.user_id), "category", &key).await? {
		let version = journal::next_version(conn, Some(row.user_id), "category", &key, row.last_modified_at + 1).await?;
		set_modified_at(&mut *conn, id, version).await?;
	}
	journal(conn, id, false, None).await
}

async fn publish_new(conn: &mut SqliteConnection, id: i64, user_id: i64) -> ApiResult<()> {
	let version = journal::next_version(conn, Some(user_id), "category", &id.to_string(), 0).await?;
	set_modified_at(&mut *conn, id, version).await?;
	journal(conn, id, false, None).await
}

#[derive(sqlx::FromRow, PartialEq)]
struct Published {
	user_id: i64,
	name: String,
	sort_order: i64,
	is_default: bool,
	last_modified_at: i64,
}

async fn published(conn: &mut SqliteConnection, id: i64) -> ApiResult<Option<Published>> {
	Ok(
		sqlx::query_as("SELECT user_id, name, sort_order, is_default, last_modified_at FROM category WHERE id = ?")
			.bind(id)
			.fetch_optional(conn)
			.await?,
	)
}

/// Publishes a new version if the write changed anything peers see
async fn publish_change(conn: &mut SqliteConnection, id: i64, before: Option<Published>) -> ApiResult<()> {
	let Some(before) = before else { return Ok(()) };
	let Some(after) = published(conn, id).await? else { return Ok(()) };
	if after == before {
		return Ok(());
	}
	let version = journal::next_version(conn, Some(after.user_id), "category", &id.to_string(), before.last_modified_at + 1).await?;
	set_modified_at(&mut *conn, id, version).await?;
	journal(conn, id, false, None).await
}

pub(crate) async fn create(conn: &mut SqliteConnection, user_id: i64, name: &str, sort_order: i64, is_default: bool) -> ApiResult<i64> {
	let id = sqlx::query_scalar(
		"INSERT INTO category (user_id, name, sort_order, is_default)
		 VALUES (?, ?, ?, ?) RETURNING id",
	)
	.bind(user_id)
	.bind(name)
	.bind(sort_order)
	.bind(is_default)
	.fetch_one(&mut *conn)
	.await?;
	publish_new(conn, id, user_id).await?;
	Ok(id)
}

pub(crate) async fn rename(conn: &mut SqliteConnection, id: i64, user_id: i64, name: &str) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE category SET name = ? WHERE id = ? AND user_id = ?")
		.bind(name)
		.bind(id)
		.bind(user_id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

pub(crate) async fn set_sort_order(conn: &mut SqliteConnection, id: i64, user_id: i64, sort_order: i64) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE category SET sort_order = ? WHERE id = ? AND user_id = ?")
		.bind(sort_order)
		.bind(id)
		.bind(user_id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

pub(crate) async fn set_placement(conn: &mut SqliteConnection, id: i64, sort_order: i64, is_default: bool) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE category SET sort_order = ?, is_default = ? WHERE id = ?")
		.bind(sort_order)
		.bind(is_default)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

pub(crate) async fn set_fields(conn: &mut SqliteConnection, id: i64, name: &str, sort_order: i64, is_default: bool) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE category SET name = ?, sort_order = ?, is_default = ? WHERE id = ?")
		.bind(name)
		.bind(sort_order)
		.bind(is_default)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

/// The tombstone goes first: it reads the row, and deleting the row cascades to its memberships
async fn publish_delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	if let Some(before) = published(conn, id).await? {
		journal(conn, id, true, Some(journal::delete_version(before.last_modified_at))).await?;
	}
	Ok(())
}

pub(crate) async fn delete_owned(conn: &mut SqliteConnection, id: i64, user_id: i64) -> ApiResult<()> {
	if published(conn, id).await?.is_some_and(|row| row.user_id == user_id) {
		publish_delete(conn, id).await?;
		release_members(conn, id).await?;
	}
	sqlx::query("DELETE FROM category WHERE id = ? AND user_id = ?")
		.bind(id)
		.bind(user_id)
		.execute(conn)
		.await?;
	Ok(())
}

pub async fn delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	publish_delete(conn, id).await?;
	release_members(conn, id).await?;
	sqlx::query("DELETE FROM category WHERE id = ?").bind(id).execute(conn).await?;
	Ok(())
}

pub(crate) async fn set_modified_at(conn: &mut SqliteConnection, id: i64, modified_at: i64) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query("UPDATE category SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(conn)
		.await?)
}

/// A nameless placeholder for a sync event whose category has not arrived yet
pub(crate) async fn insert_stub(conn: &mut SqliteConnection, user_id: i64) -> ApiResult<i64> {
	let id = sqlx::query_scalar("INSERT INTO category (user_id, name, last_modified_at) VALUES (?, '', 0) RETURNING id")
		.bind(user_id)
		.fetch_one(&mut *conn)
		.await?;
	publish_new(conn, id, user_id).await?;
	Ok(id)
}

pub(crate) async fn insert_stub_with_id(conn: &mut SqliteConnection, id: i64, user_id: i64) -> ApiResult<i64> {
	let id = sqlx::query_scalar("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (?, ?, '', 0) RETURNING id")
		.bind(id)
		.bind(user_id)
		.fetch_one(&mut *conn)
		.await?;
	publish_new(conn, id, user_id).await?;
	Ok(id)
}

async fn membership_version(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64) -> ApiResult<Option<i64>> {
	Ok(sqlx::query_scalar("SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?")
		.bind(category_id)
		.bind(manga_id)
		.bind(user_id)
		.fetch_optional(conn)
		.await?)
}

async fn publish_membership(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64) -> ApiResult<()> {
	journal::manga_for_user(conn, user_id, manga_id).await?;
	journal::membership(conn, category_id, manga_id, user_id, None).await
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish_member(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64) -> ApiResult<()> {
	let Some(version) = membership_version(conn, category_id, manga_id, user_id).await? else { return Ok(()) };
	let key = format!("{category_id}:{manga_id}:{user_id}");
	if version == 0 || version == journal::latest_version(conn, Some(user_id), "category_manga", &key).await? {
		let next = journal::next_version(conn, Some(user_id), "category_manga", &key, version + 1).await?;
		set_manga_modified_at(&mut *conn, category_id, manga_id, user_id, next).await?;
	}
	publish_membership(conn, category_id, manga_id, user_id).await
}

async fn tombstone_member(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64, version: i64) -> ApiResult<()> {
	journal::manga_for_user(conn, user_id, manga_id).await?;
	journal::membership(conn, category_id, manga_id, user_id, Some(journal::delete_version(version))).await
}

pub async fn add_manga(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64) -> ApiResult<()> {
	if membership_version(conn, category_id, manga_id, user_id).await?.is_some() {
		return Ok(());
	}
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, ?)")
		.bind(category_id)
		.bind(manga_id)
		.bind(user_id)
		.execute(&mut *conn)
		.await?;
	let key = format!("{category_id}:{manga_id}:{user_id}");
	let version = journal::next_version(conn, Some(user_id), "category_manga", &key, 0).await?;
	set_manga_modified_at(&mut *conn, category_id, manga_id, user_id, version).await?;
	publish_membership(conn, category_id, manga_id, user_id).await
}

pub async fn remove_manga(conn: &mut SqliteConnection, category_id: i64, manga_id: i64, user_id: i64) -> ApiResult<()> {
	if let Some(version) = membership_version(conn, category_id, manga_id, user_id).await? {
		tombstone_member(conn, category_id, manga_id, user_id, version).await?;
	}
	sqlx::query("DELETE FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?")
		.bind(category_id)
		.bind(manga_id)
		.bind(user_id)
		.execute(conn)
		.await?;
	Ok(())
}

pub async fn clear_manga(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<()> {
	set_manga_categories(conn, manga_id, user_id, &[]).await
}

/// Publishes and removes every member, for a category that is going away
async fn release_members(conn: &mut SqliteConnection, category_id: i64) -> ApiResult<()> {
	let members: Vec<(i64, i64, i64)> = sqlx::query_as(
		"SELECT manga_id, user_id, last_modified_at FROM category_manga WHERE category_id = ? ORDER BY manga_id, user_id",
	)
	.bind(category_id)
	.fetch_all(&mut *conn)
	.await?;
	for (manga_id, user_id, version) in members {
		tombstone_member(conn, category_id, manga_id, user_id, version).await?;
	}
	Ok(())
}

pub(crate) async fn clear_category(conn: &mut SqliteConnection, category_id: i64) -> ApiResult<()> {
	release_members(conn, category_id).await?;
	sqlx::query("DELETE FROM category_manga WHERE category_id = ?").bind(category_id).execute(conn).await?;
	Ok(())
}

/// Makes `category_ids` the manga's exact membership, leaving rows it keeps untouched
pub async fn set_manga_categories(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, category_ids: &[i64]) -> ApiResult<()> {
	let current: Vec<(i64, i64)> =
		sqlx::query_as("SELECT category_id, last_modified_at FROM category_manga WHERE manga_id = ? AND user_id = ? ORDER BY category_id")
			.bind(manga_id)
			.bind(user_id)
			.fetch_all(&mut *conn)
			.await?;
	for (category_id, version) in current {
		if category_ids.contains(&category_id) {
			continue;
		}
		tombstone_member(conn, category_id, manga_id, user_id, version).await?;
		sqlx::query("DELETE FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?")
			.bind(category_id)
			.bind(manga_id)
			.bind(user_id)
			.execute(&mut *conn)
			.await?;
	}
	for category_id in category_ids {
		add_manga(conn, *category_id, manga_id, user_id).await?;
	}
	Ok(())
}

pub(crate) async fn upsert_manga_at(
	conn: &mut SqliteConnection,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<()> {
	let existed = membership_version(conn, category_id, manga_id, user_id).await?.is_some();
	sqlx::query(
		"INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, ?, ?)
		 ON CONFLICT (category_id, manga_id, user_id) DO UPDATE SET last_modified_at = excluded.last_modified_at",
	)
	.bind(category_id)
	.bind(manga_id)
	.bind(user_id)
	.bind(modified_at)
	.execute(&mut *conn)
	.await?;
	if existed {
		return Ok(());
	}
	publish_membership(conn, category_id, manga_id, user_id).await
}

pub(crate) async fn set_manga_modified_at(
	conn: &mut SqliteConnection,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query(
		"UPDATE category_manga SET last_modified_at = ?
		 WHERE category_id = ? AND manga_id = ? AND user_id = ?",
	)
	.bind(modified_at)
	.bind(category_id)
	.bind(manga_id)
	.bind(user_id)
	.execute(conn)
	.await?)
}
