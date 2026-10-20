#![no_std]
//! Fixture source for the image-processor contract (issue #116), aligned with
//! AidokuRunner's `store(value:)` semantics:
//! - Ciphertext the host cannot decode (PLAIN_PNG XOR KEY byte by byte) must reach the
//!   processor byte-for-byte as-is; it is XOR-"decrypted", built into an image, and returned
//! - For a decodable input the host has already given pixels: data() is a PNG re-encode
//!   (no longer byte-equal to the original response), and width/height plus canvas strip
//!   reassembly (the same pattern as jmcomic decryption) must work
//! Asserted values are shared with crates/wasm/tests/rawimg_host.rs.
use aidoku::{
	CoverImageProcessor, FilterValue, ImageResponse, Manga, MangaPageResult, PageContext,
	PageImageProcessor, Result, Source,
	alloc::{String, Vec},
	canvas::Rect,
	imports::canvas::{Canvas, ImageRef},
	prelude::*,
};

/// Test key: ciphertext = PLAIN_PNG XOR 0x5A byte by byte (no image magic, so the host's image crate cannot decode it)
pub const KEY: u8 = 0x5A;

/// 2x2 RGBA PNG: red green / blue white (how it was generated: see the note at the top of rawimg_host.rs)
pub const PLAIN_PNG: &[u8] = include_bytes!("plain.png");

struct RawimgFixtureSource;

impl Source for RawimgFixtureSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut manga = Manga::default();
		manga.key = String::from("rawimg-manga");
		manga.title = String::from("Rawimg Manga");
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, mut manga: Manga, _needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		if needs_chapters {
			let mut chapter = aidoku::Chapter::default();
			chapter.key = String::from("ch-1");
			chapter.chapter_number = Some(1.0);
			manga.chapters = Some(Vec::from([chapter]));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: aidoku::Chapter) -> Result<Vec<aidoku::Page>> {
		// Placeholder Url page: the server-side test rewrites it to the mock image-host address after persisting (mirrors the canvas pipeline test)
		Ok(Vec::from([aidoku::Page {
			content: aidoku::PageContent::url(String::from("http://localhost/placeholder")),
			..aidoku::Page::default()
		}]))
	}
}

fn process(response: ImageResponse) -> Result<ImageRef> {
	let data = response.image.data();
	let cipher: Vec<u8> = PLAIN_PNG.iter().map(|b| b ^ KEY).collect();
	if data == cipher {
		// Ciphertext branch: data() must be the response's raw bytes exactly; decrypt and build the image
		let plain: Vec<u8> = data.iter().map(|b| b ^ KEY).collect();
		return Ok(ImageRef::new(&plain));
	}
	// Decodable branch: the input is pixels. The 2x2's two rows are swapped via the canvas
	// (a miniature of jmcomic's reverse-order strip assembly); when width/height cannot get
	// pixels, this fails right here
	if response.image.width() != 2.0 || response.image.height() != 2.0 {
		bail!("decodable input must expose 2x2 pixels");
	}
	let mut canvas = Canvas::new(2.0, 2.0);
	canvas.copy_image(
		&response.image,
		Rect::new(0.0, 1.0, 2.0, 1.0),
		Rect::new(0.0, 0.0, 2.0, 1.0),
	);
	canvas.copy_image(
		&response.image,
		Rect::new(0.0, 0.0, 2.0, 1.0),
		Rect::new(0.0, 1.0, 2.0, 1.0),
	);
	Ok(canvas.get_image())
}

impl PageImageProcessor for RawimgFixtureSource {
	fn process_page_image(&self, response: ImageResponse, context: Option<PageContext>) -> Result<ImageRef> {
		// Pass-through mode: same as jmcomic's old-chapter/gif branch — return the input image as-is
		if context
			.as_ref()
			.and_then(|c| c.get("mode"))
			.is_some_and(|m| m == "passthrough")
		{
			return Ok(response.image);
		}
		process(response)
	}
}

impl CoverImageProcessor for RawimgFixtureSource {
	fn process_cover_image(&self, response: ImageResponse) -> Result<ImageRef> {
		process(response)
	}
}

register_source!(RawimgFixtureSource, PageImageProcessor, CoverImageProcessor);
