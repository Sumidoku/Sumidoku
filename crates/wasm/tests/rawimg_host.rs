//! 图像处理器入参的契约测试（issue #116），对齐 AidokuRunner `store(value:)` 语义：
//! 宿主解不出的密文逐字节原样进处理器；可解码的响应以像素进处理器
//! （data() 是 PNG 重编，width/height/canvas 可用，jmcomic 解密依赖这条路）。
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

/// 密文分支的输出：解密建图 → plain.png 的 2x2 像素（解密→建图→重编全链路对账）
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

/// 可解码入参的输出：2x2 上下两行经画布互换（红绿/蓝白 → 蓝白/红绿）
fn assert_swapped_pixels(png: &[u8], label: &str) {
	let image = ImageReader::new(Cursor::new(png))
		.with_guessed_format()
		.expect("png format")
		.decode()
		.expect("png decode")
		.to_rgba8();
	assert_eq!(image.dimensions(), (2, 2), "{label} dimensions");
	for (x, y, px) in image.enumerate_pixels() {
		let expected = match (x, y) {
			(0, 0) => [0, 0, 255, 255],
			(1, 0) => [255, 255, 255, 255],
			(0, 1) => [255, 0, 0, 255],
			_ => [0, 255, 0, 255],
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

/// 可解码入参宿主给的是像素：fixture 校验尺寸 import 并经画布把两行互换
/// （jmcomic 条带重组的迷你版），输出必须是互换后的像素
#[tokio::test]
async fn processors_receive_decodable_bytes_as_pixels() {
	let (handle, _) = spawn_fixture().await;

	let page_png = handle
		.process_page_image(input(PLAIN_PNG.to_vec()), None)
		.await
		.expect("process page plain");
	assert_swapped_pixels(&page_png, "page plain");

	let cover_png = handle
		.process_cover_image(input(PLAIN_PNG.to_vec()))
		.await
		.expect("process cover plain");
	assert_swapped_pixels(&cover_png, "cover plain");
}

/// 源原样透传输入图（jmcomic 旧章节/gif 分支同款）：可解码的出像素 PNG，
/// 不可解码的原样交还原始字节（与 Aidoku 回退到原始 data 一致）
#[tokio::test]
async fn processor_passthrough_returns_input_unchanged() {
	let (handle, _) = spawn_fixture().await;
	let ctx: aidoku::PageContext = [("mode".to_string(), "passthrough".to_string())]
		.into_iter()
		.collect();

	let page_png = handle
		.process_page_image(input(PLAIN_PNG.to_vec()), Some(ctx.clone()))
		.await
		.expect("passthrough plain");
	assert_plain_pixels(&page_png, "passthrough plain");

	let raw = handle
		.process_page_image(input(cipher()), Some(ctx))
		.await
		.expect("passthrough cipher");
	assert_eq!(raw, cipher(), "undecodable passthrough must return raw bytes");
}
