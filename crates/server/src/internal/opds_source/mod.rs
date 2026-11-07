//! The built-in OPDS source (kind = opds, multi-instance): OPDS catalogs as first-class sources, no wasm payload.
//! Both wire formats are accepted, detected per response: OPDS 1.2 (Atom) and OPDS 2.0 (JSON) — the Content-Type
//! decides, with a JSON body sniff as fallback for mislabeled servers; both parse into the same Feed model.
//! manga = a feed-linked publication (its feed link as url, which refreshes use to fetch upstream), or an
//! acquisition-only publication as a one-chapter manga; navigation stays a directory link and is never a work;
//! chapters = book entries in the series feed; pages = OPDS-PSE
//! per-page streaming (1.2) or the publication manifest's readingOrder (2.0) when present, otherwise the
//! acquisition archive is downloaded to disk and split into pages with local's ZipCache. basic credentials live in
//! source_setting; register/delete go through the admin surface.

use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterRow, MangaRow, SOURCE_KIND_OPDS, SourceRow, unix_now};
use crate::source::slugify;
use crate::state::{AppState, load_defaults};
use aidoku::{Chapter, Manga};
use aidoku_wasm::DefaultValue;
use base64::Engine;
use quick_xml::events::Event;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

const PSE_STREAM_REL: &str = "http://vaemendis.net/opds-pse/stream";
const IMAGE_REL: &str = "http://opds-spec.org/image";
const THUMBNAIL_REL: &str = "http://opds-spec.org/image/thumbnail";
const THUMBNAIL_REL_ALT: &str = "http://opds-spec.org/image-thumbnail";
const WORK_KEY_PREFIX: &str = "opds:work:";

#[derive(sqlx::FromRow)]
struct PersistedWorkChapter {
	key: String,
	title: Option<String>,
	number: Option<f64>,
	volume: Option<f64>,
	language: Option<String>,
	thumbnail: Option<String>,
	url: Option<String>,
	date_uploaded: Option<i64>,
}
/// OPDS 2.0 feed media type: doubles as the drill marker on navigation items and series publications
const FEED_TYPE_V2: &str = "application/opds+json";
/// rel is prefix-matched: acquisition/open-access, buy, etc. are all file-retrieval links
const ACQUISITION_PREFIX: &str = "http://opds-spec.org/acquisition";
/// Runaway insurance for the degraded traversal: dual caps on BFS depth and total feed count (a Gutenberg-scale catalog must not be crawled whole)
const TRAVERSE_MAX_DEPTH: usize = 2;
const TRAVERSE_MAX_FEEDS: usize = 12;
/// Cap on following series-feed pagination (rel=next): chapter/page listings must be complete, but a broken catalog must not loop forever
const FEED_MAX_PAGES: usize = 20;
/// Bound catalog-supplied page counts and reading orders before allocating page rows.
const PAGE_MAX_COUNT: usize = 10_000;

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
			Some((user, pass)) if self.same_origin(&url) => builder.basic_auth(user, Some(pass)),
			_ => builder,
		})
	}

	pub fn base_url(&self) -> &str {
		&self.base_url
	}

	/// Credentials belong to the catalog's origin only: image proxy URLs are caller-chosen and feed hrefs may point
	/// at any host, so sending them elsewhere would hand the catalog password to a third party
	fn same_origin(&self, url: &reqwest::Url) -> bool {
		reqwest::Url::parse(&self.base_url).is_ok_and(|base| base.origin() == url.origin())
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
	pub fn resolve(feed_url: &str, href: &str) -> ApiResult<String> {
		let base = reqwest::Url::parse(feed_url).map_err(|_| ApiError::bad_request("invalid feed url"))?;
		base.join(href)
			.map(|u| u.to_string())
			.map_err(|_| ApiError::bad_gateway(format!("invalid link href {href:?}")))
	}

	pub async fn get_feed(&self, state: &AppState, url: &str) -> ApiResult<Feed> {
		let max = state.config.net_body_max_mb * 1024 * 1024;
		let (bytes, mime) = self.get_bytes(state, url, max).await?;
		if is_json_feed(mime.as_deref(), &bytes) {
			return parse_feed_json(&bytes).map_err(|e| ApiError::bad_gateway(format!("invalid catalog feed: {e}")));
		}
		parse_feed(&bytes).map_err(|e| ApiError::bad_gateway(format!("invalid catalog feed: {e}")))
	}
}

/// Per-response wire format detection: any +json Content-Type picks the OPDS 2.0 parser; a mislabeled body still
/// parses as JSON when it opens with an object — an Atom body always opens with markup
fn is_json_feed(mime: Option<&str>, bytes: &[u8]) -> bool {
	if let Some(mime) = mime {
		let base = mime.split(';').next().unwrap_or_default().trim();
		if base == "application/json" || base.ends_with("+json") {
			return true;
		}
	}
	let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
	body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
}

fn string_default(defaults: &std::collections::HashMap<String, DefaultValue>, key: &str) -> Option<String> {
	match defaults.get(key) {
		Some(DefaultValue::String(value)) => Some(value.clone()),
		_ => None,
	}
}

/// One link of a catalog entry; pse_count appears only on OPDS-PSE stream links (the pse:count attribute)
#[derive(Debug, Clone)]
pub struct EntryLink {
	pub rel: String,
	pub type_: String,
	pub href: String,
	pub pse_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FeedEntry {
	pub id: String,
	pub title: String,
	pub content: String,
	pub updated: Option<i64>,
	pub authors: Vec<String>,
	pub artists: Vec<String>,
	pub tags: Vec<String>,
	pub links: Vec<EntryLink>,
}

impl FeedEntry {
	fn links_by_rel(&self, rel: &str) -> impl Iterator<Item = &EntryLink> {
		self.links.iter().filter(move |l| l.rel == rel)
	}

	/// A book (acquisition entry): has an acquisition or PSE stream link, or (OPDS 2.0) a publication manifest;
	/// otherwise it is a drillable series/navigation entry
	pub fn is_book(&self) -> bool {
		!self.is_atom_navigation() && (self.acquisition().is_some() || self.pse().is_some() || self.manifest().is_some())
	}

	fn is_atom_navigation(&self) -> bool {
		self.links.iter().any(|link| {
			let nav_rel = link.rel == "subsection"
				|| link.rel.starts_with("http://opds-spec.org/sort/")
				|| link.rel == "http://opds-spec.org/group";
			nav_rel && link.type_.starts_with("application/atom+xml") && !link.type_.to_ascii_lowercase().contains("type=entry")
		})
	}

	fn is_actionable_publication(&self) -> bool {
		self.is_book() || (!self.is_atom_navigation() && self.feed_link().is_some())
	}

	pub fn acquisition(&self) -> Option<&EntryLink> {
		self.links.iter().find(|l| l.rel.starts_with(ACQUISITION_PREFIX))
	}

	pub fn pse(&self) -> Option<&EntryLink> {
		self.links_by_rel(PSE_STREAM_REL).next()
	}

	/// Drill-down link: points at another feed (atom type for 1.2, opds+json for 2.0; rel is usually subsection or self)
	pub fn feed_link(&self) -> Option<&EntryLink> {
		self.links
			.iter()
			.find(|l| (l.type_.starts_with("application/atom+xml") && !l.type_.to_ascii_lowercase().contains("type=entry")) || l.type_.starts_with(FEED_TYPE_V2))
	}

	/// The publication document of a book entry (OPDS 2.0): the self link carries the manifest with readingOrder
	/// (Komga types it divina/webpub+json, the spec opds-publication+json). Feed-typed self links are series
	/// drill-downs, not manifests — those stay with feed_link()
	pub fn manifest(&self) -> Option<&EntryLink> {
		self.links
			.iter()
			.find(|l| l.rel == "self" && l.type_.ends_with("+json") && !l.type_.starts_with(FEED_TYPE_V2))
	}

	pub fn thumbnail(&self) -> Option<&EntryLink> {
		self.links_by_rel(THUMBNAIL_REL)
			.next()
			.or_else(|| self.links_by_rel(THUMBNAIL_REL_ALT).next())
	}

	pub fn image(&self) -> Option<&EntryLink> {
		self.links_by_rel(IMAGE_REL).next()
	}
}

#[derive(Debug, Clone, Default)]
pub struct FeedGroup {
	pub title: String,
	pub navigation: Vec<FeedEntry>,
	pub publications: Vec<FeedEntry>,
}

#[derive(Debug, Default)]
pub struct Feed {
	pub title: String,
	pub links: Vec<EntryLink>,
	pub entries: Vec<FeedEntry>,
	pub navigation: Vec<FeedEntry>,
	pub publications: Vec<FeedEntry>,
	pub groups: Vec<FeedGroup>,
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
/// so per event only entry/id/title/updated/content/summary/author/name/link/category is recognized
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
							artists: Vec::new(),
							tags: Vec::new(),
							links: Vec::new(),
						});
					}
					"link" => push_link(&mut feed, &mut entry, &e)?,
					"category" => push_category(&mut entry, &e)?,
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
					"category" => push_category(&mut entry, &e)?,
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
							if done.is_atom_navigation() {
								feed.navigation.push(done.clone());
							} else {
								feed.publications.push(done.clone());
							}
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

fn push_category(entry: &mut Option<FeedEntry>, e: &quick_xml::events::BytesStart) -> Result<(), String> {
	let Some(entry) = entry.as_mut() else { return Ok(()) };
	let mut term = String::new();
	let mut label = String::new();
	for attr in e.attributes() {
		let attr = attr.map_err(|e| e.to_string())?;
		let value = attr
			.normalized_value(quick_xml::XmlVersion::Implicit1_0)
			.map_err(|e| e.to_string())?
			.into_owned();
		match local_name(attr.key.as_ref()) {
			"term" => term = value,
			"label" => label = value,
			_ => {}
		}
	}
	let category = if label.is_empty() { term } else { label };
	push_unique(&mut entry.tags, category);
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

fn push_unique(values: &mut Vec<String>, value: String) {
	if !value.is_empty() && !values.contains(&value) {
		values.push(value);
	}
}

fn parse_rfc3339(text: &str) -> Option<i64> {
	chrono::DateTime::parse_from_rfc3339(text)
		.map(|dt| dt.timestamp())
		.ok()
}

/// OPDS 2.0 (JSON) feed → the same Feed model the 1.2 parser fills. Top-level navigation and publications and each
/// group's navigation/publications remain distinct for browse while all records join entries for search and updates.
pub fn parse_feed_json(bytes: &[u8]) -> Result<Feed, String> {
	let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
	let mut feed = Feed::default();
	if let Some(title) = value.get("metadata").and_then(|m| m.get("title")) {
		feed.title = json_text(title);
	}
	if let Some(links) = value.get("links").and_then(Value::as_array) {
		for link in links {
			push_json_link(&mut feed.links, link);
		}
	}
	push_section_entries(&mut feed, &value, None);
	if let Some(groups) = value.get("groups").and_then(Value::as_array) {
		for group in groups {
			let mut parsed = FeedGroup {
				title: group.get("metadata").and_then(|m| m.get("title")).map(json_text).unwrap_or_default(),
				..FeedGroup::default()
			};
			push_section_entries(&mut feed, group, Some(&mut parsed));
			feed.groups.push(parsed);
		}
	}
	Ok(feed)
}

/// The navigation + publications pair one feed section (top level or a group) contributes
fn push_section_entries(feed: &mut Feed, owner: &Value, mut group: Option<&mut FeedGroup>) {
	if let Some(navigation) = owner.get("navigation").and_then(Value::as_array) {
		for item in navigation {
			let entry = navigation_entry(item);
			feed.entries.push(entry.clone());
			if let Some(group) = group.as_deref_mut() {
				group.navigation.push(entry);
			} else {
				feed.navigation.push(entry);
			}
		}
	}
	if let Some(publications) = owner.get("publications").and_then(Value::as_array) {
		for publication in publications {
			let entry = publication_entry(publication);
			feed.entries.push(entry.clone());
			if let Some(group) = group.as_deref_mut() {
				group.publications.push(entry);
			} else {
				feed.publications.push(entry);
			}
		}
	}
}

/// RWPM text fields accept a plain string or a language map; a map contributes its first value
fn json_text(value: &Value) -> String {
	match value {
		Value::String(text) => text.clone(),
		Value::Object(map) => map.values().find_map(Value::as_str).unwrap_or_default().to_string(),
		_ => String::new(),
	}
}

/// Contributor fields (author, artist, …) accept a name string, a {name} object, or an array of either
fn json_contributors(value: Option<&Value>) -> Vec<String> {
	fn one(value: &Value) -> Option<String> {
		match value {
			Value::String(name) => Some(name.clone()),
			Value::Object(map) => map.get("name").map(json_text).filter(|name| !name.is_empty()),
			_ => None,
		}
	}
	match value {
		Some(Value::Array(items)) => items.iter().filter_map(one).collect(),
		Some(single) => one(single).into_iter().collect(),
		None => Vec::new(),
	}
}

fn json_terms(value: Option<&Value>) -> Vec<String> {
	fn append(value: &Value, out: &mut Vec<String>) {
		match value {
			Value::Array(items) => items.iter().for_each(|item| append(item, out)),
			Value::String(text) => push_unique(out, text.clone()),
			Value::Object(map) => {
				for key in ["name", "term", "label"] {
					if let Some(text) = map.get(key).map(json_text).filter(|text| !text.is_empty()) {
						push_unique(out, text);
						return;
					}
				}
			}
			_ => {}
		}
	}
	let mut out = Vec::new();
	if let Some(value) = value {
		append(value, &mut out);
	}
	out
}

fn role_names(metadata: Option<&Value>) -> (Vec<String>, Vec<String>) {
	let Some(metadata) = metadata else { return (Vec::new(), Vec::new()) };
	let mut authors = json_contributors(metadata.get("author"));
	let mut artists = Vec::new();
	for key in ["artist", "illustrator", "letterer", "penciller", "colorist", "inker"] {
		for name in json_contributors(metadata.get(key)) {
			push_unique(&mut artists, name);
		}
	}
	if let Some(contributors) = metadata.get("contributor") {
		let items = contributors.as_array().map(Vec::as_slice).unwrap_or(std::slice::from_ref(contributors));
		for contributor in items {
			let names = json_contributors(Some(contributor.get("name").unwrap_or(contributor)));
			let Some(roles) = contributor.get("role") else { continue };
			let role_values = roles
				.as_array()
				.map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
				.unwrap_or_else(|| roles.as_str().into_iter().collect());
			for role in role_values {
				let role = role
					.rsplit(['/', '#'])
					.next()
					.unwrap_or(role)
					.to_ascii_lowercase();
				let target = match role.as_str() {
					"aut" | "author" | "cre" | "creator" => Some(&mut authors),
					"art" | "artist" | "ill" | "illustrator" | "let" | "letterer" | "pnc" | "penciller"
					| "clr" | "colorist" | "ink" | "inker" => Some(&mut artists),
					_ => None,
				};
				if let Some(target) = target {
					for name in &names {
						push_unique(target, name.clone());
					}
				}
			}
		}
	}
	(authors, artists)
}

/// rel may be a string or an array (RWPM); one EntryLink per rel keeps rel lookups working
fn push_json_link(links: &mut Vec<EntryLink>, value: &Value) {
	let Some(href) = value.get("href").and_then(Value::as_str) else {
		return;
	};
	let type_ = value.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
	match value.get("rel") {
		Some(Value::Array(rels)) => {
			for rel in rels.iter().filter_map(Value::as_str) {
				links.push(EntryLink {
					rel: rel.to_string(),
					type_: type_.clone(),
					href: href.to_string(),
					pse_count: None,
				});
			}
		}
		Some(Value::String(rel)) => links.push(EntryLink {
			rel: rel.clone(),
			type_,
			href: href.to_string(),
			pse_count: None,
		}),
		_ => links.push(EntryLink {
			rel: String::new(),
			type_,
			href: href.to_string(),
			pse_count: None,
		}),
	}
}

/// A navigation item is a drill entry: href is the only identity it carries, so it doubles as the key; an untyped
/// item still drills (navigation only ever points at feeds)
fn navigation_entry(item: &Value) -> FeedEntry {
	let href = item.get("href").and_then(Value::as_str).unwrap_or_default().to_string();
	let type_ = item.get("type").and_then(Value::as_str).unwrap_or(FEED_TYPE_V2).to_string();
	FeedEntry {
		id: href.clone(),
		title: item.get("title").map(json_text).unwrap_or_default(),
		content: String::new(),
		updated: None,
		authors: Vec::new(),
		artists: Vec::new(),
		tags: Vec::new(),
		links: vec![EntryLink {
			rel: "subsection".to_string(),
			type_,
			href,
			pse_count: None,
		}],
	}
}

/// A publication becomes one entry: metadata.identifier/title/author/subject/modified/description onto the 1.2 fields,
/// acquisition rels and self links stay links (a feed-typed self link is the series drill-down, any other json
/// self link the book's manifest), images[0] is the cover and the rest thumbnails
fn publication_entry(publication: &Value) -> FeedEntry {
	let metadata = publication.get("metadata");
	let (authors, artists) = role_names(metadata);
	let mut entry = FeedEntry {
		id: metadata.and_then(|m| m.get("identifier")).map(json_text).unwrap_or_default(),
		title: metadata.and_then(|m| m.get("title")).map(json_text).unwrap_or_default(),
		content: metadata.and_then(|m| m.get("description")).map(json_text).unwrap_or_default(),
		updated: metadata
			.and_then(|m| m.get("modified"))
			.and_then(Value::as_str)
			.and_then(parse_rfc3339),
		authors,
		artists,
		tags: json_terms(metadata.and_then(|m| m.get("subject"))),
		links: Vec::new(),
	};
	if let Some(links) = publication.get("links").and_then(Value::as_array) {
		for link in links {
			push_json_link(&mut entry.links, link);
		}
	}
	// identifier is only RECOMMENDED; the self link is the next stable handle
	if entry.id.is_empty()
		&& let Some(self_link) = entry.links.iter().find(|l| l.rel == "self")
	{
		entry.id = self_link.href.clone();
	}
	if let Some(images) = publication.get("images").and_then(Value::as_array) {
		for (index, image) in images.iter().enumerate() {
			let Some(href) = image.get("href").and_then(Value::as_str) else {
				continue;
			};
			let rel = if index == 0 { IMAGE_REL } else { THUMBNAIL_REL };
			entry.links.push(EntryLink {
				rel: rel.to_string(),
				type_: image.get("type").and_then(Value::as_str).unwrap_or_default().to_string(),
				href: href.to_string(),
				pse_count: None,
			});
		}
	}
	entry
}

/// The page list of a publication document: readingOrder's hrefs in order (resolved against the manifest URL by
/// the caller); the rest of the manifest is ignored — pages are all load_pages needs from it
fn parse_reading_order(bytes: &[u8]) -> Result<Vec<String>, String> {
	let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
	let order = value
		.get("readingOrder")
		.and_then(Value::as_array)
		.ok_or("publication has no readingOrder")?;
	Ok(order
		.iter()
		.filter_map(|item| item.get("href").and_then(Value::as_str).map(str::to_string))
		.collect())
}

fn manifest_page_rows(manifest_url: &str, reading_order: &[String]) -> ApiResult<Vec<Value>> {
	if reading_order.is_empty() || reading_order.len() > PAGE_MAX_COUNT {
		return Err(ApiError::bad_gateway("opds manifest readingOrder is empty or exceeds page limit"));
	}
	reading_order.iter().map(|href| Ok(page_url_json(Catalog::resolve(manifest_url, href)?))).collect()
}

/// v2 search template expansion: {?query} (RFC 6570 form-style) becomes ?query=<encoded>; {searchTerms} is honored
/// too for servers reusing the OpenSearch variable; any other expression drops as server-side defaulted
fn search_template_url(template: &str, query: &str) -> String {
	let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes())
		.collect::<String>()
		.replace('+', "%20");
	let mut out = template
		.replace("{?query}", &format!("?query={encoded}"))
		.replace("{query}", &encoded)
		.replace("{searchTerms?}", &encoded)
		.replace("{searchTerms}", &encoded);
	while let Some(start) = out.find('{') {
		let Some(end) = out[start..].find('}') else { break };
		out.replace_range(start..start + end + 1, "");
	}
	out
}

/// The result URL template in an OpenSearch description document, selected by result media type
pub fn parse_opensearch_template(bytes: &[u8], preferred_type: Option<&str>) -> Option<String> {
	let mut reader = quick_xml::Reader::from_reader(bytes);
	let mut buf = Vec::new();
	let mut fallback = None;
	let preferred_type = preferred_type.map(|value| value.split(';').next().unwrap_or_default().trim());
	loop {
		match reader.read_event_into(&mut buf) {
			Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
				if local_name(e.name().as_ref()) == "Url" {
					let mut template = None;
					let mut type_ = String::new();
					for attr in e.attributes().flatten() {
						if let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
							match local_name(attr.key.as_ref()) {
								"template" => template = Some(value.into_owned()),
								"type" => type_ = value.into_owned(),
								_ => {}
							}
						}
					}
					if let Some(template) = template {
						let base_type = type_.split(';').next().unwrap_or_default().trim();
						if preferred_type.is_none_or(|t| base_type.eq_ignore_ascii_case(t)) {
							return Some(template);
						}
						fallback.get_or_insert(template);
					}
				}
			}
			Ok(Event::Eof) => return if preferred_type.is_none() { fallback } else { None },
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
	pub has_next_page: bool,
}

pub struct BrowseGroup {
	pub title: String,
	pub entries: Vec<Manga>,
	pub navigation: Vec<BrowseNavigation>,
}

pub struct BrowseNavigation {
	pub title: String,
	pub listing_id: String,
}

pub struct ListingPage {
	pub entries: Vec<Manga>,
	pub has_next_page: bool,
	pub groups: Vec<BrowseGroup>,
	pub navigation: Vec<BrowseNavigation>,
	pub parent_id: Option<String>,
}

fn navigation_listing_id(path: &[String]) -> String {
	if path.len() == 1 {
		return format!("opds:nav:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&path[0]));
	}
	format!(
		"opds:path:{}",
		base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(path).expect("serialize OPDS path"))
	)
}

fn decode_listing_path(listing_id: &str) -> ApiResult<Vec<String>> {
	let Some((prefix, encoded)) = listing_id.split_once(':').and_then(|(a, tail)| tail.split_once(':').map(|(b, c)| (format!("{a}:{b}"), c))) else {
		return Err(ApiError::not_found("unknown OPDS listing"));
	};
	let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
		.decode(encoded)
		.map_err(|_| ApiError::not_found("unknown OPDS listing"))?;
	if prefix == "opds:nav" {
		return String::from_utf8(bytes)
			.map(|url| vec![url])
			.map_err(|_| ApiError::not_found("unknown OPDS listing"));
	}
	if prefix == "opds:path" {
		let path: Vec<String> = serde_json::from_slice(&bytes).map_err(|_| ApiError::not_found("unknown OPDS listing"))?;
		if path.is_empty() || path.len() > 16 {
			return Err(ApiError::not_found("unknown OPDS listing"));
		}
		return Ok(path);
	}
	Err(ApiError::not_found("unknown OPDS listing"))
}

fn navigation_destinations(feed: &Feed, feed_url: &str) -> Vec<(String, String)> {
	feed.navigation
		.iter()
		.chain(feed.groups.iter().flat_map(|group| &group.navigation))
		.filter_map(|entry| {
			let link = entry.feed_link()?;
			Some((entry.title.clone(), Catalog::resolve(feed_url, &link.href).ok()?))
		})
		.collect()
}

fn top_navigation_destinations(feed: &Feed, feed_url: &str) -> Vec<(String, String)> {
	feed.navigation
		.iter()
		.filter_map(|entry| {
			let link = entry.feed_link()?;
			Some((entry.title.clone(), Catalog::resolve(feed_url, &link.href).ok()?))
		})
		.collect()
}

pub async fn get_listings(state: &Arc<AppState>, source_id: i64) -> ApiResult<Vec<Value>> {
	let catalog = Catalog::load(state, source_id).await?;
	let root = catalog.get_feed(state, catalog.base_url()).await?;
	let mut listings = vec![json!({"id": "opds:all", "name": "All", "kind": "default"})];
	let mut seen_urls = std::collections::HashSet::new();
	for entry in &root.navigation {
		let Some(link) = entry.feed_link() else { continue };
		let Ok(url) = Catalog::resolve(catalog.base_url(), &link.href) else { continue };
		let id = navigation_listing_id(std::slice::from_ref(&url));
		if seen_urls.insert(url) {
			listings.push(json!({"id": id, "name": entry.title, "kind": "default"}));
		}
	}
	Ok(listings)
}

pub async fn listing_page(
	state: &Arc<AppState>,
	source_id: i64,
	listing_id: &str,
	page: i32,
) -> ApiResult<ListingPage> {
	let catalog = Catalog::load(state, source_id).await?;
	let root_url = catalog.base_url().to_string();
	let path = if listing_id == "opds:all" { Vec::new() } else { decode_listing_path(listing_id)? };
	let mut feed_url = root_url.clone();
	let mut seen = std::collections::HashSet::from([root_url.clone()]);
	for target in &path {
		if !validate_navigation_target(&catalog, state, &feed_url, target).await? || !seen.insert(target.clone()) {
			return Err(ApiError::not_found("unknown OPDS listing"));
		}
		feed_url = target.clone();
	}
	let (feed, resolved_url, has_next_page) = search_page(&catalog, state, feed_url, page).await?;
	let mut entries = browse_entries(&feed.publications, &resolved_url);
	sign_work_locators(state, source_id, &mut entries).await?;
	let navigation = if path.is_empty() && page == 1 { Vec::new() } else { top_navigation_destinations(&feed, &resolved_url) }
		.into_iter()
		.map(|(title, url)| {
			let mut child_path = path.clone();
			child_path.push(url);
			BrowseNavigation { title, listing_id: navigation_listing_id(&child_path) }
		})
		.collect();
	let mut groups: Vec<BrowseGroup> = feed
		.groups
		.into_iter()
		.map(|group| BrowseGroup {
			title: group.title,
			entries: browse_entries(&group.publications, &resolved_url),
			navigation: group.navigation.iter().filter_map(|entry| {
				let link = entry.feed_link()?;
				let mut child_path = path.clone();
				child_path.push(Catalog::resolve(&resolved_url, &link.href).ok()?);
				Some(BrowseNavigation { title: entry.title.clone(), listing_id: navigation_listing_id(&child_path) })
			}).collect(),
		})
		.filter(|group| !group.entries.is_empty() || !group.navigation.is_empty())
		.collect();
	for group in &mut groups {
		sign_work_locators(state, source_id, &mut group.entries).await?;
	}
	Ok(ListingPage {
		entries,
		has_next_page,
		groups,
		navigation,
		parent_id: if path.is_empty() { None } else if path.len() == 1 { Some("opds:all".into()) } else { Some(navigation_listing_id(&path[..path.len() - 1])) },
	})
}

/// Search a parent feed for a caller-encoded target one page at a time. Stop as soon as a parent link proves
/// the hop; never fetch the selected feed twice or require unrelated later pages for a page-1 browse.
async fn validate_navigation_target(catalog: &Catalog, state: &AppState, parent_url: &str, target: &str) -> ApiResult<bool> {
	let mut current = parent_url.to_string();
	let mut seen = std::collections::HashSet::from([current.clone()]);
	for page in 0..FEED_MAX_PAGES {
		let feed = match catalog.get_feed(state, &current).await {
			Ok(feed) => feed,
			Err(error) if page > 0 => {
				tracing::warn!(parent_url, "opds navigation validation stopped at an unavailable later page: {}", error.message());
				return Ok(false);
			}
			Err(error) => return Err(error),
		};
		if navigation_destinations(&feed, &current).iter().any(|(_, url)| url == target) { return Ok(true); }
		let Some(next) = feed.next_link().map(|link| link.href.as_str()) else { return Ok(false) };
		let next = Catalog::resolve(&current, next)?;
		if !seen.insert(next.clone()) { return Ok(false); }
		current = next;
	}
	Ok(false)
}

fn browse_entries(entries: &[FeedEntry], feed_url: &str) -> Vec<Manga> {
	entries
		.iter()
		.filter(|entry| entry.is_actionable_publication())
		.filter_map(|entry| {
			let stable_id = stable_entry_id(entry, feed_url)?;
			let url = if entry.is_book() {
				feed_url.to_string()
			} else {
				Catalog::resolve(feed_url, &entry.feed_link()?.href).ok()?
			};
			let key = if entry.is_book() {
				work_key(&stable_id)
			} else if entry.id.is_empty() {
				entry
					.acquisition()
					.or_else(|| entry.pse())
					.or_else(|| entry.manifest())
					.and_then(|link| Catalog::resolve(feed_url, &link.href).ok())
					.unwrap_or_else(|| url.clone())
			} else {
				entry.id.clone()
			};
			Some(Manga {
				key,
				title: entry.title.clone(),
				url: Some(url),
				cover: entry
					.image()
					.or_else(|| entry.thumbnail())
					.and_then(|link| Catalog::resolve(feed_url, &link.href).ok()),
				description: (!entry.content.is_empty()).then(|| entry.content.clone()),
				authors: (!entry.authors.is_empty()).then(|| entry.authors.clone()),
				artists: (!entry.artists.is_empty()).then(|| entry.artists.clone()),
				tags: (!entry.tags.is_empty()).then(|| entry.tags.clone()),
				chapters: if entry.is_book() {
					entry_to_chapter(entry, feed_url).map(|chapter| vec![chapter])
				} else {
					None
				},
				..Manga::default()
			})
		})
		.collect()
}

fn work_key(entry_id: &str) -> String {
	format!("{WORK_KEY_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(entry_id))
}

fn locator_payload(value: &str) -> Option<&str> {
	value.strip_prefix("opds:v1:")?.split(':').next()
}

fn locator_mac_input(source_id: i64, manga_key: &str, chapter_key: &str, feed_url: &str, payload: &[u8]) -> Vec<u8> {
	fn push_part(out: &mut Vec<u8>, part: &[u8]) {
		out.extend_from_slice(&(part.len() as u64).to_be_bytes());
		out.extend_from_slice(part);
	}
	let mut out = b"sumidoku/opds-chapter-locator/v1\0".to_vec();
	out.extend_from_slice(&source_id.to_be_bytes());
	push_part(&mut out, manga_key.as_bytes());
	push_part(&mut out, chapter_key.as_bytes());
	push_part(&mut out, feed_url.as_bytes());
	push_part(&mut out, payload);
	out
}

fn sign_locator(source_id: i64, manga_key: &str, chapter_key: &str, feed_url: &str, locator: &str, key: &[u8]) -> Option<String> {
	use base64::Engine as _;
	let payload = locator_payload(locator)?;
	let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
	let input = locator_mac_input(source_id, manga_key, chapter_key, feed_url, &payload_bytes);
	let signing_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
	let signature = ring::hmac::sign(&signing_key, &input);
	Some(format!("opds:v1:{payload}:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())))
}

fn verify_locator_signature(source_id: i64, manga_key: &str, chapter_key: &str, feed_url: &str, locator: &str, key: &[u8]) -> bool {
	use base64::Engine as _;
	let Some((payload, signature)) = locator.strip_prefix("opds:v1:").and_then(|value| value.split_once(':')) else { return false };
	let Ok(payload_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else { return false };
	let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else { return false };
	let input = locator_mac_input(source_id, manga_key, chapter_key, feed_url, &payload_bytes);
	let signing_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
	ring::hmac::verify(&signing_key, &input, &signature).is_ok()
}

async fn locator_trusted_for_manga(state: &AppState, source_id: i64, manga_key: &str, chapter_key: &str, feed_url: &str, locator: &str) -> ApiResult<bool> {
	if locator.starts_with("opds:v1:") {
		let key = crate::server_state::opds_locator_key(&state.pool).await?;
		return Ok(verify_locator_signature(source_id, manga_key, chapter_key, feed_url, locator, &key));
	}
	Ok(!manga_key.starts_with(WORK_KEY_PREFIX))
}

async fn sign_work_locators(state: &AppState, source_id: i64, mangas: &mut [Manga]) -> ApiResult<()> {
	let needs_key = mangas.iter().any(|manga| {
		manga.key.starts_with(WORK_KEY_PREFIX)
			&& manga.chapters.as_ref().is_some_and(|chapters| chapters.iter().any(|chapter| chapter.url.as_deref().is_some_and(|url| url.starts_with("opds:v1:"))))
	});
	if !needs_key { return Ok(()) }
	let key = crate::server_state::opds_locator_key(&state.pool).await?;
	for manga in mangas.iter_mut().filter(|manga| manga.key.starts_with(WORK_KEY_PREFIX)) {
		let feed_url = manga.url.as_deref().unwrap_or_default();
		for chapter in manga.chapters.iter_mut().flatten() {
			if let Some(locator) = chapter.url.as_mut().filter(|url| url.starts_with("opds:v1:")) {
				*locator = sign_locator(source_id, &manga.key, &chapter.key, feed_url, locator, &key)
					.ok_or_else(|| ApiError::internal("failed to sign OPDS discovery locator"))?;
			}
		}
	}
	Ok(())
}

async fn signed_discovery_chapter(
	state: &AppState,
	source_id: i64,
	initial: &Manga,
	entry_id: &str,
	feed_url: &str,
) -> ApiResult<Option<Chapter>> {
	let Some(chapters) = initial.chapters.as_ref().filter(|chapters| chapters.len() == 1) else { return Ok(None) };
	let chapter = &chapters[0];
	let Some(locator) = chapter.url.as_deref().filter(|locator| locator.starts_with("opds:v1:")) else { return Ok(None) };
	if chapter.key != entry_id { return Ok(None) }
	let key = crate::server_state::opds_locator_key(&state.pool).await?;
	if !verify_locator_signature(source_id, &initial.key, &chapter.key, feed_url, locator, &key)
		|| decode_locator(locator).is_none()
	{
		return Ok(None);
	}
	Ok(Some(chapter.clone()))
}

fn stable_entry_id(entry: &FeedEntry, feed_url: &str) -> Option<String> {
	if !entry.id.is_empty() {
		return Some(entry.id.clone());
	}
	entry
		.links
		.iter()
		.find(|link| link.rel == "self")
		.or_else(|| entry.acquisition())
		.or_else(|| entry.pse())
		.or_else(|| entry.manifest())
		.and_then(|link| Catalog::resolve(feed_url, &link.href).ok())
}

fn work_entry_id(key: &str) -> ApiResult<Option<String>> {
	let Some(encoded) = key.strip_prefix(WORK_KEY_PREFIX) else {
		return Ok(None);
	};
	let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
		.decode(encoded)
		.map_err(|_| ApiError::bad_request("invalid OPDS work key"))?;
	String::from_utf8(bytes)
		.map(Some)
		.map_err(|_| ApiError::bad_request("invalid OPDS work key"))
}

async fn search_page(
	catalog: &Catalog,
	state: &AppState,
	mut url: String,
	page: i32,
) -> ApiResult<(Feed, String, bool)> {
	if page < 1 || page as usize > FEED_MAX_PAGES {
		return Err(ApiError::bad_request(format!("catalog search page exceeds maximum of {FEED_MAX_PAGES}")));
	}
	let mut visited = std::collections::HashSet::new();
	visited.insert(url.clone());
	for _ in 1..page {
		let feed = catalog.get_feed(state, &url).await?;
		let Some(next) = feed.next_link() else {
			return Ok((Feed::default(), url, false));
		};
		url = Catalog::resolve(&url, &next.href)?;
		if !visited.insert(url.clone()) {
			return Err(ApiError::bad_gateway("catalog search pagination repeated a link"));
		}
	}
	let feed = catalog.get_feed(state, &url).await?;
	let has_more = feed.next_link().is_some();
	if let Some(next) = feed.next_link() {
		let next_url = Catalog::resolve(&url, &next.href)?;
		if visited.contains(&next_url) {
			return Err(ApiError::bad_gateway("catalog search pagination repeated a link"));
		}
	}
	Ok((feed, url, has_more && (page as usize) < FEED_MAX_PAGES))
}

/// search capability mapping: the OpenSearch description wins; when missing/unusable, fall back to feed traversal marked degraded.
/// An empty query = browse, traversing the root directly (not counted as degraded)
pub async fn search(
	state: &Arc<AppState>,
	source_id: i64,
	query: Option<&str>,
	page: i32,
) -> ApiResult<SearchOutcome> {
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
				&& let Some(template) = parse_opensearch_template(&descriptor, Some("application/atom+xml"))
			{
				let search_url = Catalog::resolve(&descriptor_url, &opensearch_url(&template, query))?;
				let (feed, search_url, has_next_page) = search_page(&catalog, state, search_url, page).await?;
				let mut entries = feed_entries_to_manga(&feed, &search_url);
				sign_work_locators(state, source_id, &mut entries).await?;
				return Ok(SearchOutcome {
					entries,
					degraded: false,
					has_next_page,
				});
			}
		}
		// OPDS 2.0: a templated search link (…search{?query}) replaces the OpenSearch descriptor — expand and fetch directly
		let v2_search = root
			.links
			.iter()
			.find(|l| {
				l.rel == "search"
					&& (l.type_.starts_with(FEED_TYPE_V2) || l.type_.starts_with("application/atom+xml"))
					&& l.href.contains('{')
			});
		if let Some(link) = v2_search {
			let search_url = Catalog::resolve(&root_url, &search_template_url(&link.href, query))?;
			let (feed, search_url, has_next_page) = search_page(&catalog, state, search_url, page).await?;
			let mut entries = feed_entries_to_manga(&feed, &search_url);
			sign_work_locators(state, source_id, &mut entries).await?;
			return Ok(SearchOutcome {
				entries,
				degraded: false,
				has_next_page,
			});
		}
		let mut entries = traverse(&catalog, state, &root, &root_url, Some(query)).await?;
		sign_work_locators(state, source_id, &mut entries).await?;
		return Ok(SearchOutcome {
			entries,
			degraded: true,
			has_next_page: false,
		});
	}
	let mut entries = traverse(&catalog, state, &root, &root_url, None).await?;
	sign_work_locators(state, source_id, &mut entries).await?;
	Ok(SearchOutcome {
		entries,
		degraded: false,
		has_next_page: false,
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
	append_matching_entries(&mut out, &mut seen, root, root_url, needle.as_deref());
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
		append_matching_entries(&mut out, &mut seen, &feed, &feed_url, needle.as_deref());
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

fn append_matching_entries(
	out: &mut Vec<Manga>,
	seen: &mut std::collections::HashSet<String>,
	feed: &Feed,
	feed_url: &str,
	needle: Option<&str>,
) {
	for manga in feed_entries_to_manga(feed, feed_url) {
		if needle.is_some_and(|n| !manga.title.to_lowercase().contains(n)) { continue; }
		if seen.insert(manga.key.clone()) { out.push(manga); }
	}
}

/// Convert publications to source-side Manga entries. Navigation remains available for feed traversal and listing
/// payloads, but it never becomes a work result.
fn feed_entries_to_manga(feed: &Feed, feed_url: &str) -> Vec<Manga> {
	let publications: Vec<FeedEntry> = feed
		.publications
		.iter()
		.chain(feed.groups.iter().flat_map(|group| &group.publications))
	.cloned()
	.collect();
	browse_entries(&publications, feed_url)
}

/// The opds counterpart of fetching details + chapters from the source: pulls the entry feed and maps acquisition
/// entries to chapters; pagination (rel=next) is followed to completion. initial provides key and url (search results/library rows both carry them)
pub async fn manga_update(state: &Arc<AppState>, source_id: i64, initial: Manga) -> ApiResult<Manga> {
	let catalog = Catalog::load(state, source_id).await?;
	let standalone_id = work_entry_id(&initial.key)?;
	// Client chapter values are hints only. Resolve publication identity and locators from the fetched catalog.
	let feed_url = initial
		.url
		.clone()
		.filter(|u| !u.is_empty())
		.ok_or_else(|| ApiError::bad_request("opds manga requires a catalog url"))?;
	if let Some(entry_id) = standalone_id.as_deref()
		&& let Some(chapter) = signed_discovery_chapter(state, source_id, &initial, entry_id, &feed_url).await?
	{
		let mut updated = initial;
		updated.chapters = Some(vec![chapter]);
		return Ok(updated);
	}
	let feed = match fetch_feed_pages_until_entry(&catalog, state, &feed_url, FEED_MAX_PAGES, standalone_id.as_deref()).await {
		Ok(feed) => feed,
		Err(error) if standalone_id.is_some() => {
			if let Some(updated) = persisted_work_update(state, source_id, &initial, standalone_id.as_deref().unwrap(), &feed_url).await? {
				return Ok(updated);
			}
			return Err(error);
		}
		Err(error) => return Err(error),
	};
	let standalone = if let Some(entry_id) = standalone_id {
		if let Some(entry) = feed.entries.iter().find(|entry| entry.is_book() && stable_entry_id(entry, &feed_url).as_deref() == Some(&entry_id)) {
			Some(entry)
		} else {
			if let Some(updated) = persisted_work_update(state, source_id, &initial, &entry_id, &feed_url).await? {
				return Ok(updated);
			}
			return Err(ApiError::not_found("opds publication not found in catalog feed"));
		}
	} else {
		None
	};
	let chapter_entries: Vec<&FeedEntry> = standalone
		.map(|entry| vec![entry])
		.unwrap_or_else(|| feed.entries.iter().collect());
	let mut chapters: Vec<Chapter> = chapter_entries
		.iter()
		.filter_map(|entry| entry_to_chapter(entry, &feed_url))
		.collect();
	if chapters.iter().any(|chapter| chapter.url.as_deref().is_some_and(|url| url.starts_with("opds:v1:"))) {
		let key = crate::server_state::opds_locator_key(&state.pool).await?;
		for chapter in &mut chapters {
			if let Some(locator) = chapter.url.as_mut().filter(|url| url.starts_with("opds:v1:")) {
				*locator = sign_locator(source_id, &initial.key, &chapter.key, &feed_url, locator, &key)
					.ok_or_else(|| ApiError::internal("failed to sign OPDS chapter locator"))?;
			}
		}
	}
	let cover = if let Some(entry) = standalone {
		entry
			.image()
			.or_else(|| entry.thumbnail())
			.and_then(|l| Catalog::resolve(&feed_url, &l.href).ok())
			.or(initial.cover.clone())
	} else {
		feed.entries
			.iter()
			.find_map(|entry| {
			entry
				.image()
				.or_else(|| entry.thumbnail())
				.and_then(|l| Catalog::resolve(&feed_url, &l.href).ok())
			})
			.or(initial.cover.clone())
	};
	let mut authors: Vec<String> = Vec::new();
	let mut artists: Vec<String> = Vec::new();
	let mut tags: Vec<String> = Vec::new();
	for entry in &chapter_entries {
		for author in &entry.authors {
			push_unique(&mut authors, author.clone());
		}
		for artist in &entry.artists {
			push_unique(&mut artists, artist.clone());
		}
		for tag in &entry.tags {
			push_unique(&mut tags, tag.clone());
		}
	}
	Ok(Manga {
		key: initial.key.clone(),
		title: if standalone.is_some() || feed.title.is_empty() {
			initial.title.clone()
		} else {
			feed.title
		},
		url: Some(feed_url),
		cover,
		authors: if authors.is_empty() { initial.authors.clone() } else { Some(authors) },
		artists: if artists.is_empty() { initial.artists.clone() } else { Some(artists) },
		tags: if tags.is_empty() { initial.tags.clone() } else { Some(tags) },
		description: initial.description.clone(),
		chapters: Some(chapters),
		..Manga::default()
	})
}

async fn persisted_work_update(
	state: &AppState,
	source_id: i64,
	initial: &Manga,
	entry_id: &str,
	feed_url: &str,
) -> ApiResult<Option<Manga>> {
	let persisted: Option<PersistedWorkChapter> = sqlx::query_as(
		"SELECT c.key, c.title, c.number, c.volume, c.language, c.thumbnail, c.url, c.date_uploaded FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.source_id = ? AND m.key = ? AND c.key = ? AND c.trashed_at IS NULL",
	)
	.bind(source_id)
	.bind(&initial.key)
	.bind(entry_id)
	.fetch_optional(&state.pool)
	.await?;
	let Some(chapter) = persisted else { return Ok(None) };
	if let Some(url) = chapter.url.as_deref()
		&& !locator_trusted_for_manga(state, source_id, &initial.key, &chapter.key, feed_url, url).await?
	{
		return Ok(None);
	}
	Ok(Some(Manga {
		key: initial.key.clone(),
		title: initial.title.clone(),
		url: Some(feed_url.to_string()),
		cover: initial.cover.clone(),
		authors: initial.authors.clone(),
		artists: initial.artists.clone(),
		tags: initial.tags.clone(),
		description: initial.description.clone(),
		chapters: Some(vec![Chapter {
			key: chapter.key,
			title: chapter.title,
			chapter_number: chapter.number.map(|n| n as f32),
			volume_number: chapter.volume.map(|n| n as f32),
			language: chapter.language,
			thumbnail: chapter.thumbnail,
			url: chapter.url,
			date_uploaded: chapter.date_uploaded,
			locked: false,
			..Chapter::default()
		}]),
		..Manga::default()
	}))
}

/// The feed's first page plus rel=next pagination merged into the complete entry list (the feed-level title comes only from the first page)
async fn fetch_feed_pages(catalog: &Catalog, state: &AppState, feed_url: &str, max_pages: usize) -> ApiResult<Feed> {
	fetch_feed_pages_until_entry(catalog, state, feed_url, max_pages, None).await
}

/// Fetches complete feeds for reconciliation, but lets a targeted publication lookup stop once its entry is found.
/// A later unrelated page outage must not prevent registering a publication already fetched from an earlier page.
async fn fetch_feed_pages_until_entry(
	catalog: &Catalog,
	state: &AppState,
	feed_url: &str,
	max_pages: usize,
	target_entry_id: Option<&str>,
) -> ApiResult<Feed> {
	let first = catalog.get_feed(state, feed_url).await?;
	let mut next = first.next_link().map(|l| l.href.clone());
	let mut current_url = feed_url.to_string();
	let mut seen = std::collections::HashSet::from([current_url.clone()]);
	let mut first = first;
	normalize_feed_links(&mut first, feed_url);
	let mut merged = Feed {
		title: first.title,
		links: Vec::new(),
		entries: first.entries,
		navigation: first.navigation,
		publications: first.publications,
		groups: first.groups,
	};
	if target_entry_id.is_some_and(|target| {
		merged.entries.iter().any(|entry| entry.is_book() && stable_entry_id(entry, feed_url).as_deref() == Some(target))
	}) {
		return Ok(merged);
	}
	let mut pages = 1;
	while let Some(href) = next {
		if pages >= max_pages {
			tracing::warn!(feed_url, "opds feed pagination capped");
			break;
		}
		let url = Catalog::resolve(&current_url, &href)?;
		if !seen.insert(url.clone()) { return Err(ApiError::bad_gateway("opds feed pagination repeated a link")); }
		let mut feed = catalog.get_feed(state, &url).await?;
		next = feed.next_link().map(|l| l.href.clone());
		normalize_feed_links(&mut feed, &url);
		merged.entries.extend(feed.entries);
		merged.navigation.extend(feed.navigation);
		merged.publications.extend(feed.publications);
		merged.groups.extend(feed.groups);
		current_url = url;
		pages += 1;
		if target_entry_id.is_some_and(|target| {
			merged.entries.iter().any(|entry| entry.is_book() && stable_entry_id(entry, feed_url).as_deref() == Some(target))
		}) {
			return Ok(merged);
		}
	}
	Ok(merged)
}

fn normalize_feed_links(feed: &mut Feed, page_url: &str) {
	fn normalize(entry: &mut FeedEntry, page_url: &str) {
		for link in &mut entry.links {
			let resolved = if link.href.contains("{pageNumber}") { resolve_template(page_url, &link.href) } else { Catalog::resolve(page_url, &link.href) };
			if let Ok(url) = resolved { link.href = url; }
		}
	}
	for entry in &mut feed.entries { normalize(entry, page_url); }
	for entry in &mut feed.publications { normalize(entry, page_url); }
	for entry in &mut feed.navigation { normalize(entry, page_url); }
	for group in &mut feed.groups {
		for entry in &mut group.publications { normalize(entry, page_url); }
		for entry in &mut group.navigation { normalize(entry, page_url); }
	}
}

/// Book entry → chapter: url prefers a usable PSE template with its count, then the publication manifest, then the
/// acquisition archive. Number parses from the title's digits (volume-level chapters are a source feature, so an
/// unparseable number stays empty).
fn entry_to_chapter(entry: &FeedEntry, feed_url: &str) -> Option<Chapter> {
	if !entry.is_book() {
		return None;
	}
	let archive = entry.acquisition().and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
	let pse = entry
		.pse()
		.filter(|link| link.pse_count.is_some_and(|count| count > 0 && count <= PAGE_MAX_COUNT as i64) && link.href.contains("{pageNumber}"))
		.and_then(|link| Some((resolve_template(feed_url, &link.href).ok()?, link.pse_count?)));
	let manifest = entry.manifest().and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
	let url = if let Some((stream, count)) = pse {
		Some(pse_locator(&stream, Some(count), archive.as_deref()))
	} else if let Some(manifest) = manifest {
		Some(manifest_locator(&manifest, archive.as_deref()))
	} else {
		archive.clone().map(|archive| archive_locator(&archive))
	};
	let thumbnail = entry
		.thumbnail()
		.or_else(|| entry.image())
		.and_then(|l| Catalog::resolve(feed_url, &l.href).ok());
	Some(Chapter {
		key: stable_entry_id(entry, feed_url)?,
		title: Some(entry.title.clone()),
		chapter_number: crate::internal::local::chapter_number::parse_chapter_number(&entry.title),
		volume_number: None,
		date_uploaded: entry.updated,
		scanlators: None,
		url,
		language: None,
		thumbnail,
		locked: false,
	})
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ChapterLocator {
	Pse { stream: String, count: i64, archive: Option<String> },
	Manifest { manifest: String, archive: Option<String> },
	Archive { archive: String },
}

fn encode_locator(locator: ChapterLocator) -> String {
	let payload = serde_json::to_vec(&locator).expect("serialize OPDS chapter locator");
	format!("opds:v1:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload))
}

fn pse_locator(url: &str, count: Option<i64>, archive: Option<&str>) -> String {
	encode_locator(ChapterLocator::Pse { stream: url.to_string(), count: count.unwrap_or(0), archive: archive.map(str::to_string) })
}

fn resolve_template(feed_url: &str, href: &str) -> ApiResult<String> {
	let marker = "OPDS_PAGE_PLACEHOLDER";
	let escaped = href.replace("{pageNumber}", marker);
	Ok(Catalog::resolve(feed_url, &escaped)?.replace(marker, "{pageNumber}"))
}

fn manifest_locator(url: &str, archive: Option<&str>) -> String {
	encode_locator(ChapterLocator::Manifest { manifest: url.to_string(), archive: archive.map(str::to_string) })
}

fn archive_locator(url: &str) -> String {
	encode_locator(ChapterLocator::Archive { archive: url.to_string() })
}

fn decode_locator(value: &str) -> Option<ChapterLocator> {
	if let Some(encoded) = locator_payload(value) {
		let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).ok()?;
		return serde_json::from_slice(&bytes).ok();
	}
	let (url, fragment) = value.split_once('#')?;
	if let Some(count) = fragment.strip_prefix("opds-pse-count=") {
		Some(ChapterLocator::Pse { stream: url.to_string(), count: count.parse().ok()?, archive: None })
	} else if fragment == "opds-manifest" {
		Some(ChapterLocator::Manifest { manifest: url.to_string(), archive: None })
	} else {
		None
	}
}

fn is_direct_acquisition_locator(value: &str) -> bool {
	decode_locator(value).is_none()
		&& !value.contains("{pageNumber}")
		&& reqwest::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// load_pages' opds arm: PSE per-page links expand into Url pages (PSE page numbers are 0-based; Komga's endpoint
/// implements +1), otherwise the acquisition archive is downloaded to disk and ZipCache lists image entries into OpdsZip pages
pub async fn chapter_page_rows(state: &Arc<AppState>, manga: &MangaRow, chapter: &ChapterRow) -> ApiResult<Vec<Value>> {
	let locator = chapter.url.as_deref();
	let locator_trusted = if let Some(locator) = locator {
		locator_trusted_for_manga(state, manga.source_id, &manga.key, &chapter.key, manga.url.as_deref().unwrap_or_default(), locator).await?
	} else { false };
	let legacy_archive = locator.filter(|url| locator_trusted && is_direct_acquisition_locator(url));
	if locator_trusted
		&& let Some(locator) = chapter.url.as_deref().and_then(decode_locator)
	{
		match locator {
			ChapterLocator::Pse { stream, count, archive } => {
				if count > 0 && count as usize <= PAGE_MAX_COUNT && stream.contains("{pageNumber}") {
					return (0..count).map(|i| Ok(page_url_json(stream.replace("{pageNumber}", &i.to_string())))).collect();
				}
				if let Some(archive) = archive {
					let catalog = Catalog::load(state, manga.source_id).await?;
					return archive_page_rows(state, &catalog, chapter.id, &archive).await;
				}
			}
			ChapterLocator::Manifest { manifest, archive } => {
				let catalog = Catalog::load(state, manga.source_id).await?;
				let max = state.config.net_body_max_mb * 1024 * 1024;
				let pages = match catalog.get_bytes(state, &manifest, max).await {
					Ok((bytes, _)) => parse_reading_order(&bytes).ok().and_then(|order| manifest_page_rows(&manifest, &order).ok()),
					Err(_) => None,
				};
				if let Some(pages) = pages { return Ok(pages); }
				if let Some(archive) = archive { return archive_page_rows(state, &catalog, chapter.id, &archive).await; }
			}
			ChapterLocator::Archive { archive } => {
				let catalog = Catalog::load(state, manga.source_id).await?;
				return archive_page_rows(state, &catalog, chapter.id, &archive).await;
			}
		}
	}
	let catalog = Catalog::load(state, manga.source_id).await?;
	let feed_url = manga
		.url
		.clone()
		.filter(|u| !u.is_empty());
	let Some(feed_url) = feed_url else {
		if let Some(archive) = legacy_archive { return archive_page_rows(state, &catalog, chapter.id, archive).await; }
		return Err(ApiError::bad_request("opds manga requires a catalog url"));
	};
	let feed = match fetch_feed_pages(&catalog, state, &feed_url, FEED_MAX_PAGES).await {
		Ok(feed) => feed,
		Err(_) if let Some(archive) = legacy_archive => return archive_page_rows(state, &catalog, chapter.id, archive).await,
		Err(error) => return Err(error),
	};
	let entry = feed
		.entries
		.iter()
		.find(|e| stable_entry_id(e, &feed_url).as_deref() == Some(&chapter.key));
	let Some(entry) = entry else {
		if let Some(archive) = legacy_archive { return archive_page_rows(state, &catalog, chapter.id, archive).await; }
		return Err(ApiError::not_found("opds chapter not found in catalog feed"));
	};
	if let Some(pse) = entry.pse()
		&& let Some(count) = pse.pse_count
		&& count > 0
		&& count as usize <= PAGE_MAX_COUNT
		&& pse.href.contains("{pageNumber}")
	{
		// Replace the placeholder before parsing: {pageNumber}'s braces would be percent-encoded by Url::join
		let mut urls = Vec::with_capacity(count as usize);
		for i in 0..count {
			urls.push(page_url_json(Catalog::resolve(
				&feed_url,
				&pse.href.replace("{pageNumber}", &i.to_string()),
			)?));
		}
		return Ok(urls);
	}
	// OPDS 2.0 has no PSE: the book's publication document (self link) enumerates pages through readingOrder,
	// per-page streaming like PSE — preferred over downloading the whole acquisition archive
	if let Some(manifest) = entry.manifest() {
		let manifest_url = Catalog::resolve(&feed_url, &manifest.href)?;
		let max = state.config.net_body_max_mb * 1024 * 1024;
		let pages = match catalog.get_bytes(state, &manifest_url, max).await {
			Ok((bytes, _)) => parse_reading_order(&bytes).ok().and_then(|order| manifest_page_rows(&manifest_url, &order).ok()),
			Err(e) => {
				tracing::warn!(manifest_url, "opds manifest fetch failed, falling back to acquisition: {}", e.message());
				None
			}
		};
		if let Some(pages) = pages {
			return Ok(pages);
		}
		tracing::warn!(manifest_url, "opds manifest has no usable readingOrder, falling back to acquisition");
	}
	let href = entry.acquisition().map(|link| Catalog::resolve(&feed_url, &link.href)).transpose()?;
	let Some(href) = href.or_else(|| legacy_archive.map(str::to_string)) else {
		return Err(ApiError::bad_request("opds entry has neither page streaming nor acquisition link"));
	};
	archive_page_rows(state, &catalog, chapter.id, &href).await
}

async fn archive_page_rows(state: &Arc<AppState>, catalog: &Catalog, chapter_id: i64, href: &str) -> ApiResult<Vec<Value>> {
	let path = ensure_cbz(state, catalog, chapter_id, href).await?;
	let names = list_cbz_pages(state, &path).await?;
	Ok(names.iter().map(|name| page_opds_zip_json(name)).collect())
}

/// Page-image bytes (OpdsZip variant): when the on-disk cache is missing, re-fetches the feed to find the acquisition link, self-healing
pub async fn read_cbz_page(state: &Arc<AppState>, manga: &MangaRow, chapter: &ChapterRow, entry_name: &str) -> ApiResult<(Vec<u8>, String)> {
	let path = ensure_cbz_path(state, manga, chapter).await?;
	let zip = state.local_zip.clone();
	let entry = entry_name.to_string();
	let bytes = tokio::task::spawn_blocking(move || zip.read_entry(&path, &entry))
		.await
		.map_err(|e| ApiError::internal(format!("read zip entry failed: {e}")))?
		.map_err(|e| {
			if e.kind() == std::io::ErrorKind::NotFound {
				ApiError::not_found("page not found")
			} else {
				ApiError::bad_gateway(format!("read opds archive failed: {e}"))
			}
		})?;
	Ok((bytes, crate::internal::local::image_mime(entry_name).to_string()))
}

async fn ensure_cbz_path(state: &Arc<AppState>, manga: &MangaRow, chapter: &ChapterRow) -> ApiResult<PathBuf> {
	let path = cbz_path(state, chapter.id);
	if tokio::fs::metadata(&path).await.is_ok() {
		return Ok(path);
	}
	let locator = chapter.url.as_deref();
	let locator_trusted = if let Some(locator) = locator {
		locator_trusted_for_manga(state, manga.source_id, &manga.key, &chapter.key, manga.url.as_deref().unwrap_or_default(), locator).await?
	} else { false };
	let stored_archive = if let Some(locator) = locator.filter(|_| locator_trusted) { decode_locator(locator) } else { None }
		.and_then(|locator| match locator {
			ChapterLocator::Archive { archive } => Some(archive),
			ChapterLocator::Pse { archive, .. } | ChapterLocator::Manifest { archive, .. } => archive,
		});
	let raw_archive = locator.filter(|url| locator_trusted && is_direct_acquisition_locator(url));
	if let Some(url) = stored_archive.as_deref().or(raw_archive) {
		let catalog = Catalog::load(state, manga.source_id).await?;
		ensure_cbz(state, &catalog, chapter.id, url).await?;
		crate::cache::reconcile_chapter_accounted(state, chapter.id).await?;
		return Ok(path);
	}
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
		.find(|e| stable_entry_id(e, &feed_url).as_deref() == Some(&chapter.key))
		.ok_or_else(|| ApiError::not_found("opds chapter not found in catalog feed"))?;
	let acquisition = entry
		.acquisition()
		.ok_or_else(|| ApiError::bad_request("opds entry has no acquisition link"))?;
	let href = Catalog::resolve(&feed_url, &acquisition.href)?;
	ensure_cbz(state, &catalog, chapter.id, &href).await?;
	crate::cache::reconcile_chapter_accounted(state, chapter.id).await?;
	Ok(path)
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
		.filter(|n| crate::internal::local::is_image_entry(n))
		.collect();
	pages.sort_by(|a, b| crate::internal::local::natural_sort::compare(a, b));
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
		let both = parse_feed(
			br#"
<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>A</title><id>a</id>
<link rel="http://opds-spec.org/image-thumbnail" href="legacy.jpg"/>
<link rel="http://opds-spec.org/image/thumbnail" href="canonical.jpg"/>
</entry></feed>"#,
		)
			.expect("thumbnail relation variants");
		assert_eq!(both.entries[0].thumbnail().map(|l| l.href.as_str()), Some("canonical.jpg"));
		let alias = parse_feed(
			br#"
<feed xmlns="http://www.w3.org/2005/Atom"><entry>
<link rel="http://opds-spec.org/image-thumbnail" href="legacy.jpg"/>
</entry></feed>"#,
		)
			.expect("alternate thumbnail relation");
		assert_eq!(alias.entries[0].thumbnail().map(|l| l.href.as_str()), Some("legacy.jpg"));
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
  <entry>
    <title>Volume collection</title>
    <id>volumes</id>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog;kind=acquisition" href="/opds/books"/>
  </entry>
</feed>"#;
		let feed = parse_feed(xml.as_bytes()).expect("parse");
		assert_eq!(feed.entries.len(), 2);
		assert_eq!(feed.navigation.len(), 2);
		assert_eq!(feed.publications.len(), 0);
		let entry = &feed.entries[0];
		assert!(!entry.is_book());
		assert!(feed_entries_to_manga(&feed, "https://catalog.test/root").is_empty());
		assert_eq!(
			entry.feed_link().map(|l| l.href.as_str()),
			Some("/opds/series/alpha")
		);
		let search = feed.links.iter().find(|l| l.rel == "search").expect("search link");
		assert_eq!(search.href, "/opds/search");
	}

	#[test]
	fn atom_categories_and_authors_map_to_manga_metadata() {
		let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry>
<title>Metadata series</title><id>metadata-series</id>
<category term="action" label="Action"/><category term="Drama"/>
<author><name>Atom Author</name></author>
<link rel="subsection" type="application/atom+xml;kind=acquisition" href="/series/metadata"/>
</entry></feed>"#;
		let feed = parse_feed(xml).expect("Atom metadata feed");
		let manga = feed_entries_to_manga(&feed, "https://catalog.test/root");
		assert!(manga.is_empty(), "Atom subsection remains navigation despite kind=acquisition");
	}

	#[test]
	fn official_atom_sort_navigation_and_alternate_entry_links_are_classified_by_relation() {
		let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Newest</title><id>new</id>
<link rel="http://opds-spec.org/sort/new" type="application/atom+xml;kind=acquisition" href="/new"/></entry>
<entry><title>Popular</title><id>popular</id><link rel="http://opds-spec.org/sort/popular" type="application/atom+xml;kind=acquisition" href="/popular"/></entry>
<entry><title>Book</title><id>book</id><link rel="http://opds-spec.org/acquisition" type="application/zip" href="/book.cbz"/>
<link rel="alternate" type="application/atom+xml;type=entry" href="/book-entry"/></entry></feed>"#;
		let feed = parse_feed(xml).expect("Atom navigation feed");
		assert_eq!(feed.navigation.len(), 2);
		assert_eq!(feed.publications.len(), 1);
		assert!(feed.publications[0].is_book());
		assert!(feed.publications[0].feed_link().is_none());
		assert_eq!(feed_entries_to_manga(&feed, "https://catalog.test/root").len(), 1);
	}

	#[test]
	fn persisted_locators_keep_absolute_urls_and_pse_count() {
		let pse = pse_locator("https://catalog.test/pages/{pageNumber}", Some(17), Some("https://catalog.test/archive.cbz"));
		assert_eq!(decode_locator(&pse), Some(ChapterLocator::Pse { stream: "https://catalog.test/pages/{pageNumber}".into(), count: 17, archive: Some("https://catalog.test/archive.cbz".into()) }));
		let manifest = manifest_locator("https://catalog.test/book.json", Some("https://catalog.test/archive.cbz"));
		assert_eq!(decode_locator(&manifest), Some(ChapterLocator::Manifest { manifest: "https://catalog.test/book.json".into(), archive: Some("https://catalog.test/archive.cbz".into()) }));
		assert!(pse.starts_with("opds:v1:"));
		assert!(!is_direct_acquisition_locator(&manifest));
		assert!(!is_direct_acquisition_locator("https://catalog.test/pages/{pageNumber}"));
		assert!(is_direct_acquisition_locator("https://catalog.test/archive.cbz"));
		assert_eq!(decode_locator("https://catalog.test/pages/{pageNumber}#opds-pse-count=17"), Some(ChapterLocator::Pse { stream: "https://catalog.test/pages/{pageNumber}".into(), count: 17, archive: None }));
	}

	#[test]
	fn oversized_upstream_pse_uses_the_archive_locator() {
		let xml = format!(r#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns"><entry>
<id>book</id><title>Book</title><link rel="http://opds-spec.org/acquisition" href="/book.cbz"/>
<link rel="http://vaemendis.net/opds-pse/stream" href="/pages/{{pageNumber}}" pse:count="{}"/>
</entry></feed>"#, PAGE_MAX_COUNT + 1);
		let feed = parse_feed(xml.as_bytes()).expect("feed");
		let chapter = entry_to_chapter(&feed.entries[0], "https://catalog.test/feed").expect("chapter");
		assert!(matches!(decode_locator(chapter.url.as_deref().unwrap()), Some(ChapterLocator::Archive { archive }) if archive == "https://catalog.test/book.cbz"));
	}

	#[test]
	fn signed_locator_is_bound_to_source_manga_chapter_and_payload() {
		let unsigned = pse_locator("https://catalog.test/pages/{pageNumber}", Some(2), Some("https://catalog.test/archive.cbz"));
		let key = [42u8; 32];
		let feed_url = "https://catalog.example/search?q=book";
		let signed = sign_locator(7, "opds:work:Ym9vay0x", "book-1", feed_url, &unsigned, &key).unwrap();
		assert!(verify_locator_signature(7, "opds:work:Ym9vay0x", "book-1", feed_url, &signed, &key));
		assert!(!verify_locator_signature(8, "opds:work:Ym9vay0x", "book-1", feed_url, &signed, &key));
		assert!(!verify_locator_signature(7, "opds:work:other", "book-1", feed_url, &signed, &key));
		assert!(!verify_locator_signature(7, "opds:work:Ym9vay0x", "other", feed_url, &signed, &key));
		assert!(!verify_locator_signature(7, "opds:work:Ym9vay0x", "book-1", "https://catalog.example/other", &signed, &key));
		assert!(!verify_locator_signature(7, "opds:work:Ym9vay0x", "book-1", feed_url, &unsigned, &key));
		assert_eq!(decode_locator(&signed), decode_locator(&unsigned));
	}

	#[test]
	fn relative_page_two_links_use_the_page_that_supplied_them() {
		let mut feed = parse_feed(br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>book</id><title>Book</title>
<link rel="http://opds-spec.org/acquisition" href="../books/book.cbz"/><link rel="http://vaemendis.net/opds-pse/stream" href="../pages/{pageNumber}" pse:count="2"/></entry>
<entry><id>child</id><title>Child</title><link rel="subsection" type="application/atom+xml;kind=acquisition" href="../child"/></entry></feed>"#).unwrap();
		normalize_feed_links(&mut feed, "https://catalog.test/catalog/page2");
		assert_eq!(feed.entries[0].acquisition().unwrap().href, "https://catalog.test/books/book.cbz");
		assert_eq!(feed.entries[0].pse().unwrap().href, "https://catalog.test/pages/{pageNumber}");
		assert_eq!(navigation_destinations(&feed, "https://catalog.test/catalog/page2")[0].1, "https://catalog.test/child");
	}

	#[test]
	fn degraded_search_matches_root_publications_and_deduplicates() {
		let mut feed = Feed::default();
		feed.publications.push(FeedEntry {
			id: "root-book".into(), title: "Root Only Book".into(), content: String::new(), updated: None,
			authors: Vec::new(), artists: Vec::new(), tags: Vec::new(),
			links: vec![EntryLink { rel: "http://opds-spec.org/acquisition".into(), type_: "application/zip".into(), href: "/root.cbz".into(), pse_count: None }],
		});
		let mut out = Vec::new();
		let mut seen = std::collections::HashSet::new();
		append_matching_entries(&mut out, &mut seen, &feed, "https://catalog.test/root", Some("root only"));
		append_matching_entries(&mut out, &mut seen, &feed, "https://catalog.test/root", Some("root only"));
		assert_eq!(out.len(), 1);
		assert_eq!(out[0].key, "opds:work:cm9vdC1ib29r");
	}

	#[test]
	fn rwpm_subject_shapes_and_contributor_roles_map_to_manga_metadata() {
		let json = br#"{
  "publications": [
    {"metadata": {"title": "String subject", "subject": "Action"}, "links": [{"rel": "http://opds-spec.org/acquisition", "href": "/one.cbz"}]},
    {"metadata": {"title": "Object subject", "subject": {"name": "Mystery"}}, "links": [{"rel": "http://opds-spec.org/acquisition", "href": "/two.cbz"}]},
    {"metadata": {
      "title": "Array subjects", "identifier": "mapped", "subject": ["Drama", {"name": "Seinen"}],
      "author": [{"name": "Named Author"}],
      "artist": "Named Artist", "illustrator": [{"name": "Illustrator"}],
      "letterer": "Letterer", "penciller": "Penciller", "colorist": "Colorist", "inker": "Inker",
      "contributor": [
        {"name": "Role Author", "role": "aut"},
        {"name": "Role Creator", "role": "cre"},
        {"name": "Role Artist", "role": "art"},
        {"name": "Role Illustrator", "role": "ill"},
        {"name": "Role Letterer", "role": "let"},
        {"name": "Role Penciller", "role": "pnc"},
        {"name": "Role Colorist", "role": "clr"},
        {"name": "Role Inker", "role": "ink"},
        {"name": "Unknown Role", "role": "zzz"}
      ]
    }, "links": [{"rel": "http://opds-spec.org/acquisition", "href": "/three.cbz"}]}
  ]
}"#;
		let feed = parse_feed_json(json).expect("RWPM metadata feed");
		let manga = feed_entries_to_manga(&feed, "https://catalog.test/root");
		assert_eq!(manga.len(), 3);
		assert_eq!(manga[0].tags, Some(vec!["Action".to_string()]));
		assert_eq!(manga[1].tags, Some(vec!["Mystery".to_string()]));
		assert_eq!(manga[2].tags, Some(vec!["Drama".to_string(), "Seinen".to_string()]));
		assert_eq!(
			manga[2].authors,
			Some(vec!["Named Author".to_string(), "Role Author".to_string(), "Role Creator".to_string()])
		);
		assert_eq!(
			manga[2].artists,
			Some(vec![
				"Named Artist".to_string(),
				"Illustrator".to_string(),
				"Letterer".to_string(),
				"Penciller".to_string(),
				"Colorist".to_string(),
				"Inker".to_string(),
				"Role Artist".to_string(),
				"Role Illustrator".to_string(),
				"Role Letterer".to_string(),
				"Role Penciller".to_string(),
				"Role Colorist".to_string(),
				"Role Inker".to_string(),
			])
		);
	}

	#[test]
	fn opensearch_template_extraction_and_substitution() {
		let descriptor = r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Search</ShortName>
  <Description>Search for series</Description>
  <Url type="application/atom+xml;profile=opds-catalog;kind=acquisition" template="http://host/opds/series?search={searchTerms}"/>
</OpenSearchDescription>"#;
		let template = parse_opensearch_template(descriptor.as_bytes(), Some("application/atom+xml"))
			.expect("template");
		assert_eq!(
			opensearch_url(&template, "attack on titan"),
			"http://host/opds/series?search=attack%20on%20titan"
		);
		assert_eq!(opensearch_url("http://h/s?q={searchTerms?}&count={count?}", "a b"), "http://h/s?q=a%20b&count=");
		assert_eq!(opensearch_url("http://h/s?q={searchTerms}", "a+b /漢"), "http://h/s?q=a%2Bb%20%2F%E6%BC%A2");
		let selected = br#"
<OpenSearchDescription>
  <Url type="application/json" template="json?q={searchTerms}"/>
  <Url type="application/atom+xmlx" template="bad-prefix?q={searchTerms}"/>
  <Url type="APPLICATION/ATOM+XML;profile=opds-catalog" template="atom?q={searchTerms}"/>
</OpenSearchDescription>"#;
		assert_eq!(
			parse_opensearch_template(selected, Some("application/atom+xml")).as_deref(),
			Some("atom?q={searchTerms}")
		);
	}

	#[test]
	fn slugify_rules() {
		assert_eq!(slugify("My Komga!"), Some("my-komga".to_string()));
		assert_eq!(slugify("  spaces  inside  "), Some("spaces-inside".to_string()));
		assert_eq!(slugify("demo2"), Some("demo2".to_string()));
		assert_eq!(slugify("!!!"), None);
		// CJK names slug to themselves instead of failing with "no usable characters"
		assert_eq!(slugify("漫画"), Some("漫画".to_string()));
		assert_eq!(slugify("漫画 2024"), Some("漫画-2024".to_string()));
		assert_eq!(slugify("コミック・H"), Some("コミック-h".to_string()));
		assert_eq!(slugify("Café"), slugify("Cafe\u{301}"));
	}

	const V2_FEED: &str = r#"{
  "metadata": {"title": "Demo v2 catalog"},
  "links": [
    {"rel": "self", "href": "/opds/v2/root", "type": "application/opds+json"},
    {"rel": "start", "href": "/opds/v2/root", "type": "application/opds+json"},
    {"rel": "search", "href": "/opds/v2/search{?query}", "type": "application/opds+json", "templated": true},
    {"rel": "next", "href": "/opds/v2/root?page=2", "type": "application/opds+json"}
  ],
  "navigation": [
    {"title": "All series", "href": "/opds/v2/series"},
    {"title": "Help", "href": "https://example.com/help", "type": "text/html"}
  ],
  "publications": [
    {
      "metadata": {
        "@type": "http://schema.org/Book",
        "identifier": "urn:sumidoku:manga:1",
        "title": "Alpha Series",
        "author": [{"name": "Author One"}, "Author Two"],
        "modified": "2024-03-01T10:00:00Z",
        "description": "alpha description"
      },
      "links": [{"rel": "self", "href": "/opds/v2/series/1", "type": "application/opds+json"}],
      "images": [{"href": "/opds/v2/cover/1", "type": "image/jpeg"}]
    },
    {
      "metadata": {"identifier": "book-1", "title": "Vol. 1", "author": "Author One", "modified": "2024-03-02T10:00:00Z"},
      "links": [
        {"rel": "self", "href": "/opds/v2/books/book-1/manifest", "type": "application/opds-publication+json"},
        {"rel": ["http://opds-spec.org/acquisition", "http://opds-spec.org/acquisition/open-access"], "type": "application/zip", "href": "/opds/v2/books/book-1/file.cbz"}
      ],
      "images": [
        {"href": "/opds/v2/books/book-1/cover", "type": "image/jpeg"},
        {"href": "/opds/v2/books/book-1/thumb", "type": "image/jpeg"}
      ]
    }
  ],
  "groups": [
    {
      "metadata": {"title": "Latest series"},
      "navigation": [{"title": "Beta Series", "href": "/opds/v2/series/2", "type": "application/opds+json"}]
    },
    {
      "metadata": {"title": "Latest books"},
      "publications": [
        {
          "metadata": {"identifier": "book-2", "title": "Vol. 2"},
          "links": [{"rel": "http://opds-spec.org/acquisition", "type": "application/zip", "href": "/opds/v2/books/book-2/file.cbz"}]
        }
      ]
    }
  ]
}"#;

	#[test]
	fn parse_feed_json_maps_navigation_publications_and_groups() {
		let feed = parse_feed_json(V2_FEED.as_bytes()).expect("parse");
		assert_eq!(feed.title, "Demo v2 catalog");
		assert_eq!(feed.next_link().map(|l| l.href.as_str()), Some("/opds/v2/root?page=2"));
		let search = feed.links.iter().find(|l| l.rel == "search").expect("search link");
		assert_eq!(search.href, "/opds/v2/search{?query}");

		// Navigation and publications stay distinct while entries retains the parser's flattened traversal order.
		assert_eq!(feed.entries.len(), 6);
		assert_eq!(feed.navigation.len(), 2);
		assert_eq!(feed.publications.len(), 2);
		assert_eq!(feed.groups.len(), 2);
		assert_eq!(feed.groups[0].title, "Latest series");
		assert_eq!(feed.groups[0].navigation.len(), 1);
		assert_eq!(feed.groups[0].publications.len(), 0);
		assert_eq!(feed.groups[1].publications.len(), 1);
		let nav = &feed.entries[0];
		assert_eq!(nav.id, "/opds/v2/series");
		assert_eq!(nav.title, "All series");
		assert!(!nav.is_book());
		// an untyped navigation item still drills; a non-feed type does not
		assert_eq!(nav.feed_link().map(|l| l.href.as_str()), Some("/opds/v2/series"));
		assert!(feed.entries[1].feed_link().is_none());

		let series = &feed.entries[2];
		assert_eq!(series.id, "urn:sumidoku:manga:1");
		assert_eq!(series.title, "Alpha Series");
		assert_eq!(series.content, "alpha description");
		assert_eq!(series.updated, Some(1709287200));
		assert_eq!(series.authors, vec!["Author One".to_string(), "Author Two".to_string()]);
		assert!(!series.is_book());
		assert_eq!(series.feed_link().map(|l| l.href.as_str()), Some("/opds/v2/series/1"));
		assert_eq!(series.image().map(|l| l.href.as_str()), Some("/opds/v2/cover/1"));
		assert!(series.thumbnail().is_none());

		let book = &feed.entries[3];
		assert_eq!(book.id, "book-1");
		assert_eq!(book.updated, Some(1709373600));
		assert!(book.is_book());
		assert_eq!(
			book.acquisition().map(|l| l.href.as_str()),
			Some("/opds/v2/books/book-1/file.cbz")
		);
		assert_eq!(
			book.manifest().map(|l| l.href.as_str()),
			Some("/opds/v2/books/book-1/manifest")
		);
		assert_eq!(book.image().map(|l| l.href.as_str()), Some("/opds/v2/books/book-1/cover"));
		assert_eq!(book.thumbnail().map(|l| l.href.as_str()), Some("/opds/v2/books/book-1/thumb"));

		let group_nav = &feed.entries[4];
		assert_eq!(group_nav.id, "/opds/v2/series/2");
		assert_eq!(group_nav.title, "Beta Series");
		let group_book = &feed.entries[5];
		assert_eq!(group_book.id, "book-2");
		assert!(group_book.is_book());
	}

	#[test]
	fn parse_feed_json_series_and_standalone_books_become_manga_candidates() {
		let feed = parse_feed_json(V2_FEED.as_bytes()).expect("parse");
		let manga = feed_entries_to_manga(&feed, "http://host/opds/v2/root");
		let keys: Vec<&str> = manga.iter().map(|m| m.key.as_str()).collect();
		assert_eq!(keys, vec!["urn:sumidoku:manga:1", "opds:work:Ym9vay0x", "opds:work:Ym9vay0y"]);
		assert_eq!(manga[0].url.as_deref(), Some("http://host/opds/v2/series/1"));
		assert_eq!(manga[0].cover.as_deref(), Some("http://host/opds/v2/cover/1"));
		assert_eq!(manga[0].description.as_deref(), Some("alpha description"));
	}

	#[test]
	fn opds2_groups_only_flatten_and_resolve_relative_navigation() {
		let json = br#"
{
  "metadata": {"title": "Grouped"},
  "groups": [{
    "navigation": [{"title": "Series", "href": "series/one"}],
    "publications": [{
      "metadata": {"identifier": "book-1", "title": "Book"},
      "links": [{"rel": "http://opds-spec.org/acquisition",
                 "href": "books/one.cbz", "type": "application/zip"}]
    }]
  }]
}"#;
		let feed = parse_feed_json(json).expect("groups-only feed");
		assert_eq!(feed.entries.len(), 2);
		let manga = feed_entries_to_manga(&feed, "http://host/catalog/root.json");
		assert_eq!(manga.len(), 1);
		assert_eq!(manga[0].title, "Book");
		assert_eq!(manga[0].key, "opds:work:Ym9vay0x");
		assert_eq!(manga[0].url.as_deref(), Some("http://host/catalog/root.json"));
		assert!(feed.entries.iter().any(FeedEntry::is_book));
	}

	#[test]
	fn is_json_feed_detects_type_and_sniffs_body() {
		let json = br#"{"metadata": {"title": "x"}}"#;
		let atom = br#"<?xml version="1.0"?><feed/>"#;
		assert!(is_json_feed(Some("application/opds+json"), atom));
		assert!(is_json_feed(Some("application/opds-publication+json"), atom));
		assert!(is_json_feed(Some("application/json"), atom));
		assert!(is_json_feed(Some("application/atom+xml;profile=opds-catalog"), json));
		assert!(!is_json_feed(Some("application/atom+xml"), atom));
		assert!(is_json_feed(None, b"  {\"metadata\": {}}"));
		assert!(is_json_feed(None, b"\xef\xbb\xbf{\"metadata\": {}}"));
		assert!(!is_json_feed(None, atom));
	}

	#[test]
	fn search_template_url_expands_query_forms() {
		assert_eq!(
			search_template_url("http://h/opds/v2/search{?query}", "attack on titan"),
			"http://h/opds/v2/search?query=attack%20on%20titan"
		);
		assert_eq!(
			search_template_url("http://h/s?q={searchTerms}&count={count?}", "a b"),
			"http://h/s?q=a%20b&count="
		);
		assert_eq!(search_template_url("http://h/s?q={query}", "a+b /漢"), "http://h/s?q=a%2Bb%20%2F%E6%BC%A2");
		assert_eq!(search_template_url("http://h/s{?query}{&page}", "x"), "http://h/s?query=x");
	}

	#[test]
	fn parse_reading_order_lists_page_hrefs() {
		let manifest = r#"{
  "metadata": {"identifier": "book-1", "title": "Vol. 1"},
  "links": [{"rel": "self", "href": "/manifest", "type": "application/opds-publication+json"}],
  "readingOrder": [
    {"href": "pages/0", "type": "image/png"},
    {"href": "pages/1"},
    {"type": "image/png"}
  ]
}"#;
		let hrefs = parse_reading_order(manifest.as_bytes()).expect("readingOrder");
		assert_eq!(hrefs, vec!["pages/0".to_string(), "pages/1".to_string()]);
		assert!(parse_reading_order(br#"{"metadata": {}}"#).is_err());
	}
}
