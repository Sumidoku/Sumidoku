//! 静态 settings.json 与动态 get_settings 的形状归一。
//!
//! Aidoku 源包里的 `settings.json` 是扁平的公开 schema（camelCase，如
//! `requiresFalse`/`minimumValue`），而源导出 `get_settings` 时 aidoku 的
//! `Setting` 序列化成嵌套的 `{type,key,title,...,value:{Kind:{...}}}`（snake_case）。
//! `/sources/{id}/settings` 对两种来源都返回后者，前端才只需处理一种形状。

use serde_json::{Map, Value, json};

/// 扁平 settings.json -> 动态源同款嵌套数组；输入非数组时返回空
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

/// 缺失字段一律落 null，交给前端 `??` 兜底
fn field(item: &Value, key: &str) -> Value {
	item.get(key).cloned().unwrap_or(Value::Null)
}

fn normalize_item(item: &Value, index: usize) -> Value {
	let ty = item.get("type").and_then(Value::as_str).unwrap_or_default();
	// 容器类（group/page）在扁平 schema 里没有 key；给个稳定的合成 key，
	// 避免前端 map 时 React key 重复
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
		// 未知类型保留原字符串：前端命中 else 分支展示"暂不支持的类型"，不崩
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
}
