//! Structured server settings types and conversions.
//!
//! `/api/v1/settings` now returns and accepts a nested object (camelCase) instead of the
//! legacy flat dotted-key map. This module holds the typed view and the bidirectional
//! conversion to the flat map still stored in the `setting` table.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(crate) const ROOT_GROUPS: [&str; 7] = [
	"incognito",
	"library",
	"local",
	"download",
	"backup",
	"browse",
	"readOnly",
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
	pub incognito: IncognitoSettings,
	pub library: LibrarySettings,
	pub download: DownloadSettings,
	pub backup: BackupSettings,
	pub browse: BrowseSettings,
	/// Any settings keys the server does not yet model as typed fields. Preserved round-trip so that
	/// backups, custom settings, or future keys are not lost when writing the structured view back.
	#[serde(flatten)]
	pub extra: HashMap<String, Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncognitoSettings {
	pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySettings {
	pub default_category: String,
	pub show_uncategorized: bool,
	pub excluded_update_categories: Vec<i64>,
	pub excluded_download_categories: Vec<i64>,
	pub skip_titles: Vec<String>,
	pub refresh_metadata: bool,
	pub update_interval_hours: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
	pub auto_download_new_chapters: bool,
	pub auto_download_limit: u64,
	pub download_format: String,
	pub download_concurrency: u64,
	pub delete_after_manual_read: bool,
	pub delete_after_read: bool,
	pub allow_delete_bookmarked: bool,
	pub auto_skip_manga_with_unread: bool,
	pub auto_ignore_reuploads: bool,
	pub download_ahead: bool,
	pub download_ahead_count: u64,
	#[serde(deserialize_with = "required_option")]
	pub move_path: Option<String>,
}

impl Default for DownloadSettings {
	fn default() -> Self {
		Self {
			auto_download_new_chapters: false,
			auto_download_limit: 5,
			download_format: "cbz".into(),
			download_concurrency: 3,
			delete_after_manual_read: false,
			delete_after_read: false,
			allow_delete_bookmarked: false,
			auto_skip_manga_with_unread: true,
			auto_ignore_reuploads: false,
			download_ahead: false,
			download_ahead_count: 2,
			move_path: None,
		}
	}
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSettings {
	pub enabled: bool,
	pub interval_hours: u64,
	pub retention_count: u64,
	#[serde(deserialize_with = "required_option")]
	pub last_run: Option<u64>,
	#[serde(deserialize_with = "required_option")]
	pub dir: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseSettings {
	pub pinned_sources: Vec<String>,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
	D: serde::Deserializer<'de>,
	T: Deserialize<'de>,
{
	Option::<T>::deserialize(deserializer)
}

impl Default for LibrarySettings {
	fn default() -> Self {
		Self {
			default_category: "all".into(),
			show_uncategorized: true,
			excluded_update_categories: Vec::new(),
			excluded_download_categories: Vec::new(),
			skip_titles: crate::update::SKIP_TITLE_RULES.iter().map(|s| (*s).into()).collect(),
			refresh_metadata: false,
			update_interval_hours: 0,
		}
	}
}

impl ServerSettings {
	/// Build the structured view from the legacy flat map produced by the database layer.
	pub fn from_flat(map: &serde_json::Map<String, Value>) -> crate::error::ApiResult<Self> {
		let incognito = IncognitoSettings {
			enabled: get_bool(map, "incognito.enabled").unwrap_or(false),
		};
		let library = LibrarySettings {
			default_category: get_string(map, "library.default_category").unwrap_or_else(|| "all".into()),
			show_uncategorized: get_bool(map, "library.show_uncategorized").unwrap_or(true),
			excluded_update_categories: get_i64_array(map, "library.excluded_update_categories").unwrap_or_default(),
			excluded_download_categories: get_i64_array(map, "library.excluded_download_categories").unwrap_or_default(),
			skip_titles: get_string_array(map, "library.skip_titles")
				.unwrap_or_else(|| crate::update::SKIP_TITLE_RULES.iter().map(|s| (*s).into()).collect()),
			refresh_metadata: get_bool(map, "library.refresh_metadata").unwrap_or(false),
			update_interval_hours: get_u64(map, "library.update_interval_hours").unwrap_or(0),
		};
		let download = DownloadSettings {
			auto_download_new_chapters: get_bool(map, "auto_download_new_chapters").unwrap_or(false),
			auto_download_limit: get_u64(map, "auto_download_limit").unwrap_or(5),
			download_format: get_string(map, "download_format").unwrap_or_else(|| "cbz".into()),
			download_concurrency: get_u64(map, "download_concurrency").unwrap_or(3),
			delete_after_manual_read: get_bool(map, "delete_after_manual_read").unwrap_or(false),
			delete_after_read: get_bool(map, "delete_after_read").unwrap_or(false),
			allow_delete_bookmarked: get_bool(map, "allow_delete_bookmarked").unwrap_or(false),
			auto_skip_manga_with_unread: get_bool(map, "downloads.auto_skip_manga_with_unread").unwrap_or(true),
			auto_ignore_reuploads: get_bool(map, "downloads.auto_ignore_reuploads").unwrap_or(false),
			download_ahead: get_bool(map, "downloads.download_ahead").unwrap_or(false),
			download_ahead_count: get_u64(map, "downloads.download_ahead_count").unwrap_or(2),
			move_path: get_string(map, "download.move_path"),
		};
		let backup = BackupSettings {
			enabled: get_bool(map, "backup.enabled").unwrap_or(false),
			interval_hours: get_u64(map, "backup.interval_hours").unwrap_or(24),
			retention_count: get_u64(map, "backup.retention_count").unwrap_or(10),
			last_run: get_u64(map, "backup.last_run"),
			dir: get_string(map, "backup.dir"),
		};
		let browse = BrowseSettings {
			pinned_sources: get_string_array(map, "browse.pinned_sources").unwrap_or_default(),
		};

		// Anything not modeled above is preserved round-trip (excluding injected/read-only/internal keys).
		// The legacy global local.* rows keep working as the default local instance's fallback layer
		// (state.local_dirs), so they stay stored but are no longer exposed in the structured view.
		let known: HashSet<&str> = [
			"incognito.enabled",
			"library.default_category",
			"library.show_uncategorized",
			"library.excluded_update_categories",
			"library.excluded_download_categories",
			"library.skip_titles",
			"library.refresh_metadata",
			"library.update_interval_hours",
			crate::update::LIBRARY_LAST_SCHEDULED_RUN_AT,
			crate::local::scan::LOCAL_LAST_SCAN_AT,
			"auto_download_new_chapters",
			"auto_download_limit",
			"download_format",
			"download_concurrency",
			"delete_after_manual_read",
			"delete_after_read",
			"allow_delete_bookmarked",
			"downloads.auto_skip_manga_with_unread",
			"downloads.auto_ignore_reuploads",
			"downloads.download_ahead",
			"downloads.download_ahead_count",
			"local.dirs",
			"local.oneshots_dir",
			"download.move_path",
			"backup.enabled",
			"backup.interval_hours",
			"backup.retention_count",
			"backup.last_run",
			"backup.dir",
			"browse.pinned_sources",
			"sync.instance_id",
		]
		.iter()
		.copied()
		.collect();
		let extra = map
			.iter()
			.filter(|(k, _)| {
				!known.contains(k.as_str()) && !ROOT_GROUPS.contains(&k.as_str()) && !k.starts_with("user.")
			})
			.map(|(k, v)| (k.clone(), v.clone()))
			.collect();

		Ok(Self { incognito, library, download, backup, browse, extra })
	}

	/// Convert the structured request body into the legacy flat map understood by `apply_settings`.
	pub fn into_flat(self) -> HashMap<String, Value> {
		let mut map = HashMap::new();
		map.insert("incognito.enabled".into(), json!(self.incognito.enabled));
		map.insert("library.default_category".into(), json!(self.library.default_category));
		map.insert("library.show_uncategorized".into(), json!(self.library.show_uncategorized));
		map.insert(
			"library.excluded_update_categories".into(),
			json!(self.library.excluded_update_categories),
		);
		map.insert(
			"library.excluded_download_categories".into(),
			json!(self.library.excluded_download_categories),
		);
		map.insert("library.skip_titles".into(), json!(self.library.skip_titles));
		map.insert("library.refresh_metadata".into(), json!(self.library.refresh_metadata));
		map.insert("library.update_interval_hours".into(), json!(self.library.update_interval_hours));
		map.insert(
			"auto_download_new_chapters".into(),
			json!(self.download.auto_download_new_chapters),
		);
		map.insert("auto_download_limit".into(), json!(self.download.auto_download_limit));
		map.insert("download_format".into(), json!(self.download.download_format));
		map.insert("download_concurrency".into(), json!(self.download.download_concurrency));
		map.insert("delete_after_manual_read".into(), json!(self.download.delete_after_manual_read));
		map.insert("delete_after_read".into(), json!(self.download.delete_after_read));
		map.insert("allow_delete_bookmarked".into(), json!(self.download.allow_delete_bookmarked));
		map.insert(
			"downloads.auto_skip_manga_with_unread".into(),
			json!(self.download.auto_skip_manga_with_unread),
		);
		map.insert(
			"downloads.auto_ignore_reuploads".into(),
			json!(self.download.auto_ignore_reuploads),
		);
		map.insert("downloads.download_ahead".into(), json!(self.download.download_ahead));
		map.insert("downloads.download_ahead_count".into(), json!(self.download.download_ahead_count));
		map.insert("download.move_path".into(), json!(self.download.move_path));
		map.insert("backup.enabled".into(), json!(self.backup.enabled));
		map.insert("backup.interval_hours".into(), json!(self.backup.interval_hours));
		map.insert("backup.retention_count".into(), json!(self.backup.retention_count));
		// The backup schedule owns last_run; a full settings write must not replay a stale timestamp.
		map.insert("backup.dir".into(), json!(self.backup.dir));
		map.insert("browse.pinned_sources".into(), json!(self.browse.pinned_sources));
		map.extend(self.extra);
		map
	}
}

fn get_bool(map: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
	map.get(key).and_then(Value::as_bool)
}

fn get_u64(map: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
	map.get(key).and_then(Value::as_u64)
}

fn get_string(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
	map.get(key).and_then(Value::as_str).map(str::to_string)
}

fn get_string_array(map: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<String>> {
	map.get(key)
		.and_then(Value::as_array)
		.and_then(|arr| arr.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<Vec<_>>>())
}

fn get_i64_array(map: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<i64>> {
	map.get(key)
		.and_then(Value::as_array)
		.and_then(|arr| arr.iter().map(|v| v.as_i64()).collect::<Option<Vec<_>>>())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn root_groups_do_not_leak_into_extra_settings() {
		let map = serde_json::Map::from_iter([
			("library".into(), json!({"updateIntervalHours": 12})),
			("readOnly".into(), json!({"localDirs": ["ignored"]})),
			("custom.option".into(), json!({"keep": true})),
		]);
		let settings = ServerSettings::from_flat(&map).expect("structured settings");
		assert!(!settings.extra.contains_key("library"));
		assert!(!settings.extra.contains_key("readOnly"));
		assert_eq!(settings.extra.get("custom.option"), Some(&json!({"keep": true})));
	}
}
