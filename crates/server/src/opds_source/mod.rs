//! The built-in OPDS source (kind = opds, multi-instance): OPDS 1.2 (Atom) catalogs as first-class sources, no wasm payload.
//! manga = a catalog entry (<id> as key, the entry's feed link as url, which refreshes use to fetch upstream); chapters =
//! acquisition entries in the entry feed; pages = OPDS-PSE per-page streaming links when present, otherwise the
//! acquisition archive is downloaded to disk and split into pages with local's ZipCache. basic credentials live in
//! source_setting; register/delete go through the admin surface. OPDS 2.0 (JSON) is not covered — a follow-up.

use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterRow, MangaRow, SOURCE_KIND_OPDS, SourceRow, unix_now};
use crate::state::{AppState, load_defaults};
use aidoku::{Chapter, Manga};
use aidoku_wasm::DefaultValue;
use quick_xml::events::Event;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

const PSE_STREAM_REL: &str = "http://vaemendis.net/opds-pse/stream";
const IMAGE_REL: &str = "http://opds-spec.org/image";
const THUMBNAIL_REL: &str = "http://opds-spec.org/image/thumbnail";
/// rel is prefix-matched: acquisition/open-access, buy, etc. are all file-retrieval links
const ACQUISITION_PREFIX: &str = "http://opds-spec.org/acquisition";
/// Runaway insurance for the degraded traversal: dual caps on BFS depth and total feed count (a Gutenberg-scale catalog must not be crawled whole)
const TRAVERSE_MAX_DEPTH: usize = 2;
const TRAVERSE_MAX_FEEDS: usize = 12;
/// Cap on following series-feed pagination (rel=next): chapter/page listings must be complete, but a broken catalog must not loop forever
const FEED_MAX_PAGES: usize = 20;

/// Per-instance catalog context: base_url is the registered catalog root; basic credentials come from source_setting (empty = public catalog)
pub struct Catalog {
	base_url: String,
	auth: Option<(String, String)>,
}

impl Catalog {
	pub async fn load(state: &AppState, source_id: i64) -> ApiResult<Self> {
		let row: Option<(String, String, Option<String>, bool)> =
			sqlx::query_as("SELECT kind, key, base_url, enabled FROM source WHERE id = ?")
				.bind(source_id)
				.fetch_optional(&state.pool)
				.await?;
		let Some((kind, key, base_url, enabled)) = row else {
			return Err(ApiError::not_found("source not found"));
		};
		if kind != SOURCE_KIND_OPDS {
			return Err(ApiError::bad_request("source is not an opds catalog"));
		}
		if !enabled {
			return Err(ApiError::source_disabled(&key));
		}
		let base_url = base_url
			.filter(|u| !u.is_empty())
			.ok_or_else(|| ApiError::internal("opds source missing catalog url"))?;
		let defaults = load_defaults(&state.pool, source_id).await?;
		let username = string_default(&defaults, "username");
		let password = string_default(&defaults, "password").unwrap_or_default();
		let auth = username.filter(|u| !u.is_empty()).map(|u| (u, password));
		Ok(Self { base_url, auth })
	}

	fn request(&self, state: &AppState, url: &str) -> ApiResult<reqwest::RequestBuilder> {
		let url = reqwest::Url::parse(url).map_err(|_| ApiError::bad_request("invalid catalog url"))?;
		let builder = state
			.http
			.request_with(reqwest::Method::GET, &url, &[], None)
			.map_err(|e| ApiError::bad_gateway(format!("upstream request failed: {e}")))?;
		Ok(match &self.auth {
			Some((user, pass)) => builder.basic_auth(user, Some(pass)),
			None => builder,
		})
	}

	/// Bounded authed GET; non-2xx surfaces the status code (401 is the visible face of wrong credentials)
	pub async fn get_bytes(&self, state: &AppState, url: &str, max: u64) -> ApiResult<(Vec<u8>, Option<String>)> {
		let response = self
			.request(state, url)?
			.send()
			.await
			.map_err(|e| ApiError::bad_gateway(format!("upstream request failed: {e}")))?;
		let status = response.status();
		if !status.is_success() {
			return Err(ApiError::bad_gateway(format!("catalog returned status {status}")));
		}
		crate::source::read_body(response, max).await
	}

	/// href resolves to absolute against the URL of the feed containing it: Komga sends absolute links, generic catalogs may send relative ones
	fn resolve(feed_url: &str, href: &str) -> ApiResult<String> {
		let base = reqwest::Url::parse(feed_url).map_err(|_| ApiError::bad_request("invalid feed url"))?;
		base.join(href)
			.map(|u| u.to_string())
			.map_err(|_| ApiError::bad_gateway(format!("invalid link href {href:?}")))
	}

	async fn get_feed(&self, state: &AppState, url: &str) -> ApiResult<Feed> {
		let max = state.config.net_body_max_mb * 1024 * 1024;
		let (bytes, _) = self.get_bytes(state, url, max).await?;
		parse_feed(&bytes).map_err(|e| ApiError::bad_gateway(format!("invalid catalog feed: {e}")))
	}
}

fn string_default(defaults: &std::collections::HashMap<String, DefaultValue>, key: &str) -> Option<String> {
	match defaults.get(key) {
		Some(DefaultValue::String(value)) => Some(value.clone()),
		_ => None,
	}
}

/// One link of a catalog entry; pse_count appears only on OPDS-PSE stream links (the pse:count attribute)
#[derive(Debug)]
pub struct EntryLink {
	pub rel: String,
	pub type_: String,
	pub href: String,
	pub pse_count: Option<i64>,
}

#[derive(Debug)]
pub struct FeedEntry {
	pub id: String,
	pub title: String,
	pub content: String,
	pub updated: Option<i64>,
	pub authors: Vec<String>,
	pub links: Vec<EntryLink>,
}

impl FeedEntry {
	fn links_by_rel(&self, rel: &str) -> impl Iterator<Item = &EntryLink> {
		self.links.iter().filter(move |l| l.rel == rel)
	}

	/// A book (acquisition entry): has an acquisition or PSE stream link; otherwise it is a drillable series/navigation entry
	pub fn is_book(&self) -> bool {
		self.acquisition().is_some() || self.pse().is_some()
	}

	pub fn acquisition(&self) -> Option<&EntryLink> {
		self.links.iter().find(|l| l.rel.starts_with(ACQUISITION_PREFIX))
	}

	pub fn pse(&self) -> Option<&EntryLink> {
		self.links_by_rel(PSE_STREAM_REL).next()
	}

	/// Drill-down link: points at another feed (the atom type is the feed marker, rel is usually subsection)
	pub fn feed_link(&self) -> Option<&EntryLink> {
		self.links.iter().find(|l| l.type_.starts_with("application/atom+xml"))
	}

	pub fn thumbnail(&self) -> Option<&EntryLink> {
		self.links_by_rel(THUMBNAIL_REL).next()
	}

	pub fn image(&self) -> Option<&EntryLink> {
		self.links_by_rel(IMAGE_REL).next()
	}
}

#[derive(Debug, Default)]
pub struct Feed {
	pub title: String,
	pub links: Vec<EntryLink>,
	pub entries: Vec<FeedEntry>,
}

impl Feed {
	fn next_link(&self) -> Option<&EntryLink> {
		self.links.iter().find(|l| l.rel == "next")
	}
}

/// Element/attribute local name (namespace prefix stripped): Atom's default namespace has no prefix, and pse:* attributes strip down to count
fn local_name(name: &str) -> &str {
	name.rsplit(':').next().unwrap_or(name)
}

/// Event-stream parsing of an Atom feed: serde bindings cannot handle inline markup like <br/> mixed into content,
/// so per event only the small tag set entry/id/title/updated/content/summary/author/name/link is recognized
pub fn parse_feed(bytes: &[u8]) -> Result<Feed, String> {
	let mut reader = quick_xml::Reader::from_reader(bytes);
	reader.config_mut().trim_text(true);
	let mut feed = Feed::default();
	let mut entry: Option<FeedEntry> = None;
	let mut in_author = false;
	// The tag (local name) currently collecting text; nested tags inside content/summary do not switch the target
	let mut text_tag: Option<String> = None;
	let mut text_buf = String::new();
	let mut buf = Vec::new();
	loop {
		match reader.read_event_into(&mut buf) {
			Ok(Event::Start(e)) => {
				match local_name(e.name().as_ref()) {
					"entry" => {
						entry = Some(FeedEntry {
							id: String::new(),
							title: String::new(),
							content: String::new(),
							updated: None,
							authors: Vec::new(),
							links: Vec::new(),
						});
					}
					"link" => push_link(&mut feed, &mut entry, &e)?,
					"author" => in_author = true,
					tag @ ("id" | "title" | "updated" | "content" | "summary" | "name")
						if text_tag.is_none() && (tag != "name" || in_author) =>
					{
						text_tag = Some(tag.to_string());
						text_buf.clear();
					}
					_ => {}
				}
			}
			Ok(Event::Empty(e)) => {
				match local_name(e.name().as_ref()) {
					"link" => push_link(&mut feed, &mut entry, &e)?,
					"br" if text_tag.as_deref() == Some("content") => text_buf.push('\n'),
					_ => {}
				}
			}
			Ok(Event::Text(e)) => {
				if text_tag.is_some() {
					let chunk = e.xml10_content();
					let chunk = chunk.trim();
					if !chunk.is_empty() {
						if !text_buf.is_empty() && !text_buf.ends_with('\n') {
							text_buf.push(' ');
						}
						text_buf.push_str(chunk);
					}
				}
			}
			Ok(Event::CData(e)) => {
				if text_tag.is_some() {
					text_buf.push_str(&e.xml10_content());
				}
			}
			Ok(Event::GeneralRef(e)) => {
				if text_tag.is_some()
					&& let Some(resolved) = resolve_ref(e.as_ref())
				{
					text_buf.push_str(&resolved);
				}
			}
			Ok(Event::End(e)) => {
				match local_name(e.name().as_ref()) {
					"entry" => {
						if let Some(done) = entry.take() {
							feed.entries.push(done);
						}
					}
					"author" => in_author = false,
					tag => {
						if text_tag.as_deref() == Some(tag) {
							commit_text(&mut feed, &mut entry, in_author, tag, &text_buf);
							text_tag = None;
						}
					}
				}
			}
			Ok(Event::Eof) => break,
			Err(e) => return Err(e.to_string()),
			_ => {}
		}
		buf.clear();
	}
	Ok(feed)
}

/// Entity references between text events: the five predefined entities + decimal/hex character references; anything
/// else is dropped (unknown named entities cannot be resolved without the DTD and should not appear in titles anyway)
fn resolve_ref(name: &str) -> Option<String> {
	match name {
		"amp" => return Some("&".into()),
		"lt" => return Some("<".into()),
		"gt" => return Some(">".into()),
		"quot" => return Some("\"".into()),
		"apos" => return Some("'".into()),
		_ => {}
	}
	let code = name
		.strip_prefix("#x")
		.or_else(|| name.strip_prefix("#X"))
		.and_then(|h| u32::from_str_radix(h, 16).ok())
		.or_else(|| name.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))?;
	char::from_u32(code).map(|c| c.to_string())
}

fn push_link(feed: &mut Feed, entry: &mut Option<FeedEntry>, e: &quick_xml::events::BytesStart) -> Result<(), String> {
	let mut link = EntryLink {
		rel: String::new(),
		type_: String::new(),
		href: String::new(),
		pse_count: None,
	};
	for attr in e.attributes() {
		let attr = attr.map_err(|e| e.to_string())?;
		// normalized_value resolves predefined entities: an &amp; inside an href query string must come back as &
		let value = attr
			.normalized_value(quick_xml::XmlVersion::Implicit1_0)
			.map_err(|e| e.to_string())?;
		match local_name(attr.key.as_ref()) {
			"rel" => link.rel = value.into_owned(),
			"type" => link.type_ = value.into_owned(),
			"href" => link.href = value.into_owned(),
			"count" => link.pse_count = value.parse().ok(),
			_ => {}
		}
	}
	match entry {
		Some(entry) => entry.links.push(link),
		None => feed.links.push(link),
	}
	Ok(())
}

fn commit_text(feed: &mut Feed, entry: &mut Option<FeedEntry>, in_author: bool, tag: &str, text: &str) {
	let text = text.trim();
	if let Some(entry) = entry.as_mut() {
		match tag {
			"id" => entry.id = text.to_string(),
			"title" => entry.title = text.to_string(),
			"updated" => entry.updated = parse_rfc3339(text),
			"content" | "summary" => entry.content = text.to_string(),
			"name" if in_author => entry.authors.push(text.to_string()),
			_ => {}
		}
	} else if tag == "title" {
		feed.title = text.to_string();
	}
}

fn parse_rfc3339(text: &str) -> Option<i64> {
	chrono::DateTime::parse_from_rfc3339(text)
		.map(|dt| dt.timestamp())
		.ok()
}

/// The result URL template in an OpenSearch description document (the first Url element's template attribute)
pub fn parse_opensearch_template(bytes: &[u8]) -> Option<String> {
	let mut reader = quick_xml::Reader::from_reader(bytes);
	let mut buf = Vec::new();
	loop {
		match reader.read_event_into(&mut buf) {
			Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
				if local_name(e.name().as_ref()) == "Url" {
					for attr in e.attributes().flatten() {
						if local_name(attr.key.as_ref()) == "template"
							&& let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
						{
							return Some(value.into_owned());
						}
					}
				}
			}
			Ok(Event::Eof) => return None,
			Err(_) => return None,
			_ => {}
		}
		buf.clear();
	}
}

/// Template substitution: {searchTerms} (including the optional {?} form) becomes the encoded query, other optional
/// parameters are blanked as if defaulted. byte_serialize renders a space as + (form semantics), and the query string
/// is unified to %20 (a + in the output can only come from a space, since a literal + was encoded as %2B)
fn opensearch_url(template: &str, query: &str) -> String {
	let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes())
		.collect::<String>()
		.replace('+', "%20");
	let mut out = template
		.replace("{searchTerms?}", &encoded)
		.replace("{searchTerms}", &encoded);
	while let Some(start) = out.find('{') {
		let Some(end) = out[start..].find('}') else { break };
		let param = &out[start + 1..start + end];
		if param.ends_with('?') {
			out.replace_range(start..start + end + 1, "");
		} else {
			break;
		}
	}
	out
}

pub struct SearchOutcome {
	pub entries: Vec<Manga>,
	/// Marks the feed-traversal fallback when the catalog has no OpenSearch: results are just title substring filtering, not a real search
	pub degraded: bool,
}

/// search capability mapping: the OpenSearch description wins; when missing/unusable, fall back to feed traversal marked degraded.
/// An empty query = browse, traversing the root directly (not counted as degraded)
pub async fn search(state: &Arc<AppState>, source_id: i64, query: Option<&str>) -> ApiResult<SearchOutcome> {
	let catalog = Catalog::load(state, source_id).await?;
	let root_url = catalog.base_url.clone();
	let root = catalog.get_feed(state, &root_url).await?;
	let query = query.map(str::trim).filter(|q| !q.is_empty());
	if let Some(query) = query {
		let search_link = root
			.links
			.iter()
			.find(|l| l.rel == "search" && l.type_.contains("opensearchdescription"));
		if let Some(link) = search_link {
			let descriptor_url = Catalog::resolve(&root_url, &link.href)?;
			if let Ok((descriptor, _)) = catalog.get_bytes(state, &descriptor_url, 1024 * 1024).await
				&& let Some(template) = parse_opensearch_template(&descriptor)
			{
				let search_url = opensearch_url(&Catalog::resolve(&root_url, &template)?, query);
				let feed = catalog.get_feed(state, &search_url).await?;
				let entries = feed_entries_to_manga(&feed, &search_url);
				return Ok(SearchOutcome {
					entries,
					degraded: false,
				});
			}
		}
		let entries = traverse(&catalog, state, &root, &root_url, Some(query)).await?;
		return Ok(SearchOutcome {
			entries,
			degraded: true,
		});
	}
	let entries = traverse(&catalog, state, &root, &root_url, None).await?;
	Ok(SearchOutcome {
		entries,
		degraded: false,
	})
}

/// Degraded traversal: BFS drill-down (dual depth/total caps), drillable entries become manga candidates; the root
/// feed's entries are shelves (Komga's "All series" etc.), so candidates are only collected from feeds at depth>=1
async fn traverse(
	catalog: &Catalog,
	state: &AppState,
	root: &Feed,
	root_url: &str,
	query: Option<&str>,
) -> ApiResult<Vec<Manga>> {
	let needle = query.map(str::to_lowercase);
	let mut out: Vec<Manga> = Vec::new();
	let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
	let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
	let mut queue: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::new();
	visited.insert(root_url.to_string());
	let mut fetched = 1usize;
	let mut pending: Vec<(String, usize)> = Vec::new();
	for entry in &root.entries {
		if let Some(link) = entry.feed_link()
			&& let Ok(url) = Catalog::resolve(root_url, &link.href)
			&& visited.insert(url.clone())
		{
			pending.push((url, 1));
		}
	}
	if let Some(next) = root.next_link()
		&& let Ok(url) = Catalog::resolve(root_url, &next.href)
		&& visited.insert(url.clone())
	{
		pending.push((url, 0));
	}
	queue.extend(pending);
	while let Some((feed_url, depth)) = queue.pop_front() {
		if fetched >= TRAVERSE_MAX_FEEDS {
			break;
		}
		let feed = match catalog.get_feed(state, &feed_url).await {
			Ok(feed) => feed,
			Err(e) => {
				tracing::warn!(feed_url, "opds traverse skip unreadable feed: {}", e.message());
				continue;
			}
		};
		fetched += 1;
		for manga in feed_entries_to_manga(&feed, &feed_url) {
			if needle.as_ref().is_some_and(|n| !manga.title.to_lowercase().contains(n)) {
				continue;
			}
			if seen.insert(manga.key.clone()) {
				out.push(manga);
			}
		}
		if depth < TRAVERSE_MAX_DEPTH {
			for entry in &feed.entries {
				if let Some(link) = entry.feed_link()
					&& let Ok(url) = Catalog::resolve(&feed_url, &link.href)
					&& visited.insert(url.clone())
				{
					queue.push_back((url, depth + 1));
				}
			}
		}
		if let Some(next) = feed.next_link()
			&& let Ok(url) = Catalog::resolve(&feed_url, &next.href)
			&& visited.insert(url.clone())
		{
			queue.push_back((url, depth));
		}
	}
	Ok(out)
}

/// Drillable entry → source-side Manga: key is the entry <id>, url is the drill-down feed link (the only handle for
/// fetching upstream); book-shaped entries (acquisition links straight in the list) do not enter the manga model — they are chapter material of a series
fn feed_entries_to_manga(feed: &Feed, feed_url: &str) -> Vec<Manga> {
	feed.entries
		.iter()
		.filter(|entry| !entry.is_book())
		.filter_map(|entry| {
			let link = entry.feed_link()?;
			let url = Catalog::resolve(feed_url, &link.href).ok()?;
			let cover = entry
				.image()
				.or_else(|| entry.thumbnail())
				.and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
			let description = (!entry.content.is_empty()).then(|| entry.content.clone());
			Some(Manga {
				key: entry.id.clone(),
				title: entry.title.clone(),
				url: Some(url),
				cover,
				description,
				authors: (!entry.authors.is_empty()).then(|| entry.authors.clone()),
				..Manga::default()
			})
		})
		.collect()
}

/// The opds counterpart of fetching details + chapters from the source: pulls the entry feed and maps acquisition
/// entries to chapters; pagination (rel=next) is followed to completion. initial provides key and url (search results/library rows both carry them)
pub async fn manga_update(state: &Arc<AppState>, source_id: i64, initial: Manga) -> ApiResult<Manga> {
	let catalog = Catalog::load(state, source_id).await?;
	let feed_url = initial
		.url
		.clone()
		.filter(|u| !u.is_empty())
		.ok_or_else(|| ApiError::bad_request("opds manga requires a catalog url"))?;
	let feed = fetch_feed_pages(&catalog, state, &feed_url, FEED_MAX_PAGES).await?;
	let chapters: Vec<Chapter> = feed
		.entries
		.iter()
		.filter_map(|entry| entry_to_chapter(entry, &feed_url))
		.collect();
	let cover = feed
		.entries
		.iter()
		.find_map(|entry| {
			entry
				.image()
				.or_else(|| entry.thumbnail())
				.and_then(|l| Catalog::resolve(&feed_url, &l.href).ok())
		})
		.or(initial.cover.clone());
	let mut authors: Vec<String> = Vec::new();
	for entry in &feed.entries {
		for author in &entry.authors {
			if !authors.contains(author) {
				authors.push(author.clone());
			}
		}
	}
	Ok(Manga {
		key: initial.key.clone(),
		title: if feed.title.is_empty() { initial.title.clone() } else { feed.title },
		url: Some(feed_url),
		cover,
		authors: if authors.is_empty() { initial.authors.clone() } else { Some(authors) },
		description: initial.description.clone(),
		chapters: Some(chapters),
		..Manga::default()
	})
}

/// The feed's first page plus rel=next pagination merged into the complete entry list (the feed-level title comes only from the first page)
async fn fetch_feed_pages(catalog: &Catalog, state: &AppState, feed_url: &str, max_pages: usize) -> ApiResult<Feed> {
	let first = catalog.get_feed(state, feed_url).await?;
	let mut next = first.next_link().map(|l| l.href.clone());
	let mut merged = Feed {
		title: first.title,
		links: Vec::new(),
		entries: first.entries,
	};
	let mut pages = 1;
	while let Some(href) = next {
		if pages >= max_pages {
			tracing::warn!(feed_url, "opds feed pagination capped");
			break;
		}
		let url = Catalog::resolve(feed_url, &href)?;
		let feed = catalog.get_feed(state, &url).await?;
		next = feed.next_link().map(|l| l.href.clone());
		merged.entries.extend(feed.entries);
		pages += 1;
	}
	Ok(merged)
}

/// Book entry → chapter: url prefers the acquisition file link (both the CBZ fallback and downloads point at it),
/// falling back to the PSE stream base when there is no file link; number parses from the title's digits (volume-level chapters are a source feature, so an unparseable number stays empty)
fn entry_to_chapter(entry: &FeedEntry, feed_url: &str) -> Option<Chapter> {
	if !entry.is_book() {
		return None;
	}
	let url = entry
		.acquisition()
		.or_else(|| entry.pse())
		.and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
	let thumbnail = entry
		.thumbnail()
		.or_else(|| entry.image())
		.and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
	Some(Chapter {
		key: entry.id.clone(),
		title: Some(entry.title.clone()),
		chapter_number: crate::local::chapter_number::parse_chapter_number(&entry.title),
		volume_number: None,
		date_uploaded: entry.updated,
		scanlators: None,
		url,
		language: None,
		thumbnail,
		locked: false,
	})
}

/// load_pages' opds arm: PSE per-page links expand into Url pages (PSE page numbers are 0-based; Komga's endpoint
/// implements +1), otherwise the acquisition archive is downloaded to disk and ZipCache lists image entries into OpdsZip pages
pub async fn chapter_page_rows(state: &Arc<AppState>, manga: &MangaRow, chapter: &ChapterRow) -> ApiResult<Vec<Value>> {
	let catalog = Catalog::load(state, manga.source_id).await?;
	let feed_url = manga
		.url
		.clone()
		.filter(|u| !u.is_empty())
		.ok_or_else(|| ApiError::bad_request("opds manga requires a catalog url"))?;
	let feed = fetch_feed_pages(&catalog, state, &feed_url, FEED_MAX_PAGES).await?;
	let entry = feed
		.entries
		.iter()
		.find(|e| e.id == chapter.key)
		.ok_or_else(|| ApiError::not_found("opds chapter not found in catalog feed"))?;
	if let Some(pse) = entry.pse()
		&& let Some(count) = pse.pse_count
		&& count > 0
	{
		// Replace the placeholder before parsing: {pageNumber}'s braces would be percent-encoded by Url::join
		if !pse.href.contains("{pageNumber}") {
			return Err(ApiError::bad_gateway("opds page streaming link missing {pageNumber}"));
		}
		let mut urls = Vec::with_capacity(count as usize);
		for i in 0..count {
			urls.push(page_url_json(Catalog::resolve(
				&feed_url,
				&pse.href.replace("{pageNumber}", &i.to_string()),
			)?));
		}
		return Ok(urls);
	}
	let acquisition = entry
		.acquisition()
		.ok_or_else(|| ApiError::bad_request("opds entry has neither page streaming nor acquisition link"))?;
	let href = Catalog::resolve(&feed_url, &acquisition.href)?;
	let path = ensure_cbz(state, &catalog, chapter.id, &href).await?;
	let names = list_cbz_pages(state, &path).await?;
	Ok(names.iter().map(|name| page_opds_zip_json(name)).collect())
}

/// Page-image bytes (OpdsZip variant): when the on-disk cache is missing, re-fetches the feed to find the acquisition link, self-healing
pub async fn read_cbz_page(state: &Arc<AppState>, manga: &MangaRow, chapter: &ChapterRow, entry_name: &str) -> ApiResult<(Vec<u8>, String)> {
	let path = cbz_path(state, chapter.id);
	if tokio::fs::metadata(&path).await.is_err() {
		let catalog = Catalog::load(state, manga.source_id).await?;
		let feed_url = manga
			.url
			.clone()
			.filter(|u| !u.is_empty())
			.ok_or_else(|| ApiError::bad_request("opds manga requires a catalog url"))?;
		let feed = fetch_feed_pages(&catalog, state, &feed_url, FEED_MAX_PAGES).await?;
		let entry = feed
			.entries
			.iter()
			.find(|e| e.id == chapter.key)
			.ok_or_else(|| ApiError::not_found("opds chapter not found in catalog feed"))?;
		let acquisition = entry
			.acquisition()
			.ok_or_else(|| ApiError::bad_request("opds entry has no acquisition link"))?;
		let href = Catalog::resolve(&feed_url, &acquisition.href)?;
		ensure_cbz(state, &catalog, chapter.id, &href).await?;
	}
	let zip = state.local_zip.clone();
	let read_path = path.clone();
	let entry = entry_name.to_string();
	let bytes = tokio::task::spawn_blocking(move || zip.read_entry(&read_path, &entry))
		.await
		.map_err(|e| ApiError::internal(format!("read zip entry failed: {e}")))?
		.map_err(|e| {
			if e.kind() == std::io::ErrorKind::NotFound {
				ApiError::not_found("page not found")
			} else {
				ApiError::bad_gateway(format!("read opds archive failed: {e}"))
			}
		})?;
	Ok((bytes, crate::local::image_mime(entry_name).to_string()))
}

/// On-disk path of an acquisition archive: named by chapter_id; the caller deletes the old file when page rows are rewritten
pub fn cbz_path(state: &AppState, chapter_id: i64) -> PathBuf {
	state.config.opds_cbz_dir().join(chapter_id.to_string())
}

async fn ensure_cbz(state: &AppState, catalog: &Catalog, chapter_id: i64, href: &str) -> ApiResult<PathBuf> {
	let path = cbz_path(state, chapter_id);
	if tokio::fs::metadata(&path).await.is_ok() {
		return Ok(path);
	}
	tokio::fs::create_dir_all(state.config.opds_cbz_dir())
		.await
		.map_err(|e| ApiError::internal(format!("create opds cbz dir failed: {e}")))?;
	let max = state.config.net_zip_body_max_mb * 1024 * 1024;
	let (bytes, _) = catalog.get_bytes(state, href, max).await?;
	let tmp = state.config.opds_cbz_dir().join(format!(".{chapter_id}.tmp"));
	tokio::fs::write(&tmp, &bytes)
		.await
		.map_err(|e| ApiError::internal(format!("write opds cbz failed: {e}")))?;
	tokio::fs::rename(&tmp, &path)
		.await
		.map_err(|e| ApiError::internal(format!("store opds cbz failed: {e}")))?;
	Ok(path)
}

async fn list_cbz_pages(state: &Arc<AppState>, path: &std::path::Path) -> ApiResult<Vec<String>> {
	let zip = state.local_zip.clone();
	let path = path.to_path_buf();
	let names = tokio::task::spawn_blocking(move || zip.entry_names(&path))
		.await
		.map_err(|e| ApiError::internal(format!("list zip entries failed: {e}")))?
		.map_err(|e| ApiError::bad_gateway(format!("invalid opds archive: {e}")))?;
	let mut pages: Vec<String> = names
		.into_iter()
		.filter(|n| crate::local::is_image_entry(n))
		.collect();
	pages.sort_by(|a, b| crate::local::natural_sort::compare(a, b));
	Ok(pages)
}

fn page_url_json(url: String) -> Value {
	serde_json::json!({
		"content": { "Url": [url, null] },
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	})
}

/// The OpdsZip variant of page-content JSON: the value is the page entry name inside the archive, whose path derives from chapter_id
fn page_opds_zip_json(entry: &str) -> Value {
	serde_json::json!({
		"content": { "OpdsZip": entry },
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	})
}

/// Derives an instance slug from name: lowercase alphanumeric runs joined with -; no usable character at all means no key can be built
fn slugify(name: &str) -> Option<String> {
	let mut slug = String::new();
	let mut dash = false;
	for c in name.chars().flat_map(char::to_lowercase) {
		if c.is_ascii_alphanumeric() {
			if dash && !slug.is_empty() {
				slug.push('-');
			}
			slug.push(c);
			dash = false;
		} else {
			dash = true;
		}
	}
	(!slug.is_empty()).then_some(slug)
}

/// Instance registration: key takes opds.<slug>, collisions append a -2/-3 counter (Aidoku precedent for same-named sources coexisting);
/// basic credentials land in source_setting (managed on the admin surface); an empty username = public catalog, no credentials seeded
pub async fn register_instance(
	state: &Arc<AppState>,
	name: &str,
	catalog_url: &str,
	username: Option<&str>,
	password: Option<&str>,
) -> ApiResult<SourceRow> {
	let name = name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	let parsed = reqwest::Url::parse(catalog_url.trim())
		.map_err(|_| ApiError::bad_request("invalid catalog url"))?;
	if !matches!(parsed.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("catalog url must be http or https"));
	}
	let base_url = parsed.to_string().trim_end_matches('/').to_string();
	let slug = slugify(name).ok_or_else(|| ApiError::bad_request("name has no usable characters"))?;
	let mut key = format!("opds.{slug}");
	let mut counter = 2;
	loop {
		let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM source WHERE key = ?")
			.bind(&key)
			.fetch_optional(&state.pool)
			.await?;
		if exists.is_none() {
			break;
		}
		key = format!("opds.{slug}-{counter}");
		counter += 1;
	}
	let now = unix_now();
	let result = sqlx::query(
		"INSERT INTO source (key, name, kind, version, languages, content_rating, base_url,
		 breaking_change_version, wasm_path, enabled, installed_at, updated_at)
		 VALUES (?, ?, ?, 0, '[]', 0, ?, 0, '', 1, ?, ?)",
	)
	.bind(&key)
	.bind(name)
	.bind(SOURCE_KIND_OPDS)
	.bind(&base_url)
	.bind(now)
	.bind(now)
	.execute(&state.pool)
	.await?;
	let source_id = result.last_insert_rowid();
	if let Some(username) = username.filter(|u| !u.is_empty()) {
		for (setting_key, value) in [("username", username), ("password", password.unwrap_or_default())] {
			let raw = serde_json::to_string(&DefaultValue::String(value.to_string()))?;
			sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)")
				.bind(source_id)
				.bind(setting_key)
				.bind(raw)
				.execute(&state.pool)
				.await?;
		}
	}
	let row: Option<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source WHERE id = ?",
	)
	.bind(source_id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::internal("registered opds source missing"))
}

#[cfg(test)]
mod tests {
	use super::*;

	const SERIES_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns">
  <title>Alpha Series</title>
  <id>series-alpha</id>
  <link rel="next" type="application/atom+xml;profile=opds-catalog;kind=acquisition" href="/opds/series/alpha?page=2"/>
  <entry>
    <title>Vol. 1</title>
    <id>book-1</id>
    <updated>2024-03-01T10:00:00Z</updated>
    <content>cbz - 12 MB<br/><br/>first volume</content>
    <author><name>Author One</name></author>
    <link rel="http://opds-spec.org/image/thumbnail" type="image/jpeg" href="/opds/books/book-1/thumbnail/small"/>
    <link rel="http://opds-spec.org/image" type="image/jpeg" href="/opds/books/book-1/thumbnail"/>
    <link rel="http://opds-spec.org/acquisition" type="application/zip" href="/opds/books/book-1/file/vol1.cbz"/>
    <link rel="http://vaemendis.net/opds-pse/stream" type="image/jpeg" href="/opds/books/book-1/pages/{pageNumber}" pse:count="17" pse:lastRead="3"/>
  </entry>
</feed>"#;

	#[test]
	fn parse_feed_reads_entry_links_and_pse_count() {
		let feed = parse_feed(SERIES_FEED.as_bytes()).expect("parse");
		assert_eq!(feed.title, "Alpha Series");
		assert_eq!(feed.next_link().map(|l| l.href.as_str()), Some("/opds/series/alpha?page=2"));
		assert_eq!(feed.entries.len(), 1);
		let entry = &feed.entries[0];
		assert_eq!(entry.id, "book-1");
		assert_eq!(entry.title, "Vol. 1");
		assert_eq!(entry.content, "cbz - 12 MB\n\nfirst volume");
		assert_eq!(entry.updated, Some(1709287200));
		assert_eq!(entry.authors, vec!["Author One".to_string()]);
		assert!(entry.is_book());
		let pse = entry.pse().expect("pse link");
		assert_eq!(pse.pse_count, Some(17));
		assert_eq!(pse.href, "/opds/books/book-1/pages/{pageNumber}");
		assert_eq!(
			entry.acquisition().map(|l| l.href.as_str()),
			Some("/opds/books/book-1/file/vol1.cbz")
		);
		assert_eq!(
			entry.thumbnail().map(|l| l.href.as_str()),
			Some("/opds/books/book-1/thumbnail/small")
		);
	}

	#[test]
	fn parse_feed_navigation_entry_has_feed_link() {
		let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Catalog</title>
  <link rel="search" type="application/opensearchdescription+xml" href="/opds/search"/>
  <entry>
    <title>Alpha Series</title>
    <id>series-alpha</id>
    <updated>2024-01-01T00:00:00Z</updated>
    <content></content>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog;kind=navigation" href="/opds/series/alpha"/>
  </entry>
</feed>"#;
		let feed = parse_feed(xml.as_bytes()).expect("parse");
		assert_eq!(feed.entries.len(), 1);
		let entry = &feed.entries[0];
		assert!(!entry.is_book());
		assert_eq!(
			entry.feed_link().map(|l| l.href.as_str()),
			Some("/opds/series/alpha")
		);
		let search = feed.links.iter().find(|l| l.rel == "search").expect("search link");
		assert_eq!(search.href, "/opds/search");
	}

	#[test]
	fn opensearch_template_extraction_and_substitution() {
		let descriptor = r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Search</ShortName>
  <Description>Search for series</Description>
  <Url type="application/atom+xml;profile=opds-catalog;kind=acquisition" template="http://host/opds/series?search={searchTerms}"/>
</OpenSearchDescription>"#;
		let template = parse_opensearch_template(descriptor.as_bytes()).expect("template");
		assert_eq!(
			opensearch_url(&template, "attack on titan"),
			"http://host/opds/series?search=attack%20on%20titan"
		);
		assert_eq!(opensearch_url("http://h/s?q={searchTerms?}&count={count?}", "a b"), "http://h/s?q=a%20b&count=");
	}

	#[test]
	fn slugify_rules() {
		assert_eq!(slugify("My Komga!"), Some("my-komga".to_string()));
		assert_eq!(slugify("  spaces  inside  "), Some("spaces-inside".to_string()));
		assert_eq!(slugify("demo2"), Some("demo2".to_string()));
		assert_eq!(slugify("!!!"), None);
	}
}
