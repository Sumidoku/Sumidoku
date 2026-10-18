#![no_std]
//! send_partial_result 流式加载的端到端验证源：
//! get_home 先推 Layout 占位（A/B 空组件），再推 3 个 Component partial
//!（A 填充、B 填充、C 新增），终态布局与 partial 不同以证明终态权威；
//! get_search_manga_list 中途推 3 条 Manga partial 再回 2 条终态。
//! 事件顺序与载荷由 crates/wasm/tests/partial_result.rs 断言。
use aidoku::{
	alloc::{String, Vec, vec},
	imports::{defaults::defaults_get, std::send_partial_result, std::sleep},
	prelude::*,
	Chapter, FilterValue, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult,
	Link, LinkValue, Manga, MangaPageResult, Page, Result, Source,
};

struct PartialSource;

// 端到端演示用：设置 partial_delay（秒）后每个事件之间停一下，渐进渲染才看得见；
// 测试不设这个键，零延迟
fn delay() {
	let secs = defaults_get::<i32>("partial_delay").unwrap_or(0);
	if secs > 0 {
		sleep(secs);
	}
}

fn manga(key: &str, title: &str) -> Manga {
	Manga {
		key: String::from(key),
		title: String::from(title),
		cover: Some(String::from("https://example.com/cover.jpg")),
		..Manga::default()
	}
}

fn scroller(title: &str, link_titles: &[&str]) -> HomeComponent {
	HomeComponent {
		title: Some(String::from(title)),
		subtitle: None,
		value: HomeComponentValue::Scroller {
			entries: link_titles
				.iter()
				.map(|t| Link {
					title: String::from(*t),
					subtitle: None,
					image_url: Some(String::from("https://example.com/cover.jpg")),
					value: Some(LinkValue::Manga(manga(*t, *t))),
				})
				.collect(),
			listing: None,
		},
	}
}

impl Source for PartialSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		for n in 1..=3 {
			let key = format!("partial-{n}");
			send_partial_result(&manga(&key, &key));
			delay();
		}
		Ok(MangaPageResult {
			entries: vec![manga("final-1", "Final One"), manga("final-2", "Final Two")],
			has_next_page: true,
		})
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl Home for PartialSource {
	fn get_home(&self) -> Result<HomeLayout> {
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: vec![scroller("Partial A", &[]), scroller("Partial B", &[])],
		}));
		delay();
		send_partial_result(&HomePartialResult::Component(scroller("Partial A", &["a-1"])));
		delay();
		send_partial_result(&HomePartialResult::Component(scroller("Partial B", &["b-1"])));
		delay();
		send_partial_result(&HomePartialResult::Component(scroller("Partial C", &["c-1"])));
		delay();
		Ok(HomeLayout {
			components: vec![
				scroller("Partial A", &["a-1", "a-2"]),
				scroller("Partial B", &["b-1"]),
				scroller("Partial C", &["c-1"]),
			],
		})
	}
}

register_source!(PartialSource, Home);
