//! The `setting` table: configuration stored as one JSON value per key. All of its SQL lives here.

use std::collections::HashMap;
use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{Executor, Sqlite, SqlitePool};

use crate::error::ApiResult;

/// A setting read and written as `T`. A stored value of the wrong shape reads as unset, because the
/// flat settings API accepts arbitrary JSON.
pub(crate) struct Key<T> {
	name: &'static str,
	value: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned + Serialize> Key<T> {
	pub(crate) const fn new(name: &'static str) -> Self {
		Self { name, value: PhantomData }
	}

	pub(crate) async fn get<'e, E: Executor<'e, Database = Sqlite>>(&self, db: E) -> ApiResult<Option<T>> {
		get_typed(db, self.name).await
	}

	pub(crate) async fn set<'e, E: Executor<'e, Database = Sqlite>>(&self, db: E, value: &T) -> ApiResult<()> {
		write(db, self.name, &serde_json::to_string(value)?).await
	}
}

/// A setting stored under the owner's `user.<id>.` namespace
pub(crate) struct UserKey<T> {
	name: &'static str,
	value: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned + Serialize> UserKey<T> {
	pub(crate) const fn new(name: &'static str) -> Self {
		Self { name, value: PhantomData }
	}

	pub(crate) async fn get<'e, E: Executor<'e, Database = Sqlite>>(&self, db: E, user_id: i64) -> ApiResult<Option<T>> {
		get_typed(db, &user_key(user_id, self.name)).await
	}

	pub(crate) async fn set<'e, E: Executor<'e, Database = Sqlite>>(
		&self,
		db: E,
		user_id: i64,
		value: &T,
	) -> ApiResult<()> {
		write(db, &user_key(user_id, self.name), &serde_json::to_string(value)?).await
	}
}

pub(crate) const AUTO_DOWNLOAD_NEW_CHAPTERS: Key<bool> = Key::new(crate::download::AUTO_DOWNLOAD_NEW_CHAPTERS);
pub(crate) const BACKUP_DIR: Key<String> = Key::new("backup.dir");
pub(crate) const BACKUP_ENABLED: Key<bool> = Key::new("backup.enabled");
pub(crate) const BACKUP_INTERVAL_HOURS: Key<u64> = Key::new("backup.interval_hours");
pub(crate) const BACKUP_RETENTION_COUNT: Key<u64> = Key::new("backup.retention_count");
pub(crate) const CHAPTER_CACHE_MAX_BYTES: Key<i64> = Key::new("cache.chapterMaxBytes");
pub(crate) const DOWNLOAD_MOVE_PATH: Key<String> = Key::new("download.move_path");
pub(crate) const LOCAL_DIRS: Key<Vec<String>> = Key::new("local.dirs");
pub(crate) const LOCAL_ONESHOTS_DIR: Key<String> = Key::new("local.oneshots_dir");
/// Ordered array of source keys; the order is also the default priority for later cross-source migration,
/// aligned with Aidoku's Browse.pinnedList; reinstalling/uninstalling a source changes its internal id, so the
/// key is the stable identity
pub(crate) const PINNED_SOURCES: Key<Vec<String>> = Key::new("browse.pinned_sources");

pub(crate) const EXCLUDED_DOWNLOAD_CATEGORIES: UserKey<Vec<i64>> =
	UserKey::new(crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES);
pub(crate) const EXCLUDED_UPDATE_CATEGORIES: UserKey<Vec<i64>> =
	UserKey::new(crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES);
pub(crate) const INCOGNITO_ENABLED: UserKey<bool> = UserKey::new("incognito.enabled");

pub(crate) fn user_key(user_id: i64, key: &str) -> String {
	format!("user.{user_id}.{key}")
}

async fn get_typed<'e, E, T>(db: E, key: &str) -> ApiResult<Option<T>>
where
	E: Executor<'e, Database = Sqlite>,
	T: DeserializeOwned,
{
	let raw: Option<String> = sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
		.bind(key)
		.fetch_optional(db)
		.await?;
	Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}

async fn write<'e, E: Executor<'e, Database = Sqlite>>(db: E, key: &str, raw: &str) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(key)
	.bind(raw)
	.execute(db)
	.await?;
	Ok(())
}

/// Reads a group of settings at once, leaving out unset keys and values that are not JSON
pub(crate) async fn values(pool: &SqlitePool, keys: &[&str]) -> ApiResult<HashMap<String, Value>> {
	let placeholders = vec!["?"; keys.len()].join(", ");
	let sql = format!("SELECT key, value FROM setting WHERE key IN ({placeholders})");
	let mut query = sqlx::query_as::<_, (String, String)>(&sql);
	for key in keys {
		query = query.bind(*key);
	}
	Ok(query
		.fetch_all(pool)
		.await?
		.into_iter()
		.filter_map(|(key, raw)| serde_json::from_str(&raw).ok().map(|value| (key, value)))
		.collect())
}

/// Every stored row as raw JSON text, for the settings API and backups
pub(crate) async fn all(pool: &SqlitePool) -> ApiResult<Vec<(String, String)>> {
	Ok(sqlx::query_as("SELECT key, value FROM setting ORDER BY key")
		.fetch_all(pool)
		.await?)
}

/// Writes a key known only at runtime, for the settings API and backup restore
pub(crate) async fn write_value<'e, E: Executor<'e, Database = Sqlite>>(db: E, key: &str, value: &Value) -> ApiResult<()> {
	write(db, key, &serde_json::to_string(value)?).await
}

pub(crate) async fn remove<'e, E: Executor<'e, Database = Sqlite>>(db: E, key: &str) -> ApiResult<()> {
	sqlx::query("DELETE FROM setting WHERE key = ?")
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

/// Drops a deleted category from both of the owner's exclusion lists
pub(crate) async fn remove_category_exclusions(
	tx: &mut sqlx::Transaction<'_, Sqlite>,
	user_id: i64,
	category_id: i64,
) -> ApiResult<()> {
	let update_key = user_key(user_id, crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES);
	let download_key = user_key(user_id, crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES);
	sqlx::query(
		"UPDATE setting
		 SET value = (
		  SELECT json_group_array(item.value) FROM json_each(setting.value) AS item
		  WHERE item.value <> ?
		 )
		 WHERE key IN (?, ?)
		  AND EXISTS (
		   SELECT 1 FROM json_each(setting.value) AS item WHERE item.value = ?
		  )",
	)
	.bind(category_id)
	.bind(update_key)
	.bind(download_key)
	.bind(category_id)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use std::fs;
	use std::path::Path;

	fn collect_sources(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
		for entry in fs::read_dir(dir).expect("read src dir") {
			let path = entry.expect("dir entry").path();
			if path.is_dir() {
				collect_sources(&path, files);
			} else if path.extension().is_some_and(|ext| ext == "rs") {
				files.push(path);
			}
		}
	}

	#[tokio::test]
	async fn a_wrong_shape_reads_as_unset() {
		let tmp = tempfile::tempdir().expect("tmp");
		let pool = crate::db::create_pool(&format!("sqlite://{}/settings.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		super::write_value(&pool, "backup.interval_hours", &serde_json::json!("soon"))
			.await
			.expect("write");
		assert_eq!(super::BACKUP_INTERVAL_HOURS.get(&pool).await.expect("read"), None);
		super::BACKUP_INTERVAL_HOURS.set(&pool, &6).await.expect("write");
		assert_eq!(super::BACKUP_INTERVAL_HOURS.get(&pool).await.expect("read"), Some(6));
		let values = super::values(&pool, &["backup.interval_hours", "backup.enabled"]).await.expect("values");
		assert_eq!(values.len(), 1);
		super::remove(&pool, "backup.interval_hours").await.expect("remove");
		assert_eq!(super::BACKUP_INTERVAL_HOURS.get(&pool).await.expect("read"), None);
	}

	#[test]
	fn only_the_owning_modules_touch_their_tables() {
		let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
		let mut files = Vec::new();
		collect_sources(&src, &mut files);
		let mut offenders = Vec::new();
		for file in files {
			let relative = file.strip_prefix(&src).expect("under src").to_string_lossy().replace('\\', "/");
			let text = fs::read_to_string(&file).expect("read source");
			let production = text.split("#[cfg(test)]\nmod tests").next().expect("split yields one part");
			for (owner, table) in [("settings.rs", "setting"), ("server_state.rs", "server_state")] {
				if relative == owner {
					continue;
				}
				for verb in ["FROM", "INTO", "UPDATE"] {
					if production.contains(&format!("{verb} {table} ")) || production.contains(&format!("{verb} {table}\n")) {
						offenders.push(format!("{relative}: {verb} {table}"));
					}
				}
			}
		}
		assert!(offenders.is_empty(), "{offenders:#?}");
	}
}
