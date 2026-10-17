use aidoku::{Chapter, ContentRating, Manga, MangaStatus, UpdateStrategy, Viewer};
use aidoku_wasm::DefaultValue;
use serde_json::Value;

#[derive(Debug, sqlx::FromRow)]
pub struct SourceRow {
	pub id: i64,
	pub key: String,
	pub name: String,
	pub version: i64,
	pub languages: sqlx::types::Json<Vec<String>>,
	pub content_rating: i64,
	pub base_url: Option<String>,
	pub icon_path: Option<String>,
	pub wasm_path: String,
	pub enabled: bool,
	pub installed_at: i64,
	pub updated_at: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct MangaRow {
	pub id: i64,
	pub source_id: i64,
	pub key: String,
	pub title: String,
	pub authors: sqlx::types::Json<Vec<String>>,
	pub artists: sqlx::types::Json<Vec<String>>,
	pub description: Option<String>,
	pub tags: sqlx::types::Json<Vec<String>>,
	pub status: i64,
	pub content_rating: i64,
	pub cover_url: Option<String>,
	pub url: Option<String>,
	pub initialized: bool,
	pub update_strategy: i64,
	pub next_update_time: Option<i64>,
	pub last_fetched_at: Option<i64>,
	pub chapters_last_fetched_at: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ChapterRow {
	pub id: i64,
	pub manga_id: i64,
	pub key: String,
	pub title: Option<String>,
	pub number: Option<f64>,
	pub volume: Option<f64>,
	pub scanlators: sqlx::types::Json<Vec<String>>,
	pub language: Option<String>,
	pub url: Option<String>,
	pub date_uploaded: Option<i64>,
	pub source_order: i64,
	pub page_count: i64,
	pub locked: bool,
	pub fetched_at: i64,
}

/// 章节 LEFT JOIN chapter_user（user 1）的投影；四列用户字段无行时全 NULL
#[derive(Debug, sqlx::FromRow)]
pub struct ChapterUserRow {
	#[sqlx(flatten)]
	pub chapter: ChapterRow,
	pub last_page_read: Option<i64>,
	pub is_read: Option<bool>,
	pub is_bookmarked: Option<bool>,
	pub last_read_at: Option<i64>,
}

/// 库列表：manga JOIN manga_user 的投影
#[derive(Debug, sqlx::FromRow)]
pub struct LibraryEntryRow {
	#[sqlx(flatten)]
	pub manga: MangaRow,
	pub in_library_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChapterProgress {
	pub last_page_read: i64,
	pub is_read: bool,
	pub is_bookmarked: bool,
	pub last_read_at: Option<i64>,
}

fn none_if_empty(v: &[String]) -> Option<Vec<String>> {
	if v.is_empty() { None } else { Some(v.to_vec()) }
}

pub fn status_from_int(v: i64) -> MangaStatus {
	match v {
		1 => MangaStatus::Ongoing,
		2 => MangaStatus::Completed,
		3 => MangaStatus::Cancelled,
		4 => MangaStatus::Hiatus,
		_ => MangaStatus::Unknown,
	}
}

pub fn content_rating_from_int(v: i64) -> ContentRating {
	match v {
		1 => ContentRating::Safe,
		2 => ContentRating::Suggestive,
		3 => ContentRating::NSFW,
		_ => ContentRating::Unknown,
	}
}

fn update_strategy_from_int(v: i64) -> UpdateStrategy {
	match v {
		1 => UpdateStrategy::Never,
		_ => UpdateStrategy::Always,
	}
}

/// 从 DB 行还原源侧 Manga，用于回传 get_manga_update / get_page_list
pub fn manga_from_row(row: &MangaRow) -> Manga {
	Manga {
		key: row.key.clone(),
		title: row.title.clone(),
		cover: row.cover_url.clone(),
		artists: none_if_empty(&row.artists),
		authors: none_if_empty(&row.authors),
		description: row.description.clone(),
		url: row.url.clone(),
		tags: none_if_empty(&row.tags),
		status: status_from_int(row.status),
		content_rating: content_rating_from_int(row.content_rating),
		viewer: Viewer::Unknown,
		update_strategy: update_strategy_from_int(row.update_strategy),
		next_update_time: row.next_update_time,
		chapters: None,
	}
}

pub fn chapter_from_row(row: &ChapterRow) -> Chapter {
	Chapter {
		key: row.key.clone(),
		title: row.title.clone(),
		chapter_number: row.number.map(|n| n as f32),
		volume_number: row.volume.map(|n| n as f32),
		date_uploaded: row.date_uploaded,
		scanlators: none_if_empty(&row.scanlators),
		url: row.url.clone(),
		language: row.language.clone(),
		thumbnail: None,
		locked: row.locked,
	}
}

/// PUT settings 的裸 json → 引擎 DefaultValue；对象/混合数组没有对应类型，存 Data(json bytes)
pub fn json_to_default_value(v: &Value) -> DefaultValue {
	match v {
		Value::Bool(b) => DefaultValue::Bool(*b),
		Value::Number(n) => {
			if let Some(i) = n.as_i64().and_then(|i| i32::try_from(i).ok()) {
				DefaultValue::Int(i)
			} else if let Some(f) = n.as_f64() {
				DefaultValue::Float(f as f32)
			} else {
				DefaultValue::Null
			}
		}
		Value::String(s) => DefaultValue::String(s.clone()),
		Value::Array(items) if items.iter().all(Value::is_string) => DefaultValue::StringArray(
			items
				.iter()
				.filter_map(|i| i.as_str().map(str::to_string))
				.collect(),
		),
		Value::Null => DefaultValue::Null,
		other => DefaultValue::Data(serde_json::to_vec(other).unwrap_or_default()),
	}
}

/// GET settings 时把 DefaultValue 还原成裸 json，与 PUT 对称
pub fn default_value_to_json(v: &DefaultValue) -> Value {
	match v {
		DefaultValue::Bool(b) => Value::from(*b),
		DefaultValue::Int(i) => Value::from(*i),
		DefaultValue::Float(f) => Value::from(*f),
		DefaultValue::String(s) => Value::from(s.clone()),
		DefaultValue::StringArray(a) => Value::from(a.clone()),
		DefaultValue::Null => Value::Null,
		DefaultValue::Data(bytes) => {
			serde_json::from_slice(bytes).unwrap_or_else(|_| Value::from(bytes.clone()))
		}
	}
}

pub fn unix_now() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs() as i64)
		.unwrap_or(0)
}
