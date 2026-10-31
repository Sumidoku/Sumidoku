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

/// One setting the server knows about. New settings are added here, and validation, the structured
/// view and backups all follow from this entry.
pub(crate) struct Definition {
	pub(crate) key: &'static str,
	user_scoped: bool,
	in_backup: bool,
	clears_on_null: bool,
	check: fn(&str, &Value) -> Result<(), String>,
}

impl Definition {
	const fn new(key: &'static str, check: fn(&str, &Value) -> Result<(), String>) -> Self {
		Self { key, user_scoped: false, in_backup: true, clears_on_null: false, check }
	}

	const fn user_scoped(mut self) -> Self {
		self.user_scoped = true;
		self
	}

	/// Holds instance-local ids, which backups carry by name instead
	const fn not_in_backup(mut self) -> Self {
		self.in_backup = false;
		self
	}

	const fn clearable(mut self) -> Self {
		self.clears_on_null = true;
		self
	}

	pub(crate) fn clears_on_null(&self) -> bool {
		self.clears_on_null
	}

	pub(crate) fn validate(&self, value: &Value) -> Result<(), String> {
		(self.check)(self.key, value)
	}
}

fn any(_: &str, _: &Value) -> Result<(), String> {
	Ok(())
}

fn boolean(key: &str, value: &Value) -> Result<(), String> {
	if value.is_boolean() { Ok(()) } else { Err(format!("{key} must be a boolean")) }
}

fn non_negative(key: &str, value: &Value) -> Result<(), String> {
	if value.as_u64().is_some() { Ok(()) } else { Err(format!("{key} must be a non-negative integer")) }
}

fn positive(key: &str, value: &Value) -> Result<(), String> {
	if matches!(value.as_u64(), Some(1..)) { Ok(()) } else { Err(format!("{key} must be a positive integer")) }
}

fn string(key: &str, value: &Value) -> Result<(), String> {
	if value.is_string() { Ok(()) } else { Err(format!("{key} must be a string")) }
}

fn category_ids(key: &str, value: &Value) -> Result<(), String> {
	if crate::update::valid_excluded_categories(value) {
		Ok(())
	} else {
		Err(format!("{key} must be an array of category ids"))
	}
}

fn skip_titles(key: &str, value: &Value) -> Result<(), String> {
	if crate::update::valid_skip_titles(value) {
		Ok(())
	} else {
		Err(format!("{key} must be an array containing only hasUnread, completed or notStarted"))
	}
}

fn download_format(key: &str, value: &Value) -> Result<(), String> {
	if matches!(value.as_str(), Some("cbz" | "folder")) { Ok(()) } else { Err(format!("{key} must be cbz or folder")) }
}

fn chapter_cache_bytes(key: &str, value: &Value) -> Result<(), String> {
	if value.as_u64().is_some_and(|bytes| bytes <= i64::MAX as u64) {
		Ok(())
	} else {
		Err(format!("{key} must be between 0 and {}", i64::MAX))
	}
}

/// An empty array clears the override
fn directories(key: &str, value: &Value) -> Result<(), String> {
	if value.as_array().is_some_and(|dirs| dirs.iter().all(|dir| dir.as_str().is_some_and(|dir| !dir.trim().is_empty()))) {
		Ok(())
	} else {
		Err(format!("{key} must be an array of directory paths"))
	}
}

/// The library page's initial chip; a since-deleted id is pruned client-side, so any positive id stores
fn default_category(key: &str, value: &Value) -> Result<(), String> {
	if value.as_str().is_some_and(|v| v == "all" || v == "uncategorized" || v.parse::<i64>().is_ok_and(|id| id > 0)) {
		Ok(())
	} else {
		Err(format!("{key} must be \"all\", \"uncategorized\" or a category id"))
	}
}

const DEFINITIONS: &[Definition] = &[
	Definition::new("cache.chapterMaxBytes", chapter_cache_bytes),
	Definition::new("incognito.enabled", any).user_scoped(),
	Definition::new("library.default_category", default_category).user_scoped(),
	Definition::new("library.show_uncategorized", boolean).user_scoped(),
	Definition::new(crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES, category_ids).user_scoped().not_in_backup(),
	Definition::new(crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES, category_ids).user_scoped().not_in_backup(),
	Definition::new(crate::update::LIBRARY_SKIP_TITLES, skip_titles),
	Definition::new(crate::update::LIBRARY_REFRESH_METADATA, boolean),
	Definition::new(crate::update::LIBRARY_UPDATE_INTERVAL_HOURS, non_negative),
	Definition::new(crate::download::AUTO_DOWNLOAD_NEW_CHAPTERS, boolean),
	Definition::new(crate::download::AUTO_DOWNLOAD_LIMIT, non_negative),
	Definition::new(crate::download::DOWNLOAD_FORMAT, download_format),
	Definition::new(crate::download::DOWNLOAD_CONCURRENCY, positive),
	Definition::new(crate::download::DELETE_AFTER_MANUAL_READ, boolean),
	Definition::new(crate::download::DELETE_AFTER_READ, boolean),
	Definition::new(crate::download::ALLOW_DELETE_BOOKMARKED, boolean),
	Definition::new(crate::download::AUTO_SKIP_MANGA_WITH_UNREAD, boolean),
	Definition::new(crate::download::AUTO_IGNORE_REUPLOADS, boolean),
	Definition::new(crate::download::DOWNLOAD_AHEAD, boolean),
	Definition::new(crate::download::DOWNLOAD_AHEAD_COUNT, non_negative),
	Definition::new("download.move_path", any),
	Definition::new("local.dirs", directories),
	Definition::new("local.oneshots_dir", string).clearable(),
	Definition::new("backup.enabled", any),
	Definition::new("backup.interval_hours", any),
	Definition::new("backup.retention_count", any),
	Definition::new("backup.dir", any).clearable(),
	Definition::new("browse.pinned_sources", any),
];

/// Names older clients and backups can still send: the removed chapter LRU switch, which must not
/// disable collection, and read-only values that were once injected into the flat view.
const RETIRED_KEYS: [&str; 3] = ["cache.chapterLruEnabled", "localDirs", "localOneshotsDir"];

pub(crate) fn definition(key: &str) -> Option<&'static Definition> {
	DEFINITIONS.iter().find(|definition| definition.key == key)
}

pub(crate) fn is_user_scoped(key: &str) -> bool {
	definition(key).is_some_and(|definition| definition.user_scoped)
}

/// Namespace keys come from the request identity, so a literal `user.*` is never accepted either
pub(crate) fn ignored_on_write(key: &str) -> bool {
	key.starts_with("user.") || RETIRED_KEYS.contains(&key) || crate::server_state::KEYS.contains(&key)
}

/// Anything else is an unknown key, which the settings API stores and returns as it came
pub(crate) fn is_modeled(key: &str) -> bool {
	definition(key).is_some() || ignored_on_write(key)
}

/// Older backups can still name these, so restore needs the same answer as export
pub(crate) fn omitted_from_backup(key: &str) -> bool {
	crate::server_state::KEYS.contains(&key) || definition(key).is_some_and(|definition| !definition.in_backup)
}

/// The row a request's bare key is stored under
pub(crate) fn stored_key(user_id: i64, key: &str) -> String {
	if is_user_scoped(key) { user_key(user_id, key) } else { key.to_string() }
}

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

	#[test]
	fn every_key_is_defined_once() {
		let mut keys: Vec<&str> = super::DEFINITIONS.iter().map(|definition| definition.key).collect();
		let total = keys.len();
		keys.sort_unstable();
		keys.dedup();
		assert_eq!(keys.len(), total);
	}

	#[test]
	fn every_structured_setting_is_defined() {
		let settings = crate::api::server_settings::ServerSettings::from_flat(&serde_json::Map::new()).expect("defaults");
		for key in settings.into_flat().keys() {
			assert!(super::is_modeled(key), "{key}");
		}
	}

	#[test]
	fn validation_keeps_the_public_messages() {
		for (key, value, message) in [
			("library.refresh_metadata", serde_json::json!(1), "library.refresh_metadata must be a boolean"),
			("download_concurrency", serde_json::json!(0), "download_concurrency must be a positive integer"),
			("download_format", serde_json::json!("zip"), "download_format must be cbz or folder"),
			("local.oneshots_dir", serde_json::json!(1), "local.oneshots_dir must be a string"),
			(
				"cache.chapterMaxBytes",
				serde_json::json!(-1),
				"cache.chapterMaxBytes must be between 0 and 9223372036854775807",
			),
			(
				"library.default_category",
				serde_json::json!("0"),
				"library.default_category must be \"all\", \"uncategorized\" or a category id",
			),
		] {
			assert_eq!(super::definition(key).expect("defined").validate(&value), Err(message.to_string()));
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
