//! Structured server settings types and conversions.
//!
//! `/api/v1/settings` now returns and accepts a nested object (camelCase) instead of the
//! legacy flat dotted-key map. This module holds the typed view and the bidirectional
//! conversion to the flat map still stored in the `setting` table.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
	pub incognito: IncognitoSettings,
	pub library: LibrarySettings,
	pub local: LocalSettings,
	pub download: DownloadSettings,
	pub backup: BackupSettings,
	pub browse: BrowseSettings,
	#[serde(rename = "readOnly")]
	pub read_only: ReadOnlySettings,
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
	pub skip_titles: Vec<String>,
	pub refresh_metadata: bool,
	pub update_interval_hours: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSettings {
	pub dirs: Vec<String>,
	pub oneshots_dir: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
	pub move_path: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSettings {
	pub enabled: bool,
	pub interval_hours: u64,
	pub retention_count: u64,
	pub last_run: Option<u64>,
	pub dir: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseSettings {
	pub pinned_sources: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlySettings {
	pub local_dirs: Vec<String>,
	pub local_oneshots_dir: Option<String>,
}

impl Default for LibrarySettings {
	fn default() -> Self {
		Self {
			default_category: "all".into(),
			show_uncategorized: true,
			excluded_update_categories: Vec::new(),
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
			skip_titles: get_string_array(map, "library.skip_titles")
				.unwrap_or_else(|| crate::update::SKIP_TITLE_RULES.iter().map(|s| (*s).into()).collect()),
			refresh_metadata: get_bool(map, "library.refresh_metadata").unwrap_or(false),
			update_interval_hours: get_u64(map, "library.update_interval_hours").unwrap_or(0),
		};
		let local = LocalSettings {
			dirs: get_string_array(map, "local.dirs").unwrap_or_default(),
			oneshots_dir: get_string(map, "local.oneshots_dir"),
		};
		let download = DownloadSettings {
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
		let read_only = ReadOnlySettings {
			local_dirs: get_string_array(map, "localDirs").unwrap_or_default(),
			local_oneshots_dir: get_string(map, "localOneshotsDir"),
		};

		// Anything not modeled above is preserved round-trip (excluding injected/read-only/internal keys).
		let known: HashSet<&str> = [
			"incognito.enabled",
			"library.default_category",
			"library.show_uncategorized",
			"library.excluded_update_categories",
			"library.skip_titles",
			"library.refresh_metadata",
			"library.update_interval_hours",
			"local.dirs",
			"local.oneshots_dir",
			"download.move_path",
			"backup.enabled",
			"backup.interval_hours",
			"backup.retention_count",
			"backup.last_run",
			"backup.dir",
			"browse.pinned_sources",
			"localDirs",
			"localOneshotsDir",
			"sync.instance_id",
		]
		.iter()
		.copied()
		.collect();
		let extra = map
			.iter()
			.filter(|(k, _)| !known.contains(k.as_str()) && !k.starts_with("user."))
			.map(|(k, v)| (k.clone(), v.clone()))
			.collect();

		Ok(Self { incognito, library, local, download, backup, browse, read_only, extra })
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
		map.insert("library.skip_titles".into(), json!(self.library.skip_titles));
		map.insert("library.refresh_metadata".into(), json!(self.library.refresh_metadata));
		map.insert("library.update_interval_hours".into(), json!(self.library.update_interval_hours));
		map.insert("local.dirs".into(), json!(self.local.dirs));
		if let Some(oneshots_dir) = self.local.oneshots_dir {
			map.insert("local.oneshots_dir".into(), json!(oneshots_dir));
		}
		map.insert("download.move_path".into(), json!(self.download.move_path));
		map.insert("backup.enabled".into(), json!(self.backup.enabled));
		map.insert("backup.interval_hours".into(), json!(self.backup.interval_hours));
		map.insert("backup.retention_count".into(), json!(self.backup.retention_count));
		if let Some(last_run) = self.backup.last_run {
			map.insert("backup.last_run".into(), json!(last_run));
		}
		if let Some(dir) = self.backup.dir {
			map.insert("backup.dir".into(), json!(dir));
		}
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
