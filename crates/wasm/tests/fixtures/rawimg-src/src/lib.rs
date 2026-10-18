#![no_std]
//! 图像处理器契约（issue #116）的 fixture 源：
//! 声明 Page/CoverImageProcessor 时宿主必须交给处理器原始响应字节——
//! 处理器逐字节比对 data() 与期望字节，不符即报错；
//! 密文（PLAIN_PNG 逐字节异或 KEY，不可解码）异或"解密"后建图返回，
//! 明文 PNG 对照组直接建图返回（契约不因可解码而改变）。
//! 断言数值与 crates/wasm/tests/rawimg_host.rs 共享。
use aidoku::{
	CoverImageProcessor, FilterValue, ImageResponse, Manga, MangaPageResult, PageContext,
	PageImageProcessor, Result, Source,
	alloc::{String, Vec},
	imports::canvas::ImageRef,
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

/// data() 必须是响应原始字节：密文原样给（宿主跳过自己解不了的码），明文也原样给
fn process(response: ImageResponse) -> Result<ImageRef> {
	let data = response.image.data();
	if data == PLAIN_PNG {
		return Ok(ImageRef::new(&data));
	}
	let cipher: Vec<u8> = PLAIN_PNG.iter().map(|b| b ^ KEY).collect();
	if data == cipher {
		let plain: Vec<u8> = data.iter().map(|b| b ^ KEY).collect();
		return Ok(ImageRef::new(&plain));
	}
	bail!("processor data() is not the raw response body ({} bytes)", data.len())
}

impl PageImageProcessor for RawimgFixtureSource {
	fn process_page_image(&self, response: ImageResponse, _context: Option<PageContext>) -> Result<ImageRef> {
		process(response)
	}
}

impl CoverImageProcessor for RawimgFixtureSource {
	fn process_cover_image(&self, response: ImageResponse) -> Result<ImageRef> {
		process(response)
	}
}

register_source!(RawimgFixtureSource, PageImageProcessor, CoverImageProcessor);
