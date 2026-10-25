//! Contract tests for the image-processor input (issue #116), aligned with AidokuRunner's
//! `store(value:)` semantics: ciphertext the host cannot decode enters the processor
//! byte-for-byte as-is; a decodable response enters the processor as pixels (data() is a
//! PNG re-encode, width/height/canvas usable — jmcomic's decryption depends on this path).
//! The fixture source lives in tests/fixtures/rawimg-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/rawimg-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_rawimg.wasm
//! over tests/fixtures/rawimg/main.wasm.
//! The embedded plain.png (2x2 RGBA, red-green / blue-white) was generated from
//! hand-written PNG chunks:
//! IHDR(2,2,bitdepth 8,color 6) + IDAT(zlib(each row: filter 0 + pixels)).
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

/// The same key and plaintext as in rawimg-src/src/lib.rs, kept in agreement
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

/// Output of the ciphertext branch: decrypt and build the image → plain.png's 2x2 pixels (the full decrypt → build → re-encode chain is reconciled)
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

/// Output for a decodable input: the 2x2's two rows swapped via the canvas (red-green/blue-white → blue-white/red-green)
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

/// Ciphertext (undecodable by the host) must also enter the processor: after decrypting and building the image, the output is a normal PNG
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

/// For a decodable input the host gives pixels: the fixture checks the size imports and
/// swaps the two rows via the canvas (a miniature of jmcomic's strip reassembly); the
/// output must be the swapped pixels
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

/// The source passes the input image through unchanged (same as jmcomic's old-chapter/gif
/// branch): a decodable one yields a pixel PNG, an undecodable one is handed back as the
/// original bytes (consistent with Aidoku's fallback to the raw data)
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
