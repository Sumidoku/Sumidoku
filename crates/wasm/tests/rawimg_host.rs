//! 图像处理器拿原始响应字节的契约测试（issue #116）：
//! 声明处理器的源，宿主跳过自己的解码，data() 必须是响应原始字节——
//! 密文（不可解码）逐字节原样交给处理器，明文 PNG 同样原样给。
//! fixture 源在 tests/fixtures/rawimg-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/rawimg-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_rawimg.wasm
//! 复制为 tests/fixtures/rawimg/main.wasm。
//! 内嵌 plain.png（2x2 RGBA 红绿/蓝白）是手写 PNG 块生成的：
//! IHDR(2,2,bitdepth 8,color 6) + IDAT(zlib(每行 filter 0 + 像素))。
use aidoku_wasm::{
	DirectPolicy, NoopCookieSink, NoopDefaultsSink, ProcessImageInput, SourceConfig, SourceHandle,
	StdoutLogger,
};
use image::ImageReader;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// 与 rawimg-src/src/lib.rs 对账的同一密钥与明文
const KEY: u8 = 0x5A;
const PLAIN_PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0,
	0, 0, 114, 182, 13, 36, 0, 0, 0, 18, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 12,
	129, 52, 24, 0, 0, 73, 200, 9, 247, 249, 171, 182, 13, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
	130,
];

fn cipher() -> Vec<u8> {
	PLAIN_PNG.iter().map(|b| b ^ KEY).collect()
}

async fn spawn_fixture() -> (SourceHandle, aidoku_wasm::SourceFeatures) {
	let config = SourceConfig {
		key: "en.rawimg".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rawimg/main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	};
	SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source")
}

fn input(bytes: Vec<u8>) -> ProcessImageInput {
	ProcessImageInput {
		bytes,
		url: "http://localhost/img".into(),
		status: 200,
		response_headers: Vec::new(),
		request_headers: Vec::new(),
	}
}

/// 处理器输出的 PNG 必须就是 plain.png 的 2x2 像素（解密→建图→重编全链路对账）
fn assert_plain_pixels(png: &[u8], label: &str) {
	let image = ImageReader::new(Cursor::new(png))
		.with_guessed_format()
		.expect("png format")
		.decode()
		.expect("png decode")
		.to_rgba8();
	assert_eq!(image.dimensions(), (2, 2), "{label} dimensions");
	for (x, y, px) in image.enumerate_pixels() {
		let expected = match (x, y) {
			(0, 0) => [255, 0, 0, 255],
			(1, 0) => [0, 255, 0, 255],
			(0, 1) => [0, 0, 255, 255],
			_ => [255, 255, 255, 255],
		};
		assert_eq!(px.0, expected, "{label} pixel ({x},{y})");
	}
}

/// 密文（宿主解不出）也必须进处理器：解密建图后输出正常 PNG
#[tokio::test]
async fn processors_receive_undecodable_bytes_raw() {
	let (handle, features) = spawn_fixture().await;
	assert!(features.page_image_processor);
	assert!(features.cover_image_processor);

	let page_png = handle
		.process_page_image(input(cipher()), None)
		.await
		.expect("process page cipher");
	assert_plain_pixels(&page_png, "page cipher");

	let cover_png = handle
		.process_cover_image(input(cipher()))
		.await
		.expect("process cover cipher");
	assert_plain_pixels(&cover_png, "cover cipher");
}

/// 对照组：可解码的响应同样给原始字节（fixture 逐字节比对，重编过的 PNG 对不上）
#[tokio::test]
async fn processors_receive_decodable_bytes_raw_too() {
	let (handle, _) = spawn_fixture().await;

	let page_png = handle
		.process_page_image(input(PLAIN_PNG.to_vec()), None)
		.await
		.expect("process page plain");
	assert_plain_pixels(&page_png, "page plain");

	let cover_png = handle
		.process_cover_image(input(PLAIN_PNG.to_vec()))
		.await
		.expect("process cover plain");
	assert_plain_pixels(&cover_png, "cover plain");
}
