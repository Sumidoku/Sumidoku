//! listings 的形状归一：静态条目解析与 wire 形状。
//!
//! 源包的静态 listing 是 Aidoku 公开 schema（裸字符串或对象，kind 为数值 0/1），
//! 而动态导出 `get_listings` 的 `Listing` 经 serde 序列化后 kind 是枚举名
//! （"Default"/"List"）。`/sources/{id}/listings` 把两种来源归一成同一形状
//! （`{id, name, kind}`，kind 为小写 `default`/`list`），前端才只需处理一种形状。

use aidoku::{Listing, ListingKind};
use serde_json::{Value, json};

/// 静态条目按 AidokuRunner 的 SourceInfo 解码语义解析：裸字符串 → id=name=字符串；
/// 对象缺 name 回填 id，kind 只有数值 1 是 List（Runner 按 UInt8 解码，解析失败落 default）
pub fn parse_static_listing(value: &Value) -> Option<Listing> {
	if let Some(s) = value.as_str() {
		return Some(Listing {
			id: s.to_string(),
			name: s.to_string(),
			kind: ListingKind::Default,
		});
	}
	let id = value.get("id").and_then(|v| v.as_str())?.to_string();
	let name = value
		.get("name")
		.and_then(|v| v.as_str())
		.unwrap_or(&id)
		.to_string();
	let kind = match value.get("kind").and_then(|v| v.as_i64()) {
		Some(1) => ListingKind::List,
		_ => ListingKind::Default,
	};
	Some(Listing { id, name, kind })
}

/// 端点 wire 形状：kind 归一成小写字符串，前端按 "list" 判行式布局
pub fn listing_json(listing: &Listing) -> Value {
	let kind = match listing.kind {
		ListingKind::Default => "default",
		ListingKind::List => "list",
	};
	json!({
		"id": listing.id,
		"name": listing.name,
		"kind": kind,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn bare_string_becomes_id_and_name() {
		let l = parse_static_listing(&json!("hot")).expect("bare string");
		assert_eq!(l.id, "hot");
		assert_eq!(l.name, "hot");
		assert_eq!(l.kind, ListingKind::Default);
	}

	#[test]
	fn kind_only_numeric_one_is_list() {
		let list = parse_static_listing(&json!({"id": "a", "kind": 1})).expect("object");
		assert_eq!(list.kind, ListingKind::List);
		// 0、缺省与非数值都落 default（Runner 按 UInt8 解码）
		for raw in [
			json!({"id": "a", "kind": 0}),
			json!({"id": "a"}),
			json!({"id": "a", "kind": "List"}),
			json!({"id": "a", "kind": 2}),
		] {
			assert_eq!(parse_static_listing(&raw).expect("object").kind, ListingKind::Default, "{raw}");
		}
	}

	#[test]
	fn missing_name_falls_back_to_id_and_missing_id_is_dropped() {
		let l = parse_static_listing(&json!({"id": "fresh"})).expect("object");
		assert_eq!(l.name, "fresh");
		assert!(parse_static_listing(&json!({"name": "x", "kind": 1})).is_none());
	}

	#[test]
	fn wire_shape_normalizes_kind_to_lowercase() {
		let list = listing_json(&Listing {
			id: "dyn".into(),
			name: "Dynamic Picks".into(),
			kind: ListingKind::List,
		});
		assert_eq!(list, json!({"id": "dyn", "name": "Dynamic Picks", "kind": "list"}));
		let default = listing_json(&Listing {
			id: "a".into(),
			name: "A".into(),
			kind: ListingKind::Default,
		});
		assert_eq!(default["kind"], "default");
	}
}
