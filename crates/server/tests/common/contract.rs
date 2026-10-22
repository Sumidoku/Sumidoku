use axum::http::{HeaderMap, StatusCode, header};
use serde_json::Value;
use std::sync::OnceLock;

static DOCUMENT: OnceLock<Value> = OnceLock::new();

pub fn document() -> &'static Value {
	DOCUMENT.get_or_init(|| {
		let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/api-docs/openapi.json"))
			.expect("read OpenAPI document");
		serde_json::from_str(&source).expect("valid OpenAPI JSON")
	})
}

fn resolve_ref<'a>(document: &'a Value, reference: &str) -> Result<&'a Value, String> {
	let mut current = document;
	let Some(pointer) = reference.strip_prefix("#/") else {
		return Err(format!("unsupported reference {reference}"));
	};
	for raw in pointer.split('/') {
		let part = raw.replace("~1", "/").replace("~0", "~");
		current = current
			.get(&part)
			.ok_or_else(|| format!("unresolved reference {reference}"))?;
	}
	Ok(current)
}

fn path_matches(template: &str, actual: &str) -> bool {
	let mut template_parts = template.split('/');
	let mut actual_parts = actual.split('/');
	loop {
		match (template_parts.next(), actual_parts.next()) {
			(Some(expected), Some(actual)) => {
				let parameter = expected.starts_with('{') && expected.ends_with('}');
				if (parameter && actual.is_empty()) || (!parameter && expected != actual) {
					return false;
				}
			}
			(None, None) => return true,
			_ => return false,
		}
	}
}

fn literal_segments(template: &str) -> usize {
	template
		.split('/')
		.filter(|part| !part.is_empty() && !(part.starts_with('{') && part.ends_with('}')))
		.count()
}

fn response_schema<'a>(
	document: &'a Value,
	method: &str,
	uri: &str,
	status: StatusCode,
) -> Result<Option<&'a Value>, String> {
	let actual_path = uri.split('?').next().unwrap_or(uri);
	let method = method.to_ascii_lowercase();
	let mut matched = None;
	let mut path_matched = false;
	for (template, path_item) in document["paths"].as_object().expect("paths object") {
		if !path_matches(template, actual_path) {
			continue;
		}
		path_matched = true;
		let Some(operation) = path_item.get(&method) else {
			continue;
		};
		let rank = literal_segments(template);
		if matched.as_ref().is_none_or(|(best, _, _)| rank > *best) {
			matched = Some((rank, template, operation));
		}
	}
	let Some((_, template, operation)) = matched else {
		if !path_matched && status == StatusCode::NOT_FOUND {
			return Ok(Some(&document["components"]["schemas"]["Error"]));
		}
		return Err(format!("no OpenAPI operation for {method} {actual_path}"));
	};
	if operation.get("x-websocket").is_some() {
		return Ok(None);
	}
	let responses = operation["responses"].as_object().expect("responses object");
	let response = responses
		.get(status.as_str())
		.or_else(|| responses.get("default"))
		.ok_or_else(|| format!("no OpenAPI response for {method} {template} status {status}"))?;
	let response = if let Some(reference) = response["$ref"].as_str() {
		resolve_ref(document, reference)?
	} else {
		response
	};
	let schema = &response["content"]["application/json"]["schema"];
	if schema.is_null() {
		return Err(format!("no application/json schema for {method} {template} status {status}"));
	}
	Ok(Some(match schema["$ref"].as_str() {
		Some(reference) => resolve_ref(document, reference)?,
		None => schema,
	}))
}

fn validate(document: &Value, schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
	if value.is_null() && schema["nullable"].as_bool() == Some(true) {
		return;
	}
	if let Some(reference) = schema["$ref"].as_str() {
		match resolve_ref(document, reference) {
			Ok(resolved) => validate(document, resolved, value, path, errors),
			Err(error) => errors.push(format!("{path}: {error}")),
		}
		return;
	}
	if let Some(all) = schema["allOf"].as_array() {
		for part in all {
			validate(document, part, value, path, errors);
		}
	}
	if let Some(variants) = schema["oneOf"].as_array() {
		let matches = variants
			.iter()
			.filter(|variant| {
				let mut branch_errors = Vec::new();
				validate(document, variant, value, path, &mut branch_errors);
				branch_errors.is_empty()
			})
			.count();
		if matches != 1 {
			errors.push(format!("{path}: expected exactly one oneOf match, got {matches}"));
		}
	}
	if let Some(expected) = schema["enum"].as_array()
		&& !expected.contains(value)
	{
		errors.push(format!("{path}: value {value} is not in enum {expected:?}"));
	}

	if let Some(tuple) = schema["x-tuple"].as_array() {
		let Some(items) = value.as_array() else {
			errors.push(format!("{path}: expected tuple, got {value}"));
			return;
		};
		if items.len() != tuple.len() {
			errors.push(format!("{path}: expected {} tuple items, got {}", tuple.len(), items.len()));
			return;
		}
		for (index, (item_schema, item)) in tuple.iter().zip(items).enumerate() {
			validate(document, item_schema, item, &format!("{path}[{index}]"), errors);
		}
		return;
	}

	let inferred_object = schema.get("type").is_none()
		&& (schema.get("properties").is_some() || schema.get("additionalProperties").is_some());
	let kind = schema["type"].as_str().or(if inferred_object { Some("object") } else { None });
	match kind {
		Some("object") => {
			let Some(object) = value.as_object() else {
				errors.push(format!("{path}: expected object, got {value}"));
				return;
			};
			let properties = schema["properties"].as_object();
			if let Some(required) = schema["required"].as_array() {
				for key in required.iter().filter_map(Value::as_str) {
					if !object.contains_key(key) {
						errors.push(format!("{path}.{key}: required property is missing"));
					}
				}
			}
			if let Some(properties) = properties {
				for (key, property_schema) in properties {
					if let Some(property) = object.get(key) {
						validate(document, property_schema, property, &format!("{path}.{key}"), errors);
					}
				}
			}
			match schema.get("additionalProperties") {
				Some(Value::Bool(false)) => {
					for key in object.keys() {
						if properties.is_none_or(|properties| !properties.contains_key(key)) {
							errors.push(format!("{path}.{key}: additional property is not allowed"));
						}
					}
				}
				Some(additional @ Value::Object(_)) => {
					for (key, item) in object {
						if properties.is_none_or(|properties| !properties.contains_key(key)) {
							validate(document, additional, item, &format!("{path}.{key}"), errors);
						}
					}
				}
				_ => {}
			}
		}
		Some("array") => {
			let Some(items) = value.as_array() else {
				errors.push(format!("{path}: expected array, got {value}"));
				return;
			};
			if let Some(minimum) = schema["minItems"].as_u64()
				&& items.len() < minimum as usize
			{
				errors.push(format!("{path}: expected at least {minimum} items, got {}", items.len()));
			}
			if let Some(maximum) = schema["maxItems"].as_u64()
				&& items.len() > maximum as usize
			{
				errors.push(format!("{path}: expected at most {maximum} items, got {}", items.len()));
			}
			if !schema["items"].is_null() {
				for (index, item) in items.iter().enumerate() {
					validate(document, &schema["items"], item, &format!("{path}[{index}]"), errors);
				}
			}
		}
		Some("string") => {
			if !value.is_string() {
				errors.push(format!("{path}: expected string, got {value}"));
			}
		}
		Some("integer") => {
			if value.as_i64().is_none() && value.as_u64().is_none() {
				errors.push(format!("{path}: expected integer, got {value}"));
			}
		}
		Some("number") => {
			if !value.is_number() {
				errors.push(format!("{path}: expected number, got {value}"));
			}
		}
		Some("boolean") => {
			if !value.is_boolean() {
				errors.push(format!("{path}: expected boolean, got {value}"));
			}
		}
		Some("null") => {
			if !value.is_null() {
				errors.push(format!("{path}: expected null, got {value}"));
			}
		}
		Some(other) => errors.push(format!("{path}: unsupported schema type {other}")),
		None => {}
	}
}

pub fn validate_json(document: &Value, schema: &Value, value: &Value) -> Result<(), String> {
	let mut errors = Vec::new();
	validate(document, schema, value, "$", &mut errors);
	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors.join("\n"))
	}
}

pub fn validate_response(
	method: &str,
	uri: &str,
	status: StatusCode,
	headers: &HeaderMap,
	bytes: &[u8],
) -> Result<(), String> {
	if !uri.starts_with("/api/v1/") || bytes.is_empty() {
		return Ok(());
	}
	let json = headers
		.get(header::CONTENT_TYPE)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.split(';').next())
		.is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
	if !json {
		return Ok(());
	}
	let document = document();
	let Some(schema) = response_schema(document, method, uri, status)? else {
		return Ok(());
	};
	let value: Value = serde_json::from_slice(bytes).map_err(|error| format!("invalid JSON response: {error}"))?;
	validate_json(document, schema, &value)
}
