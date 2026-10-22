mod common;

use common::{base_config, request};
use sumidoku_server::build_app;

#[tokio::test]
async fn removed_external_opds_routes_return_404_instead_of_the_spa() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	std::fs::create_dir_all(&config.webui_dir).expect("webui dir");
	std::fs::write(config.webui_dir.join("index.html"), "<html>spa</html>").expect("webui index");
	let app = build_app(config).await.expect("build app");

	for path in ["/opds/v1.2", "/opds/v2"] {
		let (status, body) = request(&app, "GET", path, None).await;
		assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{path}");
		assert_eq!(body["error"]["code"], "not_found");
	}
}
