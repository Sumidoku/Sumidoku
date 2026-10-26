//! Internal (built-in, non-wasm) source registration: one admin endpoint for every installable special
//! source — OPDS catalogs and local directory instances — dispatched on `type`, so a future built-in
//! kind is one more variant here plus its registration function

use super::repo::source_json;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RegisterInternalRequest {
	/// OPDS catalog instance: name + catalog URL + optional basic credentials
	/// (rename_all on the variant: container-level rename_all covers only variant names, not fields)
	#[serde(rename = "opds", rename_all = "camelCase")]
	Opds {
		name: String,
		url: String,
		username: Option<String>,
		password: Option<String>,
	},
	/// Local directory instance: name + one or more existing root directories + optional oneshots marker
	/// and the same per-instance overrides the settings panel exposes (exclusion substrings, scan/analysis
	/// toggles); absent fields fall back to the server config layer, same as an unset source setting
	#[serde(rename = "local", rename_all = "camelCase")]
	Local {
		name: String,
		paths: Vec<String>,
		oneshots_dir: Option<String>,
		exclusions: Option<Vec<String>>,
		force_modified_time: Option<bool>,
		analyze_pages: Option<bool>,
		analyze_metadata: Option<bool>,
	},
}

pub async fn register_internal_source(
	State(state): State<Arc<AppState>>,
	Json(body): Json<RegisterInternalRequest>,
) -> ApiResult<Json<Value>> {
	let row = match body {
		RegisterInternalRequest::Opds {
			name,
			url,
			username,
			password,
		} => {
			crate::opds_source::register_instance(
				&state,
				&name,
				&url,
				username.as_deref(),
				password.as_deref(),
			)
			.await?
		}
		RegisterInternalRequest::Local {
			name,
			paths,
			oneshots_dir,
			exclusions,
			force_modified_time,
			analyze_pages,
			analyze_metadata,
		} => {
			crate::local::register_instance(
				&state,
				&name,
				&paths,
				oneshots_dir.as_deref(),
				exclusions.as_deref(),
				force_modified_time,
				analyze_pages,
				analyze_metadata,
			)
			.await?
		}
	};
	// A new instance is in no repo cache and never pinned at registration: latest/pinned are empty maps
	Ok(Json(source_json(&row, &Default::default(), &Default::default())))
}
