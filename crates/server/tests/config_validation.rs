//! net.browser_ws 与 net.browser_trawl 互斥：两个都配时启动即报错——
//! 并存没有合并语义，配置错误要在 build_app 暴露而不是运行时行为漂移
mod common;

#[tokio::test]
async fn browser_backends_are_mutually_exclusive() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = common::base_config(&tmp);
	config.net.browser_ws = Some("ws://chromium:3000".into());
	config.net.browser_trawl = Some("http://trawl:8191".into());
	let err = sumidoku_server::build_app(config).await.expect_err("两个浏览器后端都配要拒绝");
	assert!(err.to_string().contains("mutually exclusive"), "报错点名互斥: {err}");
}

#[tokio::test]
async fn trawl_backend_alone_builds() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = common::base_config(&tmp);
	config.net.browser_trawl = Some("http://trawl:8191".into());
	let _app = sumidoku_server::build_app(config).await.expect("单配 trawl 正常启动");
}
