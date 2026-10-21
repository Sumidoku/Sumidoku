use crate::error::ApiError;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/api-docs"]
struct ApiDocsAssets;

fn asset(path: &str) -> Response {
	let Some(path) = crate::webui_assets::normalize_embed_path(path) else {
		return ApiError::not_found("route not found").into_response();
	};
	match ApiDocsAssets::get(&path) {
		Some(file) => crate::webui_assets::embedded_response(
			&path,
			file.metadata.mimetype().to_string(),
			file.data.into_owned(),
		),
		None => ApiError::not_found("route not found").into_response(),
	}
}

pub async fn openapi() -> Response {
	asset("openapi.json")
}

pub async fn index() -> Response {
	asset("index.html")
}

pub async fn file(Path(path): Path<String>) -> Response {
	asset(&path)
}
