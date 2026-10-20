#![no_std]
//! End-to-end verification source for the 9 DOM mutation host functions of the html
//! module: get_page_description assembles each function's call result and read-back value
//! into a deterministic string, asserted in full by crates/wasm/tests/html_mutation.rs.
use aidoku::{
	alloc::{String, Vec},
	imports::html::{Html, HtmlError},
	prelude::*,
	Chapter, FilterValue, Manga, MangaPageResult, Page, PageDescriptionProvider, Result, Source,
};

struct MutationSource;

impl Source for MutationSource {
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

fn status(result: core::result::Result<(), HtmlError>) -> &'static str {
	match result {
		Ok(()) => "ok",
		Err(HtmlError::InvalidDescriptor) => "err:descriptor",
		Err(HtmlError::InvalidString) => "err:string",
		Err(HtmlError::InvalidHtml) => "err:html",
		Err(HtmlError::InvalidQuery) => "err:query",
		Err(HtmlError::NoResult) => "err:noresult",
		Err(HtmlError::SwiftSoupError) => "err:swiftsoup",
	}
}

fn opt(value: Option<String>) -> String {
	value.unwrap_or_else(|| String::from("none"))
}

impl PageDescriptionProvider for MutationSource {
	fn get_page_description(&self, _page: Page) -> Result<String> {
		let doc = Html::parse(r#"<div id="a" class="x y"><p class="del">ad</p><p>keep</p></div>"#)
			.unwrap();
		let mut el = doc.select_first("#a").unwrap();
		let mut out = String::new();

		out.push_str(&format!("set_attr={},attr={}", status(el.set_attr("data-k", "v1")), opt(el.attr("data-k"))));
		out.push_str(&format!(";remove_attr={},attr={}", status(el.remove_attr("data-k")), opt(el.attr("data-k"))));

		let mut p = doc.select_first("p.del").unwrap();
		let r = p.set_text("clean");
		out.push_str(&format!(";set_text={},text={}", status(r), opt(p.text())));
		let r = p.set_html("<b>bold</b>");
		let has_b = p.select_first("b").is_some();
		out.push_str(&format!(";set_html={},b={},text={}", status(r), has_b as i32, opt(p.text())));

		out.push_str(&format!(";prepend={}", status(el.prepend("<i>head</i>"))));
		out.push_str(&format!(";append={}", status(el.append("<u>tail</u>"))));
		let children = el.children();
		out.push_str(&format!(
			",first={},last={}",
			opt(children.first().and_then(|c| c.tag_name())),
			opt(children.last().and_then(|c| c.tag_name())),
		));

		out.push_str(&format!(";add_class={},class={}", status(el.add_class("z")), opt(el.class_name())));
		out.push_str(&format!(",has_z={}", el.has_class("z") as i32));
		out.push_str(&format!(";remove_class={},class={}", status(el.remove_class("y")), opt(el.class_name())));
		let sel_z = doc.select("#a.z").map(|l| l.size()).unwrap_or(0);
		out.push_str(&format!(",sel_z={sel_z}"));

		let first_p = doc.select_first("p").unwrap();
		first_p.remove();
		let p_left = doc.select("p").map(|l| l.size()).unwrap_or(99);
		out.push_str(&format!(";remove=ok,p={p_left}"));

		doc.select("p").unwrap().remove();
		let p_left = doc.select("p").map(|l| l.size()).unwrap_or(99);
		out.push_str(&format!(";remove_list=ok,p={p_left}"));

		Ok(out)
	}
}

register_source!(MutationSource, PageDescriptionProvider);
