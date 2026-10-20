#![no_std]
//! End-to-end verification source for the canvas host module:
//! - get_page_list produces three Image-content pages: full-canvas fill, copy_image
//!   cropping the left half, and draw_text top-placement semantics
//! - get_page_description returns the ImageRef decoded size and the get_image_data
//!   read-back result
//! - process_page_image / process_cover_image ignore the input and return a fixed
//!   solid-color canvas
//! The asserted values (red/green pixels, sizes) live in crates/wasm/tests/canvas_host.rs.
use aidoku::{
	Chapter, CoverImageProcessor, FilterValue, ImageResponse, Manga, MangaPageResult, Page,
	PageContent, PageContext, PageDescriptionProvider, PageImageProcessor, Result, Source,
	alloc::{String, Vec},
	imports::canvas::{Canvas, CanvasError, Color, Font, ImageRef, Path, Point, Rect},
	prelude::*,
};

/// 8x8 PNG: left half solid green, right half solid yellow (how it was generated: see the note at the top of canvas_host.rs)
const PAIR_PNG: &[u8] = include_bytes!("pair.png");

struct CanvasFixtureSource;

impl Source for CanvasFixtureSource {
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
		manga.key = String::from("canvas-manga");
		manga.title = String::from("Canvas Manga");
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, mut manga: Manga, needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		if needs_details {
			manga.title = String::from("Canvas Manga");
		}
		if needs_chapters {
			let mut chapter = Chapter::default();
			chapter.key = String::from("ch-1");
			chapter.chapter_number = Some(1.0);
			manga.chapters = Some(Vec::from([chapter]));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		// Page 0: 8x8 solid red over the whole canvas
		let mut red = Canvas::new(8.0, 8.0);
		red.fill(&Path::rect(&Rect::new(0.0, 0.0, 8.0, 8.0)), &Color::red());
		let page_red = PageContent::image(red.get_image());

		// Page 1: crop out the left half of the embedded PNG (4x8 solid green)
		let pair = ImageRef::new(PAIR_PNG);
		let mut cropped = Canvas::new(4.0, 8.0);
		cropped.copy_image(&pair, Rect::new(0.0, 0.0, 4.0, 8.0), Rect::new(0.0, 0.0, 4.0, 8.0));
		let page_cropped = PageContent::image(cropped.get_image());

		// Page 2: 32x16 white background + draw_text placement semantics: AidokuRunner's y
		// is the top of the text; a letter without descender (A) hugs y with its ink, a
		// letter with descender (g) sinks the whole line further down by the descender depth
		let mut text = Canvas::new(32.0, 16.0);
		text.fill(&Path::rect(&Rect::new(0.0, 0.0, 32.0, 16.0)), &Color::white());
		text.draw_text(
			"A",
			12.0,
			&Point::new(2.0, 0.0),
			&Font::new("DejaVu Sans").expect("embedded font"),
			&Color::black(),
		);
		text.draw_text(
			"g",
			12.0,
			&Point::new(16.0, 0.0),
			&Font::new("DejaVu Sans").expect("embedded font"),
			&Color::black(),
		);
		let page_text = PageContent::image(text.get_image());

		Ok(Vec::from([
			Page {
				content: page_red,
				..Page::default()
			},
			Page {
				content: page_cropped,
				..Page::default()
			},
			Page {
				content: page_text,
				..Page::default()
			},
		]))
	}
}

impl PageDescriptionProvider for CanvasFixtureSource {
	fn get_page_description(&self, _page: Page) -> Result<String> {
		// ImageRef decoded size + get_image_data read-back + new_font's explicit error
		// surface: a nonexistent family must report InvalidFont, while the named embedded
		// font works as usual
		let pair = ImageRef::new(PAIR_PNG);
		let data_ok = !pair.data().is_empty();
		let bogus = Font::new("definitely-not-a-real-family");
		let bogus_invalid = bogus.as_ref().err() == Some(&CanvasError::InvalidFont);
		let dejavu_ok = Font::new("DejaVu Sans").is_ok();
		Ok(format!(
			"{}x{}:data:{}:bogus_invalid:{}:dejavu:{}",
			pair.width() as i32,
			pair.height() as i32,
			data_ok,
			bogus_invalid,
			dejavu_ok
		))
	}
}

impl PageImageProcessor for CanvasFixtureSource {
	fn process_page_image(&self, _response: ImageResponse, _context: Option<PageContext>) -> Result<ImageRef> {
		let mut canvas = Canvas::new(2.0, 2.0);
		canvas.fill(&Path::rect(&Rect::new(0.0, 0.0, 2.0, 2.0)), &Color::blue());
		Ok(canvas.get_image())
	}
}

impl CoverImageProcessor for CanvasFixtureSource {
	fn process_cover_image(&self, _response: ImageResponse) -> Result<ImageRef> {
		let mut canvas = Canvas::new(3.0, 3.0);
		canvas.fill(&Path::rect(&Rect::new(0.0, 0.0, 3.0, 3.0)), &Color::green());
		Ok(canvas.get_image())
	}
}

register_source!(
	CanvasFixtureSource,
	PageDescriptionProvider,
	PageImageProcessor,
	CoverImageProcessor
);
