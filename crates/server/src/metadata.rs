//! Metadata provenance: which external writer owns each metadata column of a
//! library entry (#...).
//!
//! The `manga` row keeps the single copy of every metadata field. A per-field
//! lock in `manga.metadata_locks` records the writer that last applied it —
//! a tracker (`tracker:anilist`), another source (`source:5`) or the user
//! (`manual`). A locked column is kept through source refreshes; the lock is
//! the provenance record shown in the UI and allows a single-column restore.
//!
//! Everything here is pure except [`load`], [`store`] and [`write`].

use crate::model::{MangaRow, content_rating_from_int, status_from_int};
use aidoku::{ContentRating, MangaStatus};
use serde_json::{Value, json};
use sqlx::SqliteConnection;
use std::collections::BTreeMap;

/// Lock provider tag for values the user typed into the edit dialog.
pub const MANUAL: &str = "manual";

pub fn tracker_reference(provider: &str) -> String {
	format!("tracker:{provider}")
}

pub fn source_reference(source_id: i64) -> String {
	format!("source:{source_id}")
}

/// A lock held by a tracker (covers need the tracker-cover fetch path) versus
/// one held by a source or by the user.
pub fn is_tracker_reference(provider: &str) -> bool {
	provider.starts_with("tracker:")
}

/// One locked metadata column: who wrote it, which remote entry, and when.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldLock {
	pub provider: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub remote_id: Option<String>,
	pub applied_at: i64,
}

impl FieldLock {
	pub fn new(provider: impl Into<String>, remote_id: Option<&str>, applied_at: i64) -> Self {
		Self { provider: provider.into(), remote_id: remote_id.map(str::to_string), applied_at }
	}

	/// The remote entry id, for previews of an already-remembered source.
	pub fn remote_id(&self) -> Option<&str> {
		self.remote_id.as_deref()
	}
}

/// The whole lock map, keyed by [`Field::as_str`].
pub type Locks = BTreeMap<String, FieldLock>;

/// A metadata column that can be owned by a writer. Order is the dialog order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
	Title,
	Authors,
	Artists,
	Description,
	Tags,
	Status,
	ContentRating,
	Cover,
}

impl Field {
	pub const ALL: [Self; 8] =
		[Self::Title, Self::Authors, Self::Artists, Self::Description, Self::Tags, Self::Status, Self::ContentRating, Self::Cover];

	/// The wire and lock-map name (camelCase, matching `manga_json` keys).
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Title => "title",
			Self::Authors => "authors",
			Self::Artists => "artists",
			Self::Description => "description",
			Self::Tags => "tags",
			Self::Status => "status",
			Self::ContentRating => "contentRating",
			Self::Cover => "cover",
		}
	}

	/// The `manga` column that stores this field.
	pub fn column(self) -> &'static str {
		match self {
			Self::Title => "title",
			Self::Authors => "authors",
			Self::Artists => "artists",
			Self::Description => "description",
			Self::Tags => "tags",
			Self::Status => "status",
			Self::ContentRating => "content_rating",
			Self::Cover => "cover_url",
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			Self::Title => "Title",
			Self::Authors => "Authors",
			Self::Artists => "Artists",
			Self::Description => "Description",
			Self::Tags => "Tags",
			Self::Status => "Status",
			Self::ContentRating => "Content rating",
			Self::Cover => "Cover",
		}
	}

	/// Accepts both wire names and column names, so PATCH clients may use either.
	pub fn parse(name: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|field| field.as_str() == name || field.column() == name)
	}

	/// The `ON CONFLICT DO UPDATE SET` guard that keeps a locked column while letting a
	/// source refresh write unlocked ones.
	pub fn refresh_guard(self) -> String {
		format!(
			"CASE WHEN json_extract(manga.metadata_locks, '$.{}') IS NOT NULL THEN manga.{} ELSE excluded.{} END",
			self.as_str(),
			self.column(),
			self.column()
		)
	}
}

/// The current database value, shaped like the JSON the API returns.
pub fn current_value(row: &MangaRow, field: Field) -> Value {
	match field {
		Field::Title => json!(row.title),
		Field::Authors => json!(row.authors.0),
		Field::Artists => json!(row.artists.0),
		Field::Description => value_or_null(row.description.clone()),
		Field::Tags => json!(row.tags.0),
		Field::Status => json!(status_name(status_from_int(row.status))),
		Field::ContentRating => json!(content_rating_name(content_rating_from_int(row.content_rating))),
		Field::Cover => value_or_null(row.cover_url.clone()),
	}
}

fn value_or_null(value: Option<String>) -> Value {
	value.map(Value::String).unwrap_or(Value::Null)
}

pub fn status_name(status: MangaStatus) -> &'static str {
	match status {
		MangaStatus::Unknown => "Unknown",
		MangaStatus::Ongoing => "Ongoing",
		MangaStatus::Completed => "Completed",
		MangaStatus::Cancelled => "Cancelled",
		MangaStatus::Hiatus => "Hiatus",
	}
}

pub fn content_rating_name(rating: ContentRating) -> &'static str {
	match rating {
		ContentRating::Unknown => "Unknown",
		ContentRating::Safe => "Safe",
		ContentRating::Suggestive => "Suggestive",
		ContentRating::NSFW => "NSFW",
	}
}

pub fn status_int(name: &str) -> Option<i64> {
	match name {
		"Unknown" => Some(0),
		"Ongoing" => Some(1),
		"Completed" => Some(2),
		"Cancelled" => Some(3),
		"Hiatus" => Some(4),
		_ => None,
	}
}

pub fn content_rating_int(name: &str) -> Option<i64> {
	match name {
		"Unknown" => Some(0),
		"Safe" => Some(1),
		"Suggestive" => Some(2),
		"NSFW" => Some(3),
		_ => None,
	}
}

/// A value ready to be bound into `UPDATE manga`: metadata columns store lists
/// as JSON text and enumerations as their integer code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
	Text(Option<String>),
	List(Vec<String>),
	Integer(i64),
}

/// Validates one candidate value from a request (PATCH or apply). The error
/// message is returned to the client as a 400.
pub fn parse_value(field: Field, value: &Value) -> Result<FieldValue, String> {
	match field {
		Field::Title => value
			.as_str()
			.map(str::trim)
			.filter(|title| !title.is_empty())
			.map(|title| FieldValue::Text(Some(title.to_string())))
			.ok_or_else(|| "title must be a non-empty string".to_string()),
		Field::Description => match value {
			Value::Null => Ok(FieldValue::Text(None)),
			Value::String(text) => Ok(FieldValue::Text(Some(text.clone()))),
			_ => Err("description must be a string or null".into()),
		},
		Field::Cover => match value {
			Value::Null => Ok(FieldValue::Text(None)),
			Value::String(url) if url.starts_with("http://") || url.starts_with("https://") => Ok(FieldValue::Text(Some(url.clone()))),
			_ => Err("cover must be an http(s) URL or null".into()),
		},
		Field::Authors | Field::Artists | Field::Tags => {
			let items = value.as_array().ok_or_else(|| format!("{} must be an array of strings", field.as_str()))?;
			let mut names: Vec<String> = Vec::new();
			for item in items {
				let name = item.as_str().ok_or_else(|| format!("{} must be an array of strings", field.as_str()))?.trim();
				if !name.is_empty() {
					names.push(name.to_string());
				}
			}
			if field == Field::Title {
				return Err("title is not a list".into());
			}
			Ok(FieldValue::List(names))
		}
		Field::Status => value
			.as_str()
			.and_then(status_int)
			.map(FieldValue::Integer)
			.ok_or_else(|| "status must be one of Unknown, Ongoing, Completed, Cancelled, Hiatus".to_string()),
		Field::ContentRating => value
			.as_str()
			.and_then(content_rating_int)
			.map(FieldValue::Integer)
			.ok_or_else(|| "contentRating must be one of Unknown, Safe, Suggestive, NSFW".to_string()),
	}
}

// ---------------------------------------------------------------- lock storage

pub fn parse_locks(raw: &str) -> Locks {
	if raw.trim().is_empty() { Locks::new() } else { serde_json::from_str(raw).unwrap_or_default() }
}

pub fn serialize_locks(locks: &Locks) -> String {
	serde_json::to_string(locks).unwrap_or_else(|_| "{}".into())
}

/// Reads the lock map on a transaction/connection already bound to one user's
/// write path. Only [`crate::store::manga`] writes it back.
pub async fn load(conn: &mut SqliteConnection, manga_id: i64) -> sqlx::Result<Locks> {
	let raw: Option<String> = sqlx::query_scalar("SELECT metadata_locks FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_optional(&mut *conn)
		.await?
		.flatten();
	Ok(parse_locks(raw.as_deref().unwrap_or("{}")))
}

/// Records (or replaces) the owner of one field.
pub fn lock_field(locks: &mut Locks, field: Field, provider: &str, remote_id: Option<&str>, applied_at: i64) {
	locks.insert(field.as_str().to_string(), FieldLock::new(provider, remote_id, applied_at));
}

/// Drops a lock; returns whether one was held.
pub fn unlock_field(locks: &mut Locks, field: Field) -> bool {
	locks.remove(field.as_str()).is_some()
}

/// Lock-map view for `manga_json`: every locked field with its provenance.
pub fn locks_json(locks: &Locks) -> Value {
	let mut map = serde_json::Map::new();
	for field in Field::ALL {
		if let Some(lock) = locks.get(field.as_str()) {
			map.insert(field.as_str().to_string(), serde_json::to_value(lock).unwrap_or(Value::Null));
		}
	}
	Value::Object(map)
}

#[cfg(test)]
mod tests {
	use super::*;
	use sqlx::sqlite::SqlitePoolOptions;

	fn row() -> MangaRow {
		MangaRow {
			id: 7,
			source_id: 1,
			key: "key".into(),
			title: "Title".into(),
			authors: sqlx::types::Json(vec!["Author".into()]),
			artists: sqlx::types::Json(vec!["Artist".into()]),
			description: Some("Description".into()),
			tags: sqlx::types::Json(vec!["Action".into()]),
			status: 1,
			content_rating: 3,
			viewer: 0,
			cover_url: Some("https://img.example/cover.jpg".into()),
			url: None,
			initialized: true,
			update_strategy: 1,
			next_update_time: None,
			last_fetched_at: None,
			chapters_last_fetched_at: None,
			trashed_at: None,
			scan_mtime: None,
			created_at: None,
			updated_at: None,
		}
	}

	#[test]
	fn field_names_round_trip_and_columns_match_the_schema() {
		for field in Field::ALL {
			assert_eq!(Field::parse(field.as_str()), Some(field));
			assert_eq!(Field::parse(field.column()), Some(field));
		}
		assert_eq!(Field::ContentRating.column(), "content_rating");
		assert_eq!(Field::Cover.column(), "cover_url");
		assert_eq!(Field::parse("coverUrl"), None);
		assert_eq!(Field::Title.refresh_guard(), "CASE WHEN json_extract(manga.metadata_locks, '$.title') IS NOT NULL THEN manga.title ELSE excluded.title END");
	}

	#[test]
	fn current_values_use_the_json_wire_shape() {
		let row = row();
		assert_eq!(current_value(&row, Field::Title), json!("Title"));
		assert_eq!(current_value(&row, Field::Authors), json!(["Author"]));
		assert_eq!(current_value(&row, Field::Status), json!("Ongoing"));
		assert_eq!(current_value(&row, Field::ContentRating), json!("NSFW"));
		assert_eq!(current_value(&row, Field::Cover), json!("https://img.example/cover.jpg"));
	}

	#[test]
	fn values_are_validated_per_field() {
		assert_eq!(parse_value(Field::Title, &json!("  Title  ")), Ok(FieldValue::Text(Some("Title".into()))));
		assert!(parse_value(Field::Title, &json!("   ")).is_err());
		assert_eq!(parse_value(Field::Authors, &json!([" A ", "A", ""])), Ok(FieldValue::List(vec!["A".into()])));
		assert!(parse_value(Field::Authors, &json!("A")).is_err());
		assert_eq!(parse_value(Field::Status, &json!("Hiatus")), Ok(FieldValue::Integer(4)));
		assert!(parse_value(Field::Status, &json!("hiatus")).is_err());
		assert_eq!(parse_value(Field::Cover, &json!(null)), Ok(FieldValue::Text(None)));
		assert!(parse_value(Field::Cover, &json!("ftp://x")).is_err());
	}

	#[test]
	fn locks_serialize_only_held_fields() {
		let mut locks = Locks::new();
		lock_field(&mut locks, Field::Title, "tracker:anilist", Some("42"), 1_700_000_000);
		let wire = locks_json(&locks);
		assert_eq!(wire["title"]["provider"], "tracker:anilist");
		assert_eq!(wire["title"]["remoteId"], "42");
		assert!(wire.get("tags").is_none());
		assert!(unlock_field(&mut locks, Field::Title));
		assert!(!unlock_field(&mut locks, Field::Title));
		assert_eq!(parse_locks(&serialize_locks(&locks)), Locks::new());
	}

	#[tokio::test]
	async fn locks_round_trip_through_the_row() {
		let pool = SqlitePoolOptions::new()
			.max_connections(1)
			.connect("sqlite::memory:")
			.await
			.unwrap();
		sqlx::raw_sql("CREATE TABLE manga (id INTEGER PRIMARY KEY, metadata_locks TEXT NOT NULL DEFAULT '{}')")
			.execute(&pool)
			.await
			.unwrap();
		sqlx::query("INSERT INTO manga (id) VALUES (1)").execute(&pool).await.unwrap();

		let mut conn = pool.acquire().await.unwrap();
		assert_eq!(load(&mut conn, 1).await.unwrap(), Locks::new());

		let mut locks = Locks::new();
		lock_field(&mut locks, Field::Title, MANUAL, None, 5);
		sqlx::query("UPDATE manga SET metadata_locks = ?")
			.bind(serialize_locks(&locks))
			.execute(&mut *conn)
			.await
			.unwrap();
		assert_eq!(load(&mut conn, 1).await.unwrap(), locks);
	}
}
