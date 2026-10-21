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
