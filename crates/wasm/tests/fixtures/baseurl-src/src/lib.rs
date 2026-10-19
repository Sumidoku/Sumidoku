#![no_std]
//! 动态 base URL 链路的端到端验证源（issue #136）：
//! get_base_url 返回 defaults 里 dynamic_url 的值——测试经 PUT settings 注入，
//! 模拟源更新后解析结果变化；缺省回退固定值。值为 "fail" 时 bail，
//! 验证导出失败回退包内静态 URL。断言方在 crates/server/tests/source_base_url.rs。
use aidoku::{
	BaseUrlProvider, Chapter, FilterValue, Manga, MangaPageResult, Page, Result, Source,
	alloc::{String, Vec},
	imports::defaults::defaults_get,
	prelude::*,
};

struct BaseUrlSource;

impl Source for BaseUrlSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		bail!("not used")
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl BaseUrlProvider for BaseUrlSource {
	fn get_base_url(&self) -> Result<String> {
		match defaults_get::<String>("dynamic_url").as_deref() {
			Some("fail") => bail!("base url export failed"),
			Some(url) => Ok(String::from(url)),
			None => Ok(String::from("https://dyn-v1.example")),
		}
	}
}

register_source!(BaseUrlSource, BaseUrlProvider);
