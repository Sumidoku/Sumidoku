#![no_std]
//! 主页组件字段保真（issue #76 Defect 4）的端到端验证源：get_home 固定返回
//! 六个组件，把 Aidoku 主页契约里 web UI 容易丢的字段全部设上——
//! ImageScroller 的 subtitle/width/height/auto_scroll_interval、BigScroller 的
//! tags/content_rating、Scroller 的 subtitle、MangaList/MangaChapterList 的
//! page_size 与 MangaList 链接内嵌 Manga 的 tags。
//! 断言方在 crates/server/tests/home.rs；web UI 冒烟按字段逐项核对渲染。
use aidoku::{
	Chapter, ContentRating, FilterValue, Home, HomeComponent, HomeComponentValue, HomeLayout, Link,
	LinkValue, Manga, MangaPageResult, MangaWithChapter, Page, Result, Source,
	alloc::{String, Vec, vec},
	prelude::*,
};

struct HomeSource;

fn manga(key: &str, title: &str, tags: &[&str], rating: ContentRating) -> Manga {
	Manga {
		key: String::from(key),
		title: String::from(title),
		cover: Some(String::from("https://aidoku.app/images/icon.png")),
		authors: Some(Vec::from([String::from("Author One")])),
		description: Some(String::from("Fixture description.")),
		tags: Some(tags.iter().map(|t| String::from(*t)).collect()),
		content_rating: rating,
		..Manga::default()
	}
}

fn manga_link(key: &str, title: &str, subtitle: &str, tags: &[&str]) -> Link {
	Link {
		title: String::from(title),
		subtitle: Some(String::from(subtitle)),
		image_url: Some(String::from("https://aidoku.app/images/icon.png")),
		value: Some(LinkValue::Manga(manga(key, title, tags, ContentRating::Safe))),
	}
}

impl Source for HomeSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		Ok(MangaPageResult {
			entries: Vec::new(),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl Home for HomeSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let image_scroller = HomeComponent {
			title: Some(String::from("Banners")),
			subtitle: None,
			value: HomeComponentValue::ImageScroller {
				// 6 张横幅才能在常见桌面宽度下溢出容器，自动滚动才有可滚区间
				links: (1..=6)
					.map(|n| Link {
						title: format!("Banner {n}"),
						subtitle: Some(format!("Banner sub {n}")),
						image_url: Some(String::from("https://aidoku.app/images/icon.png")),
						value: Some(LinkValue::Url(String::from("https://aidoku.app"))),
					})
					.collect(),
				auto_scroll_interval: Some(2.0),
				width: Some(600),
				height: Some(280),
			},
		};
		let big_scroller = HomeComponent {
			title: Some(String::from("Big Picks")),
			subtitle: None,
			value: HomeComponentValue::BigScroller {
				entries: vec![
					manga("big-nsfw", "Big NSFW", &["Action", "Drama"], ContentRating::NSFW),
					manga("big-suggestive", "Big Suggestive", &["Romance"], ContentRating::Suggestive),
					manga("big-safe", "Big Safe", &["Comedy"], ContentRating::Safe),
				],
				auto_scroll_interval: None,
			},
		};
		let scroller = HomeComponent {
			title: Some(String::from("Subbed Scroller")),
			subtitle: None,
			value: HomeComponentValue::Scroller {
				entries: (1..=3)
					.map(|n| manga_link(&format!("sc-{n}"), &format!("Scroll {n}"), &format!("Scroll sub {n}"), &[]))
					.collect(),
				listing: None,
			},
		};
		let paged_list = HomeComponent {
			title: Some(String::from("Paged Manga List")),
			subtitle: None,
			value: HomeComponentValue::MangaList {
				ranking: false,
				page_size: Some(2),
				entries: (1..=5)
					.map(|n| {
						manga_link(
							&format!("pl-{n}"),
							&format!("Paged {n}"),
							&format!("Paged sub {n}"),
							&["Action", "Fantasy"],
						)
					})
					.collect(),
				listing: None,
			},
		};
		let plain_list = HomeComponent {
			title: Some(String::from("Plain Manga List")),
			subtitle: None,
			value: HomeComponentValue::MangaList {
				ranking: false,
				page_size: None,
				entries: (1..=2)
					.map(|n| {
						manga_link(
							&format!("nl-{n}"),
							&format!("Plain {n}"),
							&format!("Plain sub {n}"),
							&["Slice of Life"],
						)
					})
					.collect(),
				listing: None,
			},
		};
		let paged_chapters = HomeComponent {
			title: Some(String::from("Paged Chapters")),
			subtitle: None,
			value: HomeComponentValue::MangaChapterList {
				page_size: Some(2),
				entries: (1..=3)
					.map(|n| MangaWithChapter {
						manga: manga(&format!("cl-{n}"), &format!("Chaptered {n}"), &[], ContentRating::Safe),
						chapter: Chapter {
							key: format!("c{n}"),
							chapter_number: Some(n as f32),
							date_uploaded: Some(1692318525),
							..Chapter::default()
						},
					})
					.collect(),
				listing: None,
			},
		};
		Ok(HomeLayout {
			components: vec![image_scroller, big_scroller, scroller, paged_list, plain_list, paged_chapters],
		})
	}
}

register_source!(HomeSource, Home);
