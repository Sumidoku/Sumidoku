mod common;

use axum::http::{StatusCode, header};
use common::{TestApp, request, request_full, test_app};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

fn document() -> Value {
	let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/api-docs/openapi.json"))
		.expect("read OpenAPI document");
	serde_json::from_str(&source).expect("valid OpenAPI JSON")
}

fn route_call_end(source: &str) -> usize {
	let mut depth = 1;
	let mut quoted = false;
	let mut escaped = false;
	for (index, character) in source.char_indices() {
		if quoted {
			if escaped {
				escaped = false;
			} else if character == '\\' {
				escaped = true;
			} else if character == '"' {
				quoted = false;
			}
			continue;
		}
		match character {
			'"' => quoted = true,
			'(' => depth += 1,
			')' => {
				depth -= 1;
				if depth == 0 {
					return index;
				}
			}
			_ => {}
		}
	}
	source.len()
}

fn registered_routes() -> BTreeMap<String, BTreeSet<&'static str>> {
	let mut routes = BTreeMap::new();
	let mut remaining = include_str!("../src/api/mod.rs");
	while let Some(start) = remaining.find(".route(") {
		remaining = &remaining[start + ".route(".len()..];
		let end = route_call_end(remaining);
		let call = &remaining[..end];
		remaining = &remaining[end..];
		let Some(path_start) = call.find('"') else {
			continue;
		};
		let path = &call[path_start + 1..];
		let Some(path_end) = path.find('"') else {
			continue;
		};
		let path = &path[..path_end];
		if !path.starts_with("/api/v1") {
			continue;
		}
		for method in METHODS {
			if call.contains(&format!("{method}(")) {
				routes.entry(path.to_string()).or_insert_with(BTreeSet::new).insert(method);
			}
		}
	}
	routes
}

fn documented_routes(document: &Value) -> BTreeMap<String, BTreeSet<&'static str>> {
	let mut routes = BTreeMap::new();
	for (path, operations) in document["paths"].as_object().expect("paths object") {
		if !path.starts_with("/api/v1") {
			continue;
		}
		for method in METHODS {
			if operations.get(method).is_some() {
				routes.entry(path.clone()).or_insert_with(BTreeSet::new).insert(method);
			}
		}
	}
	routes
}

#[test]
fn route_parser_reads_multiline_and_chained_methods() {
	let routes = registered_routes();
	assert_eq!(routes["/api/v1/users"], BTreeSet::from(["get", "post"]));
	assert_eq!(routes["/api/v1/sources/import"], BTreeSet::from(["post"]));
	assert_eq!(routes["/api/v1/users/{id}"], BTreeSet::from(["patch", "delete"]));
	assert_eq!(routes["/api/v1/sync/changes"], BTreeSet::from(["get", "post"]));
}

#[test]
fn document_matches_all_registered_routes() {
	assert_eq!(documented_routes(&document()), registered_routes());
}

#[test]
fn document_describes_shared_contracts() {
	let document = document();
	assert_eq!(document["openapi"], "3.0.3");
	assert!(document["components"]["securitySchemes"]["bearerAuth"].is_object());
	assert!(document["components"]["securitySchemes"]["accessTokenQuery"].is_object());
	assert!(document["components"]["schemas"]["ApiError"]["properties"]["code"].is_object());
	assert!(document["components"]["schemas"]["ApiError"]["properties"]["message"].is_object());
	assert!(document["paths"]["/api/v1/sync/changes"]["get"].is_object());
	assert!(document["paths"]["/api/v1/sync/changes"]["post"].is_object());
	assert!(document["paths"]["/api/v1/sync/compact"]["post"].is_object());
	assert!(document["paths"]["/api/v1/auth/logout"]["post"]["security"].as_array().expect("logout security").is_empty());
	assert!(document["paths"]["/api/v1/sources/{id}/filters"]["get"]["responses"]["404"].is_object());
	for path in ["/api/v1/downloads/ws", "/api/v1/update/ws"] {
		let operation = &document["paths"][path]["get"];
		assert!(operation["responses"]["101"].is_object(), "missing WebSocket upgrade for {path}");
		assert!(operation["x-websocket"].is_object(), "missing WebSocket frame contract for {path}");
	}
}

#[test]
fn representative_response_shapes_match_handlers() {
	let document = document();
	let mut errors = Vec::new();
	let page = &document["components"]["schemas"]["Page"]["properties"];
	if page["hasDescription"]["type"] != "boolean" || page.get("has_description").is_some() {
		errors.push("Page must expose hasDescription in camelCase");
	}
	if document["components"]["schemas"]["Settings"]["properties"]["library.skip_titles"]["type"] != "array" {
		errors.push("library.skip_titles must be an array");
	}
	if document["components"]["schemas"]["SourceRepo"]["properties"]["name"]["nullable"] != true {
		errors.push("SourceRepo.name must be nullable");
	}
	if document["components"]["schemas"]["RepoSource"]["properties"]["repoName"]["nullable"] != true {
		errors.push("RepoSource.repoName must be nullable");
	}
	let limit = document["paths"]["/api/v1/sync/changes"]["get"]["parameters"]
		.as_array()
		.expect("sync parameters")
		.iter()
		.find(|parameter| parameter["name"] == "limit")
		.expect("sync limit");
	if limit["schema"].get("maximum").is_some() {
		errors.push("sync limit is capped by the server, not rejected by a maximum constraint");
	}
	assert!(errors.is_empty(), "{errors:?}");
}

fn contract_fixture() -> (Value, Value) {
	let document = serde_json::json!({
		"components": {"schemas": {
			"Base": {
				"type": "object", "required": ["id"],
				"properties": {"id": {"type": "integer"}}
			},
			"Payload": {
				"type": "object",
				"required": ["tuple", "map", "array", "choice", "combined", "nullable", "mode"],
				"properties": {
					"tuple": {"x-tuple": [{"type": "string"}, {"type": "integer", "nullable": true}]},
					"map": {"type": "object", "additionalProperties": {"type": "array", "items": {"type": "integer"}}},
					"array": {"type": "array", "items": {"type": "boolean"}},
					"choice": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
					"combined": {"allOf": [
						{"$ref": "#/components/schemas/Base"},
						{"type": "object", "required": ["extra"], "properties": {"extra": {"type": "boolean"}}}
					]},
					"nullable": {"type": "string", "nullable": true},
					"mode": {"type": "string", "enum": ["a", "b"]}
				}
			}
		}}
	});
	let schema = document["components"]["schemas"]["Payload"].clone();
	(document, schema)
}

#[test]
fn contract_validator_accepts_refs_collections_compositions_and_nullable() {
	let (document, schema) = contract_fixture();
	let value = serde_json::json!({
		"tuple": ["cover", null],
		"map": {"a": [1, 2]},
		"array": [true, false],
		"choice": "text",
		"combined": {"id": 1, "extra": true},
		"nullable": null,
		"mode": "a"
	});
	common::contract::validate_json(&document, &schema, &value).expect("valid contract payload");
}

#[test]
fn contract_validator_rejects_each_contract_violation() {
	let (document, schema) = contract_fixture();
	let valid = serde_json::json!({
		"tuple": ["cover", null],
		"map": {"a": [1, 2]},
		"array": [true, false],
		"choice": "text",
		"combined": {"id": 1, "extra": true},
		"nullable": null,
		"mode": "a"
	});
	let reject = |value: Value, expected: &str| {
		let error = common::contract::validate_json(&document, &schema, &value).expect_err("invalid payload must fail");
		assert!(error.contains(expected), "expected {expected:?} in {error}");
	};
	reject(serde_json::json!({}), "required property");
	let mut value = valid.clone();
	value["tuple"] = serde_json::json!(["cover"]);
	reject(value, "tuple items");
	let mut value = valid.clone();
	value["tuple"] = serde_json::json!(["cover", "bad"]);
	reject(value, "$.tuple[1]");
	let mut value = valid.clone();
	value["map"] = serde_json::json!({"a": ["bad"]});
	reject(value, "$.map.a[0]");
	let mut value = valid.clone();
	value["array"] = serde_json::json!([true, 1]);
	reject(value, "$.array[1]");
	let mut value = valid.clone();
	value["choice"] = serde_json::json!(true);
	reject(value, "oneOf");
	let mut value = valid.clone();
	value["combined"] = serde_json::json!({"id": 1});
	reject(value, "$.combined.extra");
	let mut value = valid.clone();
	value["nullable"] = serde_json::json!(1);
	reject(value, "$.nullable");
	let mut value = valid;
	value["mode"] = serde_json::json!("c");
	reject(value, "enum");
}

#[test]
fn contract_validator_enforces_unicode_length_and_numeric_boundaries() {
	let document = serde_json::json!({});
	let cases = [
		(
			serde_json::json!({"type": "string", "minLength": 2}),
			serde_json::json!("🦀🦀"),
			serde_json::json!("🦀"),
			"at least 2 characters",
		),
		(
			serde_json::json!({"type": "integer", "minimum": 1}),
			serde_json::json!(1),
			serde_json::json!(0),
			"number >= 1",
		),
		(
			serde_json::json!({"type": "integer", "maximum": 3}),
			serde_json::json!(3),
			serde_json::json!(4),
			"number <= 3",
		),
	];
	for (schema, valid, invalid, expected) in cases {
		common::contract::validate_json(&document, &schema, &valid).expect("valid boundary value");
		let error = common::contract::validate_json(&document, &schema, &invalid).expect_err("out-of-range value");
		assert!(error.contains(expected), "expected {expected:?} in {error}");
	}
}

#[test]
fn response_validator_prefers_static_routes_and_skips_ndjson() {
	let mut headers = axum::http::HeaderMap::new();
	headers.insert(header::CONTENT_TYPE, "application/json".parse().expect("JSON header"));
	common::contract::validate_response("PATCH", "/api/v1/categories/reorder", StatusCode::OK, &headers, b"[]")
		.expect("static reorder response");
	let error = common::contract::validate_response(
		"GET",
		"/api/v1/version",
		StatusCode::OK,
		&headers,
		br#"{"version":1}"#,
	)
	.expect_err("real response schema must reject the wrong type");
	assert!(error.contains("expected string"), "{error}");
	headers.insert(header::CONTENT_TYPE, "application/x-ndjson".parse().expect("NDJSON header"));
	common::contract::validate_response(
		"POST",
		"/api/v1/sources/1/search",
		StatusCode::OK,
		&headers,
		b"not JSON",
	)
	.expect("NDJSON is outside the JSON response validator");
}

fn response_fixture() -> Value {
	serde_json::json!({
		"paths": {
			"/api/v1/json": {"get": {"responses": {
				"200": {"description": "JSON", "content": {"application/json": {"schema": {
					"type": "object", "required": ["ok"], "properties": {"ok": {"type": "boolean"}}
				}}}}
			}}},
			"/api/v1/no-content": {"post": {"responses": {
				"204": {"description": "No content"}
			}}},
			"/api/v1/stream": {"get": {"responses": {
				"200": {"description": "NDJSON", "content": {"application/x-ndjson": {"schema": {"type": "string"}}}}
			}}},
			"/api/v1/binary": {"get": {"responses": {
				"200": {"description": "Binary", "content": {"image/png": {"schema": {"type": "string", "format": "binary"}}}}
			}}},
			"/api/v1/ws": {"get": {
				"x-websocket": {"messageFormat": "json-text"},
				"responses": {"101": {"description": "Upgrade"}}
			}}
		}
	})
}

#[test]
fn response_validator_enforces_body_and_media_contract() {
	let document = response_fixture();
	let check = |method: &str, path: &str, status: StatusCode, mime: Option<&str>, body: &[u8]| {
		let mut headers = axum::http::HeaderMap::new();
		if let Some(mime) = mime {
			headers.insert(header::CONTENT_TYPE, mime.parse().expect("content type header"));
		}
		common::contract::validate_response_with_document(&document, method, path, status, &headers, body)
	};
	let reject = |result: Result<(), String>, expected: &str| {
		let error = result.expect_err("contract violation must fail");
		assert!(error.contains(expected), "expected {expected:?} in {error}");
	};
	check("GET", "/api/v1/json", StatusCode::OK, Some("application/json; charset=utf-8"), br#"{"ok":true}"#)
		.expect("valid JSON");
	reject(check("GET", "/api/v1/json", StatusCode::OK, Some("application/json"), b""), "empty application/json");
	reject(check("GET", "/api/v1/json", StatusCode::OK, Some("text/plain"), br#"{"ok":true}"#), "content type text/plain");
	reject(check("GET", "/api/v1/json", StatusCode::OK, None, br#"{"ok":true}"#), "missing response Content-Type");
	reject(check("GET", "/api/v1/json", StatusCode::CREATED, Some("application/json"), br#"{"ok":true}"#), "no OpenAPI response");
	check("POST", "/api/v1/no-content", StatusCode::NO_CONTENT, None, b"").expect("documented no-content");
	reject(check("POST", "/api/v1/no-content", StatusCode::NO_CONTENT, Some("application/json"), b"{}"), "empty response body");
	check("GET", "/api/v1/stream", StatusCode::OK, Some("application/x-ndjson"), b"{}\n").expect("documented NDJSON");
	reject(check("GET", "/api/v1/stream", StatusCode::OK, Some("application/json"), b"{}\n"), "content type application/json");
	check("GET", "/api/v1/binary", StatusCode::OK, Some("image/png"), b"\x89PNG").expect("documented binary");
	reject(check("GET", "/api/v1/binary", StatusCode::OK, Some("application/zip"), b"\x89PNG"), "content type application/zip");
	check("GET", "/api/v1/ws", StatusCode::SWITCHING_PROTOCOLS, Some("text/plain"), b"frame").expect("WebSocket exemption");
	let mut headers = axum::http::HeaderMap::new();
	headers.insert(header::CONTENT_TYPE, "application/json".parse().expect("JSON header"));
	common::contract::validate_response(
		"GET",
		"/api/v1/nope",
		StatusCode::NOT_FOUND,
		&headers,
		br#"{"error":{"code":"not_found","message":"missing"}}"#,
	)
	.expect("unknown-route JSON error");
}

#[tokio::test]
async fn spec_is_served_from_embedded_assets() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let (status, headers, bytes) = request_full(&app, "GET", "/api/openapi.json", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(headers[header::CONTENT_TYPE].to_str().expect("JSON content type").starts_with("application/json"));
	let document: Value = serde_json::from_slice(&bytes).expect("served OpenAPI JSON");
	assert!(document["paths"]["/api/v1/sync/changes"].is_object());
}

#[tokio::test]
async fn docs_assets_are_served_offline() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	for path in ["/api/docs", "/api/docs/"] {
		let (status, headers, bytes) = request_full(&app, "GET", path, None).await;
		assert_eq!(status, StatusCode::OK);
		assert!(headers[header::CONTENT_TYPE].to_str().expect("HTML content type").starts_with("text/html"));
		let body = String::from_utf8(bytes).expect("HTML body");
		assert!(body.contains("SwaggerUIBundle"));
		assert!(body.contains("/api/openapi.json"));
		assert!(body.contains("validatorUrl: null"));
		assert!(!body.contains("https://"));
	}
	let (status, headers, bytes) = request_full(&app, "GET", "/api/docs/swagger-ui-bundle.js", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(headers[header::CONTENT_TYPE].to_str().expect("JS content type").contains("javascript"));
	assert!(bytes.len() > 100_000);
	let (status, headers, _) = request_full(&app, "GET", "/api/docs/swagger-ui.css", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(headers[header::CONTENT_TYPE].to_str().expect("CSS content type").starts_with("text/css"));
	let (status, _, bytes) = request_full(&app, "GET", "/api/docs/LICENSE.swagger-ui", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(String::from_utf8(bytes).expect("Swagger UI license").contains("Apache License"));

	let (status, body) = request(&app, "GET", "/api/docs/missing.js", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
	let (status, body) = request(&app, "GET", "/api/docs/%2e%2e%2fopenapi.json", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn third_party_notices_are_embedded() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	for file in ["swagger-ui-bundle.js.LICENSE.txt", "swagger-ui-standalone-preset.js.LICENSE.txt"] {
		let path = format!("/api/docs/{file}");
		let (status, _, bytes) = request_full(&app, "GET", &path, None).await;
		assert_eq!(status, StatusCode::OK, "{file}");
		let body = String::from_utf8(bytes).expect("third-party notice");
		assert!(body.contains("Permission"), "{file}");
	}
}
