#![no_std]
//! 图像处理器契约（issue #116）的 fixture 源，对齐 AidokuRunner `store(value:)` 语义：
//! - 宿主解不出的密文（PLAIN_PNG 逐字节异或 KEY）必须逐字节原样到达处理器，
//!   异或"解密"后建图返回；
//! - 可解码入参宿主已给像素：data() 是 PNG 重编（不再逐字节等于原响应），
//!   width/height 与 canvas 条带重组（jmcomic 解密同款模式）必须可用。
//! 断言数值与 crates/wasm/tests/rawimg_host.rs 共享。
use aidoku::{
	CoverImageProcessor, FilterValue, ImageResponse, Manga, MangaPageResult, PageContext,
	PageImageProcessor, Result, Source,
	alloc::{String, Vec},
	canvas::Rect,
	imports::canvas::{Canvas, ImageRef},
	prelude::*,
};

/// 测试密钥：密文 = PLAIN_PNG 逐字节异或 0x5A（无图像 magic，宿主 image crate 解不出）
pub const KEY: u8 = 0x5A;

/// 2x2 RGBA PNG：红 绿 / 蓝 白（生成方式见 rawimg_host.rs 顶部说明）
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
		// 占位 Url 页：服务端测试落库后改写成 mock 图床地址（mirrors canvas 管线测试）
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
		// 密文分支：data() 必须逐字节是响应原始字节，解密后建图
		let plain: Vec<u8> = data.iter().map(|b| b ^ KEY).collect();
		return Ok(ImageRef::new(&plain));
	}
	// 可解码分支：入参是像素。2x2 上下两行经画布互换（jmcomic 逆序拼条带的迷你版），
	// width/height 拿不到像素时这里直接判负
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
		// 透传模式：jmcomic 旧章节/gif 分支同款，输入图原样返回
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
