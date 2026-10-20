//! Shape normalization for listings: static-entry parsing and the wire shape.
//!
//! A source package's static listing is Aidoku's public schema (a bare string or an object, kind numeric 0/1),
//! while a dynamically exported `get_listings` `Listing` serializes kind as the enum name
//! ("Default"/"List"). `/sources/{id}/listings` normalizes both sources into one shape
//! (`{id, name, kind}`, kind lowercase `default`/`list`), so the frontend handles exactly one shape.

use aidoku::{Listing, ListingKind};
use serde_json::{Value, json};

/// Static entries parse by AidokuRunner's SourceInfo decoding semantics: a bare string → id=name=the string;
/// an object with no name backfills id, and only numeric kind 1 is List (Runner decodes as UInt8, failures fall to default)
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

/// The endpoint's wire shape: kind normalized to a lowercase string; the frontend keys the row layout on "list"
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
		// 0, absent, and non-numeric all fall to default (Runner decodes as UInt8)
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
