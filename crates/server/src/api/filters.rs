//! Shape normalization for static filters (the source.json `filters` array and filters.json).
//!
//! A source package's static filters are Aidoku's public schema (camelCase, `id` optional), while a source
//! exporting `get_filters` serializes aidoku's `Filter` into a flat snake_case shape (id always present).
//! `/sources/{id}/filters` normalizes static items into the latter before merging with dynamic ones, so the
//! frontend handles exactly one shape; dynamic exports are left untouched.

use serde_json::{Map, Value};

/// Flat static schema -> the dynamic export's own shape; non-array input yields empty
pub fn normalize_static_filters(raw: &Value) -> Vec<Value> {
	raw.as_array()
		.map(|items| items.iter().map(normalize_filter).collect())
		.unwrap_or_default()
}

/// Missing fields always land on null, left to the frontend's `??` fallback
fn field(item: &Value, key: &str) -> Value {
	item.get(key).cloned().unwrap_or(Value::Null)
}

fn normalize_filter(item: &Value) -> Value {
	let ty = item.get("type").and_then(Value::as_str).unwrap_or_default();
	// Aidoku decoding of static filters falls back from id to title to type; id-less items sharing an empty id
	// would make the frontend's id-indexed state and React keys clobber each other
	let id = item
		.get("id")
		.and_then(Value::as_str)
		.or_else(|| item.get("title").and_then(Value::as_str))
		.unwrap_or(ty);
	let mut out = Map::new();
	out.insert("id".into(), Value::String(id.to_string()));
	out.insert("title".into(), field(item, "title"));
	out.insert("hide_from_header".into(), field(item, "hideFromHeader"));
	out.insert("type".into(), Value::String(ty.to_string()));
	out.extend(variant_fields(ty, item));
	Value::Object(out)
}

/// Variant fields keep the same names and set as the dynamic `Filter` serialization, camelCase capability keys mapped to snake_case;
/// unknown types carry no variant fields, so the frontend's else branch shows the unsupported type
fn variant_fields(ty: &str, item: &Value) -> Map<String, Value> {
	let mut out = Map::new();
	match ty {
		"text" => {
			out.insert("placeholder".into(), field(item, "placeholder"));
		}
		"sort" => {
			out.insert("can_ascend".into(), field(item, "canAscend"));
			out.insert("options".into(), field(item, "options"));
			out.insert("default".into(), field(item, "default"));
		}
		"check" => {
			out.insert("name".into(), field(item, "name"));
			out.insert("can_exclude".into(), field(item, "canExclude"));
			out.insert("default".into(), field(item, "default"));
		}
		"select" => {
			out.insert("is_genre".into(), field(item, "isGenre"));
			out.insert("uses_tag_style".into(), field(item, "usesTagStyle"));
			out.insert("options".into(), field(item, "options"));
			out.insert("ids".into(), field(item, "ids"));
			out.insert("default".into(), field(item, "default"));
		}
		"multi-select" => {
			out.insert("is_genre".into(), field(item, "isGenre"));
			out.insert("can_exclude".into(), field(item, "canExclude"));
			out.insert("uses_tag_style".into(), field(item, "usesTagStyle"));
			out.insert("options".into(), field(item, "options"));
			out.insert("ids".into(), field(item, "ids"));
			out.insert("default_included".into(), field(item, "defaultIncluded"));
			out.insert("default_excluded".into(), field(item, "defaultExcluded"));
		}
		"note" => {
			out.insert("text".into(), field(item, "text"));
		}
		"range" => {
			out.insert("min".into(), field(item, "min"));
			out.insert("max".into(), field(item, "max"));
			out.insert("decimal".into(), field(item, "decimal"));
		}
		_ => {}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn id_less_select_derives_id_from_title() {
		let raw = json!([
			{"type": "select", "title": "Genre", "options": ["A", "B"], "isGenre": true, "usesTagStyle": true},
			{"type": "select", "title": "Status", "options": ["Ongoing"]}
		]);
		let out = normalize_static_filters(&raw);
		assert_eq!(out[0]["id"], "Genre");
		assert_eq!(out[0]["is_genre"], true);
		assert_eq!(out[0]["uses_tag_style"], true);
		assert_eq!(out[0]["options"], json!(["A", "B"]));
		assert_eq!(out[0]["ids"], Value::Null);
		assert_eq!(out[0]["default"], Value::Null);
		assert_eq!(out[0]["hide_from_header"], Value::Null);
		// Two id-less filters each get a stable id, so their state no longer clobbers each other
		assert_eq!(out[1]["id"], "Status");
		assert_eq!(out[1]["is_genre"], Value::Null);
	}

	#[test]
	fn id_falls_back_to_type_then_keeps_explicit() {
		let raw = json!([
			{"type": "text", "placeholder": "Search"},
			{"type": "note", "id": "n1", "text": "hello"}
		]);
		let out = normalize_static_filters(&raw);
		assert_eq!(out[0]["id"], "text");
		assert_eq!(out[0]["title"], Value::Null);
		assert_eq!(out[0]["placeholder"], "Search");
		assert_eq!(out[1]["id"], "n1");
		assert_eq!(out[1]["text"], "hello");
	}

	#[test]
	fn capability_keys_map_to_snake_case() {
		let raw = json!([
			{"type": "sort", "id": "sort", "canAscend": false, "options": ["New"], "default": {"index": 0, "ascending": false}},
			{"type": "check", "id": "official", "canExclude": true, "default": true},
			{"type": "multi-select", "title": "Tags", "options": ["x"], "canExclude": true,
			 "defaultIncluded": ["x"], "hideFromHeader": true}
		]);
		let out = normalize_static_filters(&raw);
		assert_eq!(out[0]["can_ascend"], false);
		assert_eq!(out[0]["default"], json!({"index": 0, "ascending": false}));
		assert!(out[0].get("canAscend").is_none());

		assert_eq!(out[1]["can_exclude"], true);
		assert_eq!(out[1]["name"], Value::Null);
		assert_eq!(out[1]["default"], true);

		assert_eq!(out[2]["id"], "Tags");
		assert_eq!(out[2]["can_exclude"], true);
		assert_eq!(out[2]["default_included"], json!(["x"]));
		assert_eq!(out[2]["default_excluded"], Value::Null);
		assert_eq!(out[2]["hide_from_header"], true);
	}

	#[test]
	fn unknown_type_keeps_no_variant_fields() {
		let raw = json!([{"type": "mystery", "title": "M", "canExclude": true}]);
		let out = normalize_static_filters(&raw);
		assert_eq!(out[0]["id"], "M");
		assert_eq!(out[0]["type"], "mystery");
		assert!(out[0].get("can_exclude").is_none());
	}

	#[test]
	fn non_array_input_is_empty() {
		assert!(normalize_static_filters(&json!({"type": "select"})).is_empty());
		assert!(normalize_static_filters(&Value::Null).is_empty());
	}
}
