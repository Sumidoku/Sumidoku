//! 内置 OPDS 源（kind = opds，多实例）：OPDS 1.2（Atom）目录作为一等源，无 wasm 载荷。
//! manga = 目录条目（<id> 为 key，条目 feed 链接为 url，刷新凭它回源）；chapters =
//! 条目 feed 里的 acquisition entries；pages = OPDS-PSE 逐页流链接优先，缺失时下载
//! acquisition 压缩包落盘、复用 local 的 ZipCache 拆页。basic 凭据存 source_setting，
//! 注册/删除走 admin 面。OPDS 2.0（JSON）未覆盖，是 follow-up。

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
/// rel 以前缀匹配：acquisition/open-access、buy 等都是文件取回链接
const ACQUISITION_PREFIX: &str = "http://opds-spec.org/acquisition";
/// 降级遍历的失控保险：BFS 深度与总 feed 数双帽（Gutenberg 级目录不能全站爬）
const TRAVERSE_MAX_DEPTH: usize = 2;
const TRAVERSE_MAX_FEEDS: usize = 12;
/// series feed 分页（rel=next）跟随上限：章节/页清单要全量，但目录坏了不能死循环
const FEED_MAX_PAGES: usize = 20;

/// 单实例目录上下文：base_url 是注册的目录根；basic 凭据来自 source_setting（空 = 公共目录）
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

	/// 有界 authed GET；非 2xx 连带状态码上报（401 即凭据错误的可见面）
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

	/// href 相对它所在 feed 的 URL 解析成绝对：Komga 发绝对链接，通用目录可能发相对
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

/// 目录条目的一个链接；pse_count 只出现在 OPDS-PSE 流链接上（pse:count 属性）
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

	/// 册（acquisition entry）：有 acquisition 或 PSE 流链接，否则是可下钻的 series/导航条目
	pub fn is_book(&self) -> bool {
		self.acquisition().is_some() || self.pse().is_some()
	}

	pub fn acquisition(&self) -> Option<&EntryLink> {
		self.links.iter().find(|l| l.rel.starts_with(ACQUISITION_PREFIX))
	}

	pub fn pse(&self) -> Option<&EntryLink> {
		self.links_by_rel(PSE_STREAM_REL).next()
	}

	/// 下钻链接：指向另一个 feed（atom 类型即 feed 标记，rel 多为 subsection）
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

/// 元素/属性本地名（剥命名空间前缀）：Atom 默认命名空间无前缀，pse:* 属性剥成 count
fn local_name(name: &str) -> &str {
	name.rsplit(':').next().unwrap_or(name)
}

/// 事件流解析 Atom feed：serde 绑定搞不定 content 里混嵌的 <br/> 这类内联标记，
/// 逐事件只认 entry/id/title/updated/content/summary/author/name/link 这一小组标签
pub fn parse_feed(bytes: &[u8]) -> Result<Feed, String> {
	let mut reader = quick_xml::Reader::from_reader(bytes);
	reader.config_mut().trim_text(true);
	let mut feed = Feed::default();
	let mut entry: Option<FeedEntry> = None;
	let mut in_author = false;
	// 正在收集文本的目标标签（本地名）；content/summary 内的嵌套标签不切换目标
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

/// 文本事件之间的实体引用：五个预定义实体 + 十进制/十六进制字符引用，其余丢弃
/// （未知命名实体拿不到 DTD 没法解析，标题里也不该出现）
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
		// normalized_value 解析预定义实体：href 查询串里的 &amp; 必须还原成 &
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

/// OpenSearch 描述文档里的结果 URL 模板（第一个 Url 元素的 template 属性）
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

/// 模板替换：{searchTerms}（含可选形 {?}）换成编码后的查询词，其余可选参数按缺省抹空。
/// byte_serialize 的空格是 +（form 语义），查询串统一成 %20（输出里的 + 只可能来自空格，
/// 原义 + 已被编成 %2B）
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
	/// 目录没有 OpenSearch 时退 feed 遍历的标记：结果只是标题子串过滤，不是真搜索
	pub degraded: bool,
}

/// search 能力映射：OpenSearch 描述优先；缺失/不可用退 feed 遍历并标 degraded。
/// query 空 = 浏览，直接遍历根目录（不算降级）
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

/// 降级遍历：BFS 下钻（深度/总量双帽），可下钻条目作为 manga 候选；根 feed 的条目是
/// 栏位（Komga 的 All series 等），候选只收 depth>=1 的 feed 里的条目
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

/// 可下钻条目 → 源侧 Manga：key 是条目 <id>，url 是下钻 feed 链接（回源的唯一凭据）；
/// 册形条目（acquisition 直接挂在列表里）不进 manga 模型，是系列章节的料
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

/// 回源拉详情 + 章节的 opds 对应物：取条目 feed，acquisition entries 映射成章节；
/// 分页（rel=next）跟随到全量。initial 提供 key 与 url（搜索结果/库行都带）
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

/// feed 首页 + rel=next 分页拼成全量条目列表（feed 级 title 只从首页出）
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

/// 册条目 → 章节：url 优先 acquisition 文件链接（CBZ 兜底与下载都指它），
/// 无文件链接退 PSE 流基址；number 按标题数字解析（卷级章是源特性，解析不出留空）
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

/// load_pages 的 opds 臂：PSE 逐页链接展开成 Url 页（PSE 页码 0 基，Komga 端点 +1 实现），
/// 否则下载 acquisition 压缩包落盘、ZipCache 列图片条目成 OpdsZip 页
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
		// 先替换占位符再解析：{pageNumber} 的花括号过 Url::join 会被百分号编码
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

/// 页图字节（OpdsZip 变体）：落盘缓存缺席时重取 feed 找 acquisition 链接自愈
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

/// acquisition 压缩包的落盘路径：按 chapter_id 命名，页行重写时由调用方删旧文件
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

/// 页内容 JSON 的 OpdsZip 变体：值是压缩包内的页条目名，包路径按 chapter_id 派生
fn page_opds_zip_json(entry: &str) -> Value {
	serde_json::json!({
		"content": { "OpdsZip": entry },
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	})
}

/// name 派生实例 slug：小写字母数字段以 - 相连；一个合法字符都没有没法生成 key
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

/// 实例注册：key 取 opds.<slug>，撞名追加 -2/-3 计数（Aidoku 同名源并存的先例）；
/// basic 凭据随 source_setting 落库（admin 面管理），空用户名 = 公共目录不种凭据
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
