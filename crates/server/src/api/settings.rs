//! Shape normalization for static settings.json and dynamic get_settings.
//!
//! The `settings.json` in an Aidoku source package is a flat public schema (camelCase, e.g.
//! `requiresFalse`/`minimumValue`), while a source exporting `get_settings` serializes aidoku's
//! `Setting` into the nested `{type,key,title,...,value:{Kind:{...}}}` (snake_case).
//! `/sources/{id}/settings` returns the latter for both sources, so the frontend handles exactly one shape.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::source::{
	candidate_urls, default_language_pick, language_select_single, source_languages,
};

/// Synthetic setting: mirrors the base-url branch of AidokuRunner's `Source.getExtraSettings`.
///
/// When `config.allowsBaseUrlSelect` is true and there are at least two candidates (the dynamically resolved
/// value prepended to `info.url` + `info.urls`), the app generates a mirror-select select in the source settings
/// (key `url`, defaulting to the first), which the source reads at runtime via `defaults.get("url")`. Here we build
/// the same nested shape, inserted before the static/dynamic settings (AidokuRunner ranks extraSettings before staticSettings).
pub fn base_url_setting(doc: &Value, persisted: Option<&str>) -> Option<Value> {
	if doc
		.get("config")
		.and_then(|c| c.get("allowsBaseUrlSelect"))
		.and_then(Value::as_bool)
		!= Some(true)
	{
		return None;
	}
	let urls = candidate_urls(doc, persisted);
	let (first, rest) = urls.split_first()?;
	if rest.is_empty() {
		return None;
	}
	let default = first.clone();
	let select = json!({
		"type": "select",
		"key": "url",
		"title": "基础地址",
		"notification": null,
		"requires": null,
		"requires_false": null,
		"refreshes": ["content"],
		"value": { "Select": {
			"values": urls,
			"titles": null,
			"auth_to_open": null,
			"default": default,
		} },
	});
	Some(json!({
		"type": "group",
		"key": "__base_url",
		"title": "基础地址",
		"notification": null,
		"requires": null,
		"requires_false": null,
		"refreshes": null,
		"value": { "Group": { "footer": null, "items": [select] } },
	}))
}

/// Synthetic setting: mirrors the language branch of AidokuRunner's `Source.getExtraSettings`.
///
/// More than one `info.languages` entry generates a language setting: a single-choice select (key language) when
/// languageSelectType == "single", otherwise a multi-select (key languages); the default takes isDefault, or the
/// first when none (AidokuRunner intersects with the device language, which a server does not have). Inserted before
/// base url (Runner's extraSettings order).
pub fn language_setting(doc: &Value) -> Option<Value> {
	let langs = source_languages(doc);
	if langs.len() < 2 {
		return None;
	}
	let values: Vec<String> = langs.iter().map(|(code, _)| code.clone()).collect();
	let picked = default_language_pick(&langs);
	let single = language_select_single(doc);
	let (ty, key, value) = if single {
		(
			"select",
			"language",
			json!({ "Select": {
				"values": values,
				"titles": null,
				"auth_to_open": null,
				"default": picked.first(),
			} }),
		)
	} else {
		(
			"multi-select",
			"languages",
			json!({ "MultiSelect": {
				"values": values,
				"titles": null,
				"auth_to_open": null,
				"default": picked,
			} }),
		)
	};
	let select = json!({
		"type": ty,
		"key": key,
		"title": "语言",
		"notification": key,
		"requires": null,
		"requires_false": null,
		"refreshes": ["content"],
		"value": value,
	});
	Some(json!({
		"type": "group",
		"key": "__language",
		"title": "语言",
		"notification": null,
		"requires": null,
		"requires_false": null,
		"refreshes": null,
		"value": { "Group": { "footer": null, "items": [select] } },
	}))
}

/// Leaf setting keys the source itself declares (recursing into group/page, skipping containers)
pub fn declared_keys(items: &[Value]) -> HashSet<String> {
	let mut keys = HashSet::new();
	collect_leaf_keys(items, &mut keys);
	keys
}

/// Whether a synthesized language/mirror setting collides with a key the source already declared: on collision skip synthesizing, so no key appears twice
pub fn clashes(extra: &Value, declared: &HashSet<String>) -> bool {
	let mut keys = HashSet::new();
	collect_leaf_keys(std::slice::from_ref(extra), &mut keys);
	keys.iter().any(|key| declared.contains(key))
}

fn collect_leaf_keys(items: &[Value], out: &mut HashSet<String>) {
	for item in items {
		let container = item
			.get("value")
			.and_then(|value| value.get("Group").or_else(|| value.get("Page")));
		if let Some(container) = container {
			if let Some(children) = container.get("items").and_then(Value::as_array) {
				collect_leaf_keys(children, out);
			}
			continue;
		}
		if let Some(key) = item.get("key").and_then(Value::as_str) {
			out.insert(key.to_string());
		}
	}
}

/// Merges static + dynamic settings: static first, dynamic appended, aligned with AidokuRunner's
/// `staticSettings + getSettings()`. Runner itself does not dedupe (a duplicate key shows twice); here we dedupe by
/// leaf key keeping the first occurrence — the same rule as synthesized settings skipping on collision, so a key appears only once in the list
pub fn merge_settings(static_items: Vec<Value>, dynamic_items: Vec<Value>) -> Vec<Value> {
	let mut declared = declared_keys(&static_items);
	let mut out = static_items;
	for item in dynamic_items {
		if clashes(&item, &declared) {
			continue;
		}
		collect_leaf_keys(std::slice::from_ref(&item), &mut declared);
		out.push(item);
	}
	out
}

/// Flat settings.json -> the dynamic source's own nested array; non-array input yields empty
pub fn normalize_static_settings(raw: &Value) -> Vec<Value> {
	raw.as_array()
		.map(|items| {
			items
				.iter()
				.enumerate()
				.map(|(i, item)| normalize_item(item, i))
				.collect()
		})
		.unwrap_or_default()
}

fn normalize_children(items: Option<&Value>) -> Value {
	let Some(items) = items.and_then(Value::as_array) else {
		return Value::Array(Vec::new());
	};
	Value::Array(
		items
			.iter()
			.enumerate()
			.map(|(i, item)| normalize_item(item, i))
			.collect(),
	)
}

/// Missing fields always land on null, left to the frontend's `??` fallback
fn field(item: &Value, key: &str) -> Value {
	item.get(key).cloned().unwrap_or(Value::Null)
}

fn normalize_item(item: &Value, index: usize) -> Value {
	let ty = item.get("type").and_then(Value::as_str).unwrap_or_default();
	// Container kinds (group/page) have no key in the flat schema; give them a stable synthetic key
	// so the frontend's map does not produce duplicate React keys
	let key = item
		.get("key")
		.and_then(Value::as_str)
		.map(str::to_string)
		.unwrap_or_else(|| format!("__{ty}_{index}"));
	let mut out = Map::new();
	out.insert("type".into(), Value::String(ty.to_string()));
	out.insert("key".into(), Value::String(key));
	out.insert("title".into(), field(item, "title"));
	out.insert("notification".into(), field(item, "notification"));
	out.insert("requires".into(), field(item, "requires"));
	out.insert("requires_false".into(), field(item, "requiresFalse"));
	out.insert("refreshes".into(), field(item, "refreshes"));
	out.insert("value".into(), normalize_value(ty, item));
	Value::Object(out)
}

fn normalize_value(ty: &str, item: &Value) -> Value {
	match ty {
		"group" => json!({ "Group": {
			"footer": field(item, "footer"),
			"items": normalize_children(item.get("items")),
		} }),
		"page" => json!({ "Page": {
			"items": normalize_children(item.get("items")),
			"inline_title": field(item, "inlineTitle"),
			"auth_to_open": field(item, "authToOpen"),
			"icon": field(item, "icon"),
			"info": field(item, "info"),
		} }),
		"select" => json!({ "Select": {
			"values": field(item, "values"),
			"titles": field(item, "titles"),
			"auth_to_open": field(item, "authToOpen"),
			"default": field(item, "default"),
		} }),
		"multi-select" => json!({ "MultiSelect": {
			"values": field(item, "values"),
			"titles": field(item, "titles"),
			"auth_to_open": field(item, "authToOpen"),
			"default": field(item, "default"),
		} }),
		"switch" => json!({ "Toggle": {
			"subtitle": field(item, "subtitle"),
			"auth_to_disable": field(item, "authToDisable"),
			"default": item.get("default").and_then(Value::as_bool).unwrap_or(false),
		} }),
		"stepper" => json!({ "Stepper": {
			"minimum_value": field(item, "minimumValue"),
			"maximum_value": field(item, "maximumValue"),
			"step_value": field(item, "stepValue"),
			"default": field(item, "default"),
		} }),
		"segment" => json!({ "Segment": {
			"options": field(item, "options"),
			"default": field(item, "default"),
		} }),
		"text" => json!({ "Text": {
			"placeholder": field(item, "placeholder"),
			"autocapitalization_type": field(item, "autocapitalizationType"),
			"autocorrection_disabled": field(item, "autocorrectionDisabled"),
			"keyboard_type": field(item, "keyboardType"),
			"return_key_type": field(item, "returnKeyType"),
			"secure": field(item, "secure"),
			"default": field(item, "default"),
		} }),
		"button" => Value::String("Button".into()),
		"link" => json!({ "Link": {
			"url": field(item, "url"),
			"external": field(item, "external"),
		} }),
		"login" => json!({ "Login": {
			"method": field(item, "method"),
			"url": field(item, "url"),
			"url_key": field(item, "urlKey"),
			"logout_title": field(item, "logoutTitle"),
			"pkce": field(item, "pkce"),
			"token_url": field(item, "tokenUrl"),
			"callback_scheme": field(item, "callbackScheme"),
			"use_email": field(item, "useEmail"),
			"local_storage_keys": field(item, "localStorageKeys"),
			"clear_cookies_on_log_out": field(item, "clearCookiesOnLogOut"),
		} }),
		"editable-list" => json!({ "EditableList": {
			"line_limit": field(item, "lineLimit"),
			"inline": field(item, "inline"),
			"placeholder": field(item, "placeholder"),
			"default": field(item, "default"),
		} }),
		"picker" => json!({ "Picker": {
			"values": field(item, "values"),
			"titles": field(item, "titles"),
			"default": field(item, "default"),
		} }),
		// Unknown types keep the raw string: the frontend's else branch shows "unsupported type" without crashing
		other => Value::String(other.to_string()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn flat_schema_maps_to_nested() {
		let raw = json!([
			{"type": "group", "title": "G", "footer": "F", "items": [
				{"type": "switch", "key": "dataSaver", "title": "DS", "default": true},
				{"type": "select", "key": "quality", "title": "Q", "values": ["low", "high"], "default": "high"},
				{"type": "stepper", "key": "threads", "title": "T", "minimumValue": 1, "maximumValue": 8, "stepValue": 2, "default": 4}
			]},
			{"type": "select", "key": "mode", "title": "M", "values": ["a", "b"], "requiresFalse": "dataSaver"},
			{"type": "button", "title": "Do", "notification": "do"}
		]);
		let out = normalize_static_settings(&raw);

		assert_eq!(out[0]["type"], "group");
		assert_eq!(out[0]["value"]["Group"]["footer"], "F");
		let items = out[0]["value"]["Group"]["items"].as_array().unwrap();
		assert_eq!(items[0]["type"], "switch");
		assert_eq!(items[0]["value"]["Toggle"]["default"], true);
		assert_eq!(items[1]["value"]["Select"]["values"], json!(["low", "high"]));
		assert_eq!(items[1]["value"]["Select"]["default"], "high");
		assert_eq!(items[2]["value"]["Stepper"]["minimum_value"], 1);
		assert_eq!(items[2]["value"]["Stepper"]["step_value"], 2);

		assert_eq!(out[1]["requires_false"], "dataSaver");
		assert_eq!(out[2]["value"], "Button");
		assert_eq!(out[2]["notification"], "do");
	}

	#[test]
	fn non_array_input_is_empty() {
		assert!(normalize_static_settings(&json!({"type": "switch"})).is_empty());
		assert!(normalize_static_settings(&Value::Null).is_empty());
	}

	#[test]
	fn base_url_setting_matches_runner_extra_settings() {
		let doc = json!({
			"info": {
				"url": "https://main.example",
				"urls": ["https://m1.example", "https://m2.example"]
			},
			"config": {"allowsBaseUrlSelect": true}
		});
		let out = base_url_setting(&doc, None).expect("base url setting");
		assert_eq!(out["type"], "group");
		let item = &out["value"]["Group"]["items"][0];
		assert_eq!(item["type"], "select");
		assert_eq!(item["key"], "url");
		assert_eq!(
			item["value"]["Select"]["values"],
			json!(["https://main.example", "https://m1.example", "https://m2.example"])
		);
		assert_eq!(item["value"]["Select"]["default"], "https://main.example");
		assert_eq!(item["refreshes"], json!(["content"]));
	}

	#[test]
	fn base_url_setting_prepends_persisted_dynamic() {
		let doc = json!({
			"info": {
				"url": "https://main.example",
				"urls": ["https://m1.example"]
			},
			"config": {"allowsBaseUrlSelect": true}
		});
		let out = base_url_setting(&doc, Some("https://dyn.example")).expect("base url setting");
		let item = &out["value"]["Group"]["items"][0];
		assert_eq!(
			item["value"]["Select"]["values"],
			json!(["https://dyn.example", "https://main.example", "https://m1.example"])
		);
		assert_eq!(item["value"]["Select"]["default"], "https://dyn.example");

		// A persisted value already in the package list (dynamic equals static, or info.url when unresolved) is not prepended again
		let out = base_url_setting(&doc, Some("https://main.example")).expect("base url setting");
		let item = &out["value"]["Group"]["items"][0];
		assert_eq!(
			item["value"]["Select"]["values"],
			json!(["https://main.example", "https://m1.example"])
		);
		assert_eq!(item["value"]["Select"]["default"], "https://main.example");
	}

	#[test]
	fn base_url_setting_needs_config_and_two_urls() {
		let two_urls = json!({
			"info": {"url": "https://a.example", "urls": ["https://b.example"]}
		});
		assert!(base_url_setting(&two_urls, None).is_none());

		let disabled = json!({
			"info": {"url": "https://a.example", "urls": ["https://b.example"]},
			"config": {"allowsBaseUrlSelect": false}
		});
		assert!(base_url_setting(&disabled, None).is_none());

		let single = json!({
			"info": {"url": "https://a.example"},
			"config": {"allowsBaseUrlSelect": true}
		});
		assert!(base_url_setting(&single, None).is_none());
	}

	#[test]
	fn language_setting_matches_runner_extra_settings() {
		let doc = json!({"info": {"languages": ["en", "ja", "zh"]}});
		let out = language_setting(&doc).expect("language setting");
		assert_eq!(out["type"], "group");
		assert_eq!(out["title"], "语言");
		let item = &out["value"]["Group"]["items"][0];
		assert_eq!(item["type"], "multi-select");
		assert_eq!(item["key"], "languages");
		assert_eq!(item["notification"], "languages");
		assert_eq!(item["value"]["MultiSelect"]["values"], json!(["en", "ja", "zh"]));
		assert_eq!(item["value"]["MultiSelect"]["default"], json!(["en"]));
		assert_eq!(item["refreshes"], json!(["content"]));
	}

	#[test]
	fn language_setting_single_uses_is_default() {
		let doc = json!({
			"info": {"languages": [
				{"code": "en"}, {"code": "ja", "isDefault": true}
			]},
			"config": {"languageSelectType": "single"}
		});
		let out = language_setting(&doc).expect("language setting");
		let item = &out["value"]["Group"]["items"][0];
		assert_eq!(item["type"], "select");
		assert_eq!(item["key"], "language");
		assert_eq!(item["value"]["Select"]["default"], "ja");
	}

	#[test]
	fn language_setting_needs_two_languages() {
		assert!(language_setting(&json!({"info": {"languages": ["en"]}})).is_none());
		assert!(language_setting(&json!({"info": {}})).is_none());
	}

	#[test]
	fn declared_keys_clashes_guard_synthesized_settings() {
		let items = json!([
			{"type": "select", "key": "url", "title": "U", "value": {"Select": {"values": ["a"], "default": null}}},
			{"type": "switch", "key": "dataSaver", "title": "D", "value": {"Toggle": {"default": false}}},
			{"type": "group", "key": "__group_0", "title": "G", "value": {"Group": {"footer": null, "items": [
				{"type": "select", "key": "language", "title": "L", "value": {"Select": {"values": ["en"], "default": null}}}
			]}}}
		]);
		let keys = declared_keys(items.as_array().unwrap());
		assert!(keys.contains("url"));
		assert!(keys.contains("dataSaver"));
		assert!(keys.contains("language"));
		assert!(!keys.contains("__group_0"), "容器合成键不算设置键");

		let doc = json!({
			"info": {"url": "https://a.example", "urls": ["https://b.example"]},
			"config": {"allowsBaseUrlSelect": true}
		});
		let base = base_url_setting(&doc, None).expect("base url setting");
		assert!(clashes(&base, &keys), "源已声明 url，合成项应判冲突");

		let single = json!({
			"info": {"languages": ["en", "ja"]},
			"config": {"languageSelectType": "single"}
		});
		let language = language_setting(&single).expect("language setting");
		assert!(clashes(&language, &keys), "源已声明 language，合成项应判冲突");
	}
}
