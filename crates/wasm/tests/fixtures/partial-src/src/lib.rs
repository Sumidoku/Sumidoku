#![no_std]
//! End-to-end verification source for send_partial_result streaming:
//! get_home first pushes a Layout placeholder (empty components A/B), then 3 Component
//! partials (A filled, B filled, C new); the final layout differs from the partials to
//! prove the final state is authoritative; get_search_manga_list pushes 3 Manga partials
//! midway and then returns 2 final entries; get_manga_update pushes snapshots according
//! to the request's needs_details/needs_chapters (details snapshot → details+two-chapters
//! snapshot), with a final state of three chapters, again differing from the partials.
//! Event order and payloads are asserted by crates/wasm/tests/partial_result.rs.
use aidoku::{
	alloc::{String, Vec, vec},
	imports::{defaults::defaults_get, std::send_partial_result, std::sleep},
	prelude::*,
	Chapter, FilterValue, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult,
	Link, LinkValue, Manga, MangaPageResult, Page, Result, Source,
};

struct PartialSource;

// For end-to-end demos: set partial_delay (seconds) to pause between events so progressive
// rendering is visible; tests do not set this key, so zero delay
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

fn chapter(key: &str, number: f32) -> Chapter {
	Chapter {
		key: String::from(key),
		chapter_number: Some(number),
		..Chapter::default()
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

	fn get_manga_update(&self, manga: Manga, needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		let mut updated = manga;
		if needs_details {
			updated.title = String::from("Partial Title");
			updated.description = Some(String::from("fetched in pieces"));
			send_partial_result(&updated);
			delay();
		}
		if needs_chapters {
			updated.chapters = Some(vec![chapter("c1", 1.0), chapter("c2", 2.0)]);
			send_partial_result(&updated);
			delay();
			updated.chapters = Some(vec![chapter("c1", 1.0), chapter("c2", 2.0), chapter("c3", 3.0)]);
		}
		Ok(updated)
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
