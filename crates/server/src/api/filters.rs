//! 静态 filters（source.json `filters` 数组与 filters.json）的形状归一。
//!
//! 源包的静态筛选是 Aidoku 公开 schema（camelCase，`id` 可缺省），而源导出
//! `get_filters` 时 aidoku 的 `Filter` 序列化成扁平的 snake_case 形状（必有 id）。
//! `/sources/{id}/filters` 把静态项归一成后者再与动态项合并，前端才只需处理
//! 一种形状；动态导出一律不动。

use serde_json::{Map, Value};

/// 扁平静态 schema -> 动态导出同款形状；输入非数组时返回空
pub fn normalize_static_filters(raw: &Value) -> Vec<Value> {
	raw.as_array()
		.map(|items| items.iter().map(normalize_filter).collect())
		.unwrap_or_default()
}

/// 缺失字段一律落 null，交给前端 `??` 兜底
fn field(item: &Value, key: &str) -> Value {
	item.get(key).cloned().unwrap_or(Value::Null)
}

fn normalize_filter(item: &Value) -> Value {
	let ty = item.get("type").and_then(Value::as_str).unwrap_or_default();
	// Aidoku 解码静态筛选时 id 缺省落 title 再落 type；无 id 的项若共享空 id，
	// 前端按 id 索引的状态与 React key 会互相覆盖
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

/// 变体字段与动态 `Filter` 序列化保持同名同集，camelCase 能力键映射成 snake_case；
/// 未知类型不带变体字段，前端命中 else 分支展示暂不支持的类型
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
		// 两个无 id 筛选各得稳定 id，状态不再互相覆盖
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
