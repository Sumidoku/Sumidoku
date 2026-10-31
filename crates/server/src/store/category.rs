//! Writes to `category` and `category_manga`.

use sqlx::sqlite::SqliteQueryResult;
use sqlx::{Executor, Sqlite, Transaction};

use crate::error::ApiResult;

pub(crate) async fn create<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	user_id: i64,
	name: &str,
	sort_order: i64,
	is_default: bool,
) -> ApiResult<i64> {
	Ok(sqlx::query_scalar(
		"INSERT INTO category (user_id, name, sort_order, is_default)
		 VALUES (?, ?, ?, ?) RETURNING id",
	)
	.bind(user_id)
	.bind(name)
	.bind(sort_order)
	.bind(is_default)
	.fetch_one(db)
	.await?)
}

pub(crate) async fn rename<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, user_id: i64, name: &str) -> ApiResult<()> {
	sqlx::query("UPDATE category SET name = ? WHERE id = ? AND user_id = ?")
		.bind(name)
		.bind(id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_sort_order<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	user_id: i64,
	sort_order: i64,
) -> ApiResult<()> {
	sqlx::query("UPDATE category SET sort_order = ? WHERE id = ? AND user_id = ?")
		.bind(sort_order)
		.bind(id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_placement<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	sort_order: i64,
	is_default: bool,
) -> ApiResult<()> {
	sqlx::query("UPDATE category SET sort_order = ?, is_default = ? WHERE id = ?")
		.bind(sort_order)
		.bind(is_default)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_fields<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	name: &str,
	sort_order: i64,
	is_default: bool,
) -> ApiResult<()> {
	sqlx::query("UPDATE category SET name = ?, sort_order = ?, is_default = ? WHERE id = ?")
		.bind(name)
		.bind(sort_order)
		.bind(is_default)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn delete_owned<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM category WHERE id = ? AND user_id = ?")
		.bind(id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn delete<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM category WHERE id = ?").bind(id).execute(db).await?;
	Ok(())
}

pub(crate) async fn set_modified_at<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	modified_at: i64,
) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query("UPDATE category SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(db)
		.await?)
}

/// A nameless placeholder for a sync event whose category has not arrived yet
pub(crate) async fn insert_stub<'e, E: Executor<'e, Database = Sqlite>>(db: E, user_id: i64) -> ApiResult<i64> {
	Ok(
		sqlx::query_scalar("INSERT INTO category (user_id, name, last_modified_at) VALUES (?, '', 0) RETURNING id")
			.bind(user_id)
			.fetch_one(db)
			.await?,
	)
}

pub(crate) async fn insert_stub_with_id<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, user_id: i64) -> ApiResult<i64> {
	Ok(
		sqlx::query_scalar("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (?, ?, '', 0) RETURNING id")
			.bind(id)
			.bind(user_id)
			.fetch_one(db)
			.await?,
	)
}

pub(crate) async fn add_manga<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
) -> ApiResult<()> {
	sqlx::query("INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, ?)")
		.bind(category_id)
		.bind(manga_id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn remove_manga<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
) -> ApiResult<()> {
	sqlx::query("DELETE FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = ?")
		.bind(category_id)
		.bind(manga_id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn clear_manga<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?")
		.bind(manga_id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn clear_category<'e, E: Executor<'e, Database = Sqlite>>(db: E, category_id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM category_manga WHERE category_id = ?")
		.bind(category_id)
		.execute(db)
		.await?;
	Ok(())
}

/// Makes `category_ids` the manga's exact membership, leaving rows it keeps untouched
pub(crate) async fn set_manga_categories(
	tx: &mut Transaction<'_, Sqlite>,
	manga_id: i64,
	user_id: i64,
	category_ids: &[i64],
) -> ApiResult<()> {
	if category_ids.is_empty() {
		return clear_manga(&mut **tx, manga_id, user_id).await;
	}
	let placeholders = vec!["?"; category_ids.len()].join(",");
	let sql = format!("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ? AND category_id NOT IN ({placeholders})");
	let mut delete = sqlx::query(&sql).bind(manga_id).bind(user_id);
	for category_id in category_ids {
		delete = delete.bind(category_id);
	}
	delete.execute(&mut **tx).await?;
	for category_id in category_ids {
		add_manga(&mut **tx, *category_id, manga_id, user_id).await?;
	}
	Ok(())
}

pub(crate) async fn upsert_manga_at<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	category_id: i64,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, ?, ?)
		 ON CONFLICT (category_id, manga_id, user_id) DO UPDATE SET last_modified_at = excluded.last_modified_at",
	)
	.bind(category_id)
	.bind(manga_id)
	.bind(user_id)
	.bind(modified_at)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn set_manga_modified_at<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
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
	.execute(db)
	.await?)
}
