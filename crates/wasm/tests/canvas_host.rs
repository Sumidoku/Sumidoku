//! canvas host 模块与 process_*_image 的端到端测试。
//! fixture 源在 tests/fixtures/canvas-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/canvas-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_canvas.wasm
//! 复制为 tests/fixtures/canvas/main.wasm。
//! 源内嵌的 pair.png（8x8 RGBA 左半绿右半黄）是手写 PNG 块生成的：
//! IHDR(8,8,bitdepth 8,color 6) + IDAT(zlib(每行 filter 0 + 左 4 像素绿右 4 像素黄))。
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

async fn spawn_fixture() -> (SourceHandle, aidoku_wasm::SourceFeatures) {
	let config = SourceConfig {
		key: "en.canvas".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/canvas/main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		browser_ws: None,
	};
	SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source")
}

fn decode_png(bytes: &[u8]) -> image::RgbaImage {
	ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.expect("png format")
		.decode()
		.expect("png decode")
		.to_rgba8()
}

fn assert_solid(image: &image::RgbaImage, expected: [u8; 4], label: &str) {
	for (x, y, px) in image.enumerate_pixels() {
		assert_eq!(px.0, expected, "{label} pixel ({x},{y})");
	}
}

/// 1x1 透明 PNG，处理器入参用
const ONE_PX_PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0,
	31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1, 13, 10, 45, 180,
	0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn process_input() -> ProcessImageInput {
	ProcessImageInput {
		bytes: ONE_PX_PNG.to_vec(),
		url: "http://localhost/p.png".into(),
		status: 200,
		response_headers: Vec::new(),
		request_headers: Vec::new(),
	}
}

#[tokio::test]
async fn canvas_draw_crop_and_text_pages() {
	let (handle, features) = spawn_fixture().await;
	assert!(features.page_image_processor);
	assert!(features.cover_image_processor);

	let pages = handle
		.get_page_list(aidoku::Manga::default(), aidoku::Chapter::default())
		.await
		.expect("page list");
	assert_eq!(pages.len(), 3);

	let mut images = Vec::new();
	for page in &pages {
		// ImageRefPriv 字段 pub(crate) 不外露，rid 走序列化形状 {"Image": rid} 取
		let content = serde_json::to_value(&page.content).expect("serialize content");
		let Some(rid) = content.get("Image").and_then(|v| v.as_i64()) else {
			panic!("page content must be an image: {content}");
		};
		let png = handle.take_page_image(rid as i32).await.expect("page image png");
		images.push(decode_png(&png));
	}

	assert_eq!(images[0].dimensions(), (8, 8));
	assert_solid(&images[0], [255, 0, 0, 255], "red page");

	assert_eq!(images[1].dimensions(), (4, 8));
	assert_solid(&images[1], [0, 255, 0, 255], "cropped page");

	assert_eq!(images[2].dimensions(), (32, 16));
	// 白底黑字：只要存在非白像素即 draw_text 真的画出了字形
	let drawn = images[2].pixels().any(|px| px.0 != [255, 255, 255, 255]);
	assert!(drawn, "draw_text must paint glyphs");
}

#[tokio::test]
async fn image_ref_smoke_via_page_description() {
	let (handle, _) = spawn_fixture().await;
	let out = handle
		.get_page_description(aidoku::Page::default())
		.await
		.expect("page description");
	assert_eq!(out, "8x8:data:true:font:true");
}

#[tokio::test]
async fn processors_return_fixed_canvas_images() {
	let (handle, _) = spawn_fixture().await;

	let page_png = handle
		.process_page_image(process_input(), None)
		.await
		.expect("process page image");
	let page = decode_png(&page_png);
	assert_eq!(page.dimensions(), (2, 2));
	assert_solid(&page, [0, 0, 255, 255], "processed page");

	let cover_png = handle
		.process_cover_image(process_input())
		.await
		.expect("process cover image");
	let cover = decode_png(&cover_png);
	assert_eq!(cover.dimensions(), (3, 3));
	assert_solid(&cover, [0, 255, 0, 255], "processed cover");
}
