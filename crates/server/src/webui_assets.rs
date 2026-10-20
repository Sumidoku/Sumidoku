use axum::http::header;
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../webui/build"]
pub struct WebuiAssets;

/// mime is guessed by rust-embed from the extension; path must go through normalize_embed_path first
pub fn get(path: &str) -> Option<(String, Vec<u8>)> {
	let file = WebuiAssets::get(path)?;
	Some((file.metadata.mimetype().to_string(), file.data.into_owned()))
}

pub fn has_index() -> bool {
	WebuiAssets::get("index.html").is_some()
}

/// URL path → embed-relative path; rejects anything with unusual components such as ParentDir (traversal guard)
pub(crate) fn normalize_embed_path(path: &str) -> Option<String> {
	let mut out = std::path::PathBuf::new();
	for component in std::path::Path::new(path.trim_start_matches('/')).components() {
		match component {
			std::path::Component::Normal(part) => out.push(part),
			std::path::Component::CurDir => {}
			_ => return None,
		}
	}
	Some(out.to_string_lossy().into_owned())
}

pub(crate) fn embed_cache_control(path: &str) -> &'static str {
	if path == "index.html" || path == "404.html" {
		"no-cache"
	} else if path.starts_with("assets/") || path.starts_with("_app/") {
		// Build artifacts carry a content hash in the file name, so changing content never reuses a URL
		"public, max-age=31536000, immutable"
	} else {
		"private, max-age=3600"
	}
}

pub(crate) fn embedded_response(path: &str, mime: String, body: Vec<u8>) -> Response {
	(
		[
			(header::CONTENT_TYPE, mime),
			(header::CACHE_CONTROL, embed_cache_control(path).to_string()),
		],
		body,
	)
		.into_response()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalize_rejects_traversal() {
		assert_eq!(normalize_embed_path("/"), Some(String::new()));
		assert_eq!(
			normalize_embed_path("/assets/app.js"),
			Some("assets/app.js".to_string())
		);
		assert_eq!(normalize_embed_path("/a/./b"), Some("a/b".to_string()));
		assert_eq!(normalize_embed_path("//assets//app.js"), Some("assets/app.js".to_string()));
		assert_eq!(normalize_embed_path("/../secret"), None);
		assert_eq!(normalize_embed_path("/a/../../b"), None);
	}

	#[test]
	fn cache_control_tiers() {
		assert_eq!(embed_cache_control("index.html"), "no-cache");
		assert_eq!(embed_cache_control("404.html"), "no-cache");
		assert_eq!(
			embed_cache_control("assets/app-a1b2c3.js"),
			"public, max-age=31536000, immutable"
		);
		assert_eq!(
			embed_cache_control("_app/immutable/start-x1y2.js"),
			"public, max-age=31536000, immutable"
		);
		assert_eq!(embed_cache_control("favicon.png"), "private, max-age=3600");
	}
}
