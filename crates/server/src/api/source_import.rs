//! Source import from a local `.aix` upload: the third install path next to repo installs and the
//! directory scan. The package registers through the same register_aix pipeline; a same-key import
//! replaces the installed source with repo-update semantics (breaking-change migration included)

use crate::error::{ApiError, ApiResult};
use crate::model::SourceRow;
use crate::source::repo;
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde_json::{Value, json};
use std::sync::Arc;

/// Upload cap for the multipart body: the package entry caps in aidoku_wasm::package (wasm payload,
/// manifest + settings, icon) bound any valid package; the slack covers zip container and multipart framing
pub const IMPORT_BODY_MAX: usize = (aidoku_wasm::package::WASM_PAYLOAD_MAX
	+ 2 * aidoku_wasm::package::JSON_ENTRY_MAX
	+ aidoku_wasm::package::ICON_ENTRY_MAX
	+ 1024 * 1024) as usize;

pub async fn import_source(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
	let bytes = read_upload(&mut multipart).await?;
	// The manifest key names the landed file (<key>.aix), so the manifest is read and the key
	// validated (against path-unsafe characters) before anything touches disk; the landed package
	// is fully validated by register_aix
	let (bytes, manifest) = tokio::task::spawn_blocking(move || {
		let manifest = crate::source::read_aix_manifest(&bytes);
		(bytes, manifest)
	})
	.await
	.map_err(|e| ApiError::internal(format!("join: {e}")))?;
	let doc = manifest.map_err(ApiError::bad_request)?;
	let key = crate::source::manifest_key(&doc).map_err(ApiError::bad_request)?;
	let old: Option<(i64, i64)> = sqlx::query_as("SELECT id, version FROM source WHERE key = ?")
		.bind(&key)
		.fetch_optional(&state.pool)
		.await?;

	let dir = state.config.sources_dir();
	let path = dir.join(format!("{key}.aix"));
	let tmp = dir.join(format!(".{key}.aix.tmp"));
	let landed = async {
		tokio::fs::write(&tmp, &bytes).await?;
		tokio::fs::rename(&tmp, &path).await
	};
	if let Err(e) = landed.await {
		let _ = tokio::fs::remove_file(&tmp).await;
		return Err(ApiError::from(sqlx::Error::Io(e)));
	}
	if let Err(reason) = crate::source::register_aix(&state, &path).await {
		let _ = tokio::fs::remove_file(&path).await;
		let _ = tokio::fs::remove_file(path.with_extension("png")).await;
		return Err(ApiError::bad_request(reason));
	}

	let source: SourceRow = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source WHERE key = ?",
	)
	.bind(&key)
	.fetch_one(&state.pool)
	.await?;
	// Same as install_from_repo: kick out the running instance so the next call lazily loads the new package
	state.evict_source(source.id).await;
	// A same-key replace gets repo update semantics: cross-breaking imports run the key migration
	// (a fresh install or same-version refresh reports nothing by construction)
	let migration = match old {
		Some((_, old_version)) => {
			crate::source::rekey::migrate_after_update(&state, source.id, old_version, user.id).await?
		}
		None => crate::source::rekey::RekeyReport::default(),
	};
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = super::pins::pinned_map(&state.pool).await?;
	let mut value = super::repo::source_json(&source, &latest, &pinned);
	if let Some(object) = value.as_object_mut() {
		object.insert("migrationRequired".into(), json!(migration.required));
		object.insert("migrationWarnings".into(), json!(migration.warnings));
	}
	Ok(Json(value))
}

/// The upload is exactly one file field; plain form fields are ignored, a second file is rejected
async fn read_upload(multipart: &mut Multipart) -> ApiResult<Vec<u8>> {
	let mut upload: Option<Vec<u8>> = None;
	while let Some(field) = multipart.next_field().await.map_err(multipart_err)? {
		if field.file_name().is_none() {
			continue;
		}
		if upload.is_some() {
			return Err(ApiError::bad_request("a single .aix file is expected"));
		}
		upload = Some(field.bytes().await.map_err(multipart_err)?.to_vec());
	}
	upload.ok_or_else(|| ApiError::bad_request("missing .aix file"))
}

/// Multipart parse failures keep their own status: an over-limit body is a 413, anything else a 400
fn multipart_err(e: axum::extract::multipart::MultipartError) -> ApiError {
	if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
		ApiError::payload_too_large(format!("upload exceeds the {IMPORT_BODY_MAX} byte limit"))
	} else {
		ApiError::bad_request(format!("invalid upload: {e}"))
	}
}
