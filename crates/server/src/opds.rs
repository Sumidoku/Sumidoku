use crate::cover::bytes_response;
use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterRow, LibraryEntryRow, MangaRow, unix_now};
use crate::service::{chapter_row, manga_row};
use crate::state::AppState;
use crate::{PAGE_SIZE, USER_ID, service};
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

const ATOM_FEED: &str = "application/atom+xml;profile=opds-catalog;type=feed";
const COVER_MAX_AGE: u64 = 86400;
const PAGE_MAX_AGE: u64 = 3600;

/// 在库 manga 投影（同 library.rs 的 LIBRARY_SELECT，私有无法复用故重抄）
const LIBRARY_SELECT: &str = "SELECT m.*, mu.in_library_at FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1
	 WHERE mu.in_library = 1";

/// 章节 + 进度 + 下载产物路径 + 漫画名（series/updates 两条 feed 共用）
const CHAPTER_SELECT: &str = "SELECT c.*, cu.last_page_read, cu.last_read_at,
	 d.path AS download_path, m.title AS manga_title
	 FROM chapter c
	 JOIN manga m ON m.id = c.manga_id
	 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
	 LEFT JOIN download d ON d.chapter_id = c.id AND d.status = 'finished'";

#[derive(Debug, sqlx::FromRow)]
struct OpdsChapterRow {
	#[sqlx(flatten)]
	chapter: ChapterRow,
	last_page_read: Option<i64>,
	last_read_at: Option<i64>,
	download_path: Option<String>,
	manga_title: String,
}

#[derive(Debug, sqlx::FromRow)]
struct HistoryRow {
	#[sqlx(flatten)]
	manga: MangaRow,
	last_read_at: Option<i64>,
}

pub fn router(state: Arc<AppState>) -> Router {
	Router::new()
		.route("/root", get(root_feed))
		.route("/osd", get(open_search_description))
		.route("/library", get(library_feed))
		.route("/categories", get(categories_feed))
		.route("/categories/{id}", get(category_feed))
		.route("/sources", get(sources_feed))
		.route("/sources/{id}", get(source_feed))
		.route("/history", get(history_feed))
		.route("/updates", get(updates_feed))
		.route("/search", get(search_feed))
		.route("/series/{manga_id}", get(series_feed))
		.route("/cover/{manga_id}", get(cover_image))
		.route("/file/{chapter_id}", get(chapter_file))
		.route("/stream/{chapter_id}/{page}", get(stream_page))
		.layer(axum::middleware::from_fn_with_state(state.clone(), basic_auth))
		.with_state(state)
}

/// OPDS 恒 Basic（§9）：basic_password 非空时全链路校验，用户名任意非空、密码匹配即可
async fn basic_auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
	let Some(expected) = &state.config.basic_password else {
		return next.run(req).await;
	};
	let granted = req
		.headers()
		.get(header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Basic "))
		.and_then(base64_decode)
		.and_then(|raw| String::from_utf8(raw).ok())
		.map(|cred| match cred.split_once(':') {
			Some((user, pass)) => !user.is_empty() && pass == *expected,
			None => false,
		})
		.unwrap_or(false);
	if granted {
		return next.run(req).await;
	}
	(
		StatusCode::UNAUTHORIZED,
		[(header::WWW_AUTHENTICATE, "Basic realm=\"Suzuri OPDS\"")],
		Json(json!({"error": {"code": "unauthorized", "message": "authentication required"}})),
	)
		.into_response()
}

/// 仓库没引 base64 crate，Basic 凭证只用标准字母表，手写解码
fn base64_decode(input: &str) -> Option<Vec<u8>> {
	fn val(c: u8) -> Option<u32> {
		match c {
			b'A'..=b'Z' => Some((c - b'A') as u32),
			b'a'..=b'z' => Some((c - b'a' + 26) as u32),
			b'0'..=b'9' => Some((c - b'0' + 52) as u32),
			b'+' => Some(62),
			b'/' => Some(63),
			_ => None,
		}
	}
	let input = input.trim_end_matches('=');
	let mut out = Vec::with_capacity(input.len() * 3 / 4);
	let mut acc = 0u32;
	let mut bits = 0;
	for &c in input.as_bytes() {
		acc = (acc << 6) | val(c)?;
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((acc >> bits) as u8);
		}
	}
	Some(out)
}

/// 拼绝对 URL：Host 头 + X-Forwarded-Proto（反代后才是 https），缺省 http://localhost
fn base_url(headers: &HeaderMap) -> String {
	let host = headers
		.get(header::HOST)
		.and_then(|v| v.to_str().ok())
		.filter(|s| !s.is_empty())
		.unwrap_or("localhost");
	let proto = headers
		.get("x-forwarded-proto")
		.and_then(|v| v.to_str().ok())
		.filter(|s| !s.is_empty())
		.unwrap_or("http");
	format!("{proto}://{host}")
}

fn esc(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			'"' => out.push_str("&quot;"),
			'\'' => out.push_str("&apos;"),
			c => out.push(c),
		}
	}
	out
}

fn rfc3339(secs: i64) -> String {
	chrono::DateTime::from_timestamp(secs, 0)
		.map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
		.unwrap_or_default()
}

/// rel=next 的 q 参数要进 URL query，先 percent-encode 再交给 esc 转义 &
fn url_query_escape(s: &str) -> String {
	let mut out = String::new();
	for b in s.bytes() {
		match b {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
			b => {
				let _ = write!(out, "%{b:02X}");
			}
		}
	}
	out
}

fn feed_response(xml: String) -> Response {
	Response::builder()
		.status(StatusCode::OK)
		.header(header::CONTENT_TYPE, ATOM_FEED)
		.body(Body::from(xml))
		.expect("feed response")
}

fn feed_head(id: &str, title: &str, self_href: &str, base: &str) -> String {
	format!(
		concat!(
			"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
			"<feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:opds=\"http://opds-spec.org/2010/catalog\" xmlns:pse=\"http://vaemendis.net/opds-pse\">\n",
			"<id>{}</id>\n<title>{}</title>\n<updated>{}</updated>\n",
			"<link rel=\"start\" type=\"application/atom+xml;profile=opds-catalog;type=feed\" href=\"{}\"/>\n",
			"<link rel=\"self\" type=\"application/atom+xml;profile=opds-catalog;type=feed\" href=\"{}\"/>\n",
		),
		esc(id),
		esc(title),
		rfc3339(unix_now()),
		esc(&format!("{base}/opds/v1.2/root")),
		esc(self_href),
	)
}

fn next_link(xml: &mut String, href: &str) {
	let _ = write!(
		xml,
		"<link rel=\"next\" type=\"application/atom+xml;profile=opds-catalog;type=feed\" href=\"{}\"/>\n",
		esc(href),
	);
}

fn nav_entry(xml: &mut String, base: &str, id: &str, title: &str, path: &str) {
	let _ = write!(
		xml,
		concat!(
			"<entry><id>urn:suzuri:nav:{}</id><title>{}</title><updated>{}</updated>",
			"<link rel=\"subsection\" type=\"application/atom+xml;profile=opds-catalog;type=feed\" href=\"{}\"/>",
			"<content type=\"text\">{}</content></entry>\n",
		),
		esc(id),
		esc(title),
		rfc3339(unix_now()),
		esc(&format!("{base}/opds/v1.2/{path}")),
		esc(title),
	);
}

fn manga_entry(xml: &mut String, base: &str, row: &MangaRow, updated: i64) {
	let _ = write!(
		xml,
		"<entry><id>urn:suzuri:manga:{}</id><title>{}</title><updated>{}</updated>",
		row.id,
		esc(&row.title),
		rfc3339(updated),
	);
	if let Some(author) = row.authors.0.first() {
		let _ = write!(xml, "<author><name>{}</name></author>", esc(author));
	}
	if let Some(description) = &row.description {
		let _ = write!(xml, "<summary>{}</summary>", esc(description));
	}
	let cover_href = esc(&format!("{base}/opds/v1.2/cover/{}", row.id));
	let _ = write!(
		xml,
		concat!(
			"<link rel=\"http://opds-spec.org/image\" type=\"image/jpeg\" href=\"{}\"/>",
			"<link rel=\"http://opds-spec.org/image/thumbnail\" type=\"image/jpeg\" href=\"{}\"/>",
			"<link rel=\"subsection\" type=\"application/atom+xml;profile=opds-catalog;type=feed\" href=\"{}\"/>",
		),
		cover_href,
		cover_href,
		esc(&format!("{base}/opds/v1.2/series/{}", row.id)),
	);
	xml.push_str("</entry>\n");
}

/// 多抓一行判 has_next，与 API 分页同一手法
fn manga_feed(
	base: &str,
	feed_id: &str,
	title: &str,
	self_href: &str,
	next_href: &str,
	rows: Vec<LibraryEntryRow>,
) -> Response {
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let mut xml = feed_head(feed_id, title, self_href, base);
	if has_next {
		next_link(&mut xml, next_href);
	}
	for row in rows.iter().take(PAGE_SIZE as usize) {
		let updated = row
			.manga
			.last_fetched_at
			.or(row.in_library_at)
			.unwrap_or(0);
		manga_entry(&mut xml, base, &row.manga, updated);
	}
	xml.push_str("</feed>");
	feed_response(xml)
}

fn chapter_feed(
	base: &str,
	feed_id: &str,
	title: &str,
	self_href: &str,
	next_href: &str,
	rows: Vec<OpdsChapterRow>,
) -> Response {
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let mut xml = feed_head(feed_id, title, self_href, base);
	if has_next {
		next_link(&mut xml, next_href);
	}
	for row in rows.iter().take(PAGE_SIZE as usize) {
		chapter_entry(&mut xml, base, row);
	}
	xml.push_str("</feed>");
	feed_response(xml)
}

fn chapter_title(number: Option<f64>, title: Option<&str>, key: &str) -> String {
	let mut out = String::new();
	if let Some(n) = number {
		if n.fract() == 0.0 {
			let _ = write!(out, "第 {} 话", n as i64);
		} else {
			let _ = write!(out, "第 {n} 话");
		}
	}
	if let Some(t) = title.filter(|t| !t.is_empty()) {
		if !out.is_empty() {
			out.push(' ');
		}
		out.push_str(t);
	}
	if out.is_empty() {
		out.push_str(key);
	}
	out
}

fn chapter_entry(xml: &mut String, base: &str, row: &OpdsChapterRow) {
	let c = &row.chapter;
	let title = chapter_title(c.number, c.title.as_deref(), &c.key);
	let updated = c.date_uploaded.unwrap_or(c.fetched_at);
	let _ = write!(
		xml,
		"<entry><id>urn:suzuri:chapter:{}</id><title>{}</title><updated>{}</updated>",
		c.id,
		esc(&title),
		rfc3339(updated),
	);
	// 章节自身没有简介字段，content 给漫画名 + 扫图组做摘要
	let mut summary = row.manga_title.clone();
	if !c.scanlators.0.is_empty() {
		let _ = write!(summary, " ({})", c.scanlators.0.join(", "));
	}
	let _ = write!(xml, "<content type=\"text\">{}</content>", esc(&summary));
	if row.download_path.as_deref().is_some_and(|p| !p.is_empty()) {
		let _ = write!(
			xml,
			"<link rel=\"http://opds-spec.org/acquisition\" type=\"application/zip\" href=\"{}\"/>",
			esc(&format!("{base}/opds/v1.2/file/{}", c.id)),
		);
	}
	let _ = write!(
		xml,
		concat!(
			"<link rel=\"http://vaemendis.net/opds-pse/stream\" type=\"image/jpeg\" href=\"{}\"",
			" pse:count=\"{}\" pse:lastRead=\"{}\"",
		),
		esc(&format!("{base}/opds/v1.2/stream/{}/0", c.id)),
		c.page_count,
		row.last_page_read.unwrap_or(0),
	);
	if let Some(last_read_at) = row.last_read_at {
		let _ = write!(xml, " pse:lastReadDate=\"{}\"", rfc3339(last_read_at));
	}
	xml.push_str("/></entry>\n");
}

#[derive(Debug, Deserialize)]
struct FeedQuery {
	page: Option<i64>,
	q: Option<String>,
}

fn page_of(query: &FeedQuery) -> ApiResult<i64> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	Ok(page)
}

async fn root_feed(headers: HeaderMap) -> Response {
	let base = base_url(&headers);
	let mut xml = feed_head("urn:suzuri:root", "Suzuri", &format!("{base}/opds/v1.2/root"), &base);
	let _ = write!(
		xml,
		"<link rel=\"search\" type=\"application/opensearchdescription+xml\" href=\"{}\"/>\n",
		esc(&format!("{base}/opds/v1.2/osd")),
	);
	for (id, title, path) in [
		("library", "Library", "library"),
		("categories", "Categories", "categories"),
		("sources", "Sources", "sources"),
		("history", "History", "history"),
		("updates", "Updates", "updates"),
	] {
		nav_entry(&mut xml, &base, id, title, path);
	}
	xml.push_str("</feed>");
	feed_response(xml)
}

async fn open_search_description(headers: HeaderMap) -> Response {
	let base = base_url(&headers);
	let xml = format!(
		concat!(
			"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
			"<OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n",
			"<ShortName>Suzuri</ShortName>\n<Description>Search Suzuri library</Description>\n",
			"<InputEncoding>UTF-8</InputEncoding>\n<OutputEncoding>UTF-8</OutputEncoding>\n",
			"<Url type=\"application/atom+xml;profile=opds-catalog;type=feed\" template=\"{}\"/>\n",
			"</OpenSearchDescription>",
		),
		// {searchTerms} 是 OpenSearch 模板占位符，原样输出
		esc(&format!("{base}/opds/v1.2/search?q={{searchTerms}}")),
	);
	Response::builder()
		.status(StatusCode::OK)
		.header(header::CONTENT_TYPE, "application/opensearchdescription+xml")
		.body(Body::from(xml))
		.expect("osd response")
}

async fn library_feed(
	State(state): State<Arc<AppState>>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/library?page={page}");
	let next_href = format!("{base}/opds/v1.2/library?page={}", page + 1);
	Ok(manga_feed(&base, "urn:suzuri:library", "Library", &self_href, &next_href, rows))
}

async fn categories_feed(
	State(state): State<Arc<AppState>>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let base = base_url(&headers);
	let rows: Vec<(i64, String)> =
		sqlx::query_as("SELECT id, name FROM category WHERE user_id = ? ORDER BY sort_order ASC, id ASC")
			.bind(USER_ID)
			.fetch_all(&state.pool)
			.await?;
	let mut xml = feed_head(
		"urn:suzuri:categories",
		"Categories",
		&format!("{base}/opds/v1.2/categories"),
		&base,
	);
	for (id, name) in rows {
		nav_entry(&mut xml, &base, &format!("category:{id}"), &name, &format!("categories/{id}"));
	}
	xml.push_str("</feed>");
	Ok(feed_response(xml))
}

async fn category_feed(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let category: Option<(String,)> =
		sqlx::query_as("SELECT name FROM category WHERE id = ? AND user_id = ?")
			.bind(id)
			.bind(USER_ID)
			.fetch_optional(&state.pool)
			.await?;
	let Some((name,)) = category else {
		return Err(ApiError::not_found("category not found"));
	};
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} AND EXISTS (SELECT 1 FROM category_manga cm
		 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND cm.category_id = ?)
		 ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/categories/{id}?page={page}");
	let next_href = format!("{base}/opds/v1.2/categories/{id}?page={}", page + 1);
	Ok(manga_feed(
		&base,
		&format!("urn:suzuri:category:{id}"),
		&name,
		&self_href,
		&next_href,
		rows,
	))
}

async fn sources_feed(
	State(state): State<Arc<AppState>>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let base = base_url(&headers);
	let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM source ORDER BY id ASC")
		.fetch_all(&state.pool)
		.await?;
	let mut xml = feed_head(
		"urn:suzuri:sources",
		"Sources",
		&format!("{base}/opds/v1.2/sources"),
		&base,
	);
	for (id, name) in rows {
		nav_entry(&mut xml, &base, &format!("source:{id}"), &name, &format!("sources/{id}"));
	}
	xml.push_str("</feed>");
	Ok(feed_response(xml))
}

async fn source_feed(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let source: Option<(String,)> = sqlx::query_as("SELECT name FROM source WHERE id = ?")
		.bind(id)
		.fetch_optional(&state.pool)
		.await?;
	let Some((name,)) = source else {
		return Err(ApiError::not_found("source not found"));
	};
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} AND m.source_id = ? ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/sources/{id}?page={page}");
	let next_href = format!("{base}/opds/v1.2/sources/{id}?page={}", page + 1);
	Ok(manga_feed(
		&base,
		&format!("urn:suzuri:source:{id}"),
		&name,
		&self_href,
		&next_href,
		rows,
	))
}

async fn history_feed(
	State(state): State<Arc<AppState>>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let base = base_url(&headers);
	let rows: Vec<HistoryRow> = sqlx::query_as(
		"SELECT m.*, MAX(cu.last_read_at) AS last_read_at
		 FROM chapter_user cu
		 JOIN chapter c ON c.id = cu.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 WHERE cu.user_id = ? AND cu.last_read_at IS NOT NULL
		 GROUP BY m.id
		 ORDER BY last_read_at DESC, m.id DESC
		 LIMIT ? OFFSET ?",
	)
	.bind(USER_ID)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/history?page={page}");
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let mut xml = feed_head("urn:suzuri:history", "History", &self_href, &base);
	if has_next {
		next_link(&mut xml, &format!("{base}/opds/v1.2/history?page={}", page + 1));
	}
	for row in rows.iter().take(PAGE_SIZE as usize) {
		manga_entry(&mut xml, &base, &row.manga, row.last_read_at.unwrap_or(0));
	}
	xml.push_str("</feed>");
	Ok(feed_response(xml))
}

async fn updates_feed(
	State(state): State<Arc<AppState>>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let base = base_url(&headers);
	let rows: Vec<OpdsChapterRow> = sqlx::query_as(&format!(
		"{CHAPTER_SELECT}
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = 1 AND mu.in_library = 1
		 ORDER BY c.fetched_at DESC, c.id DESC LIMIT ? OFFSET ?"
	))
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/updates?page={page}");
	let next_href = format!("{base}/opds/v1.2/updates?page={}", page + 1);
	Ok(chapter_feed(&base, "urn:suzuri:updates", "Updates", &self_href, &next_href, rows))
}

async fn search_feed(
	State(state): State<Arc<AppState>>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let q = query.q.clone().unwrap_or_default();
	let base = base_url(&headers);
	let mut pattern = String::with_capacity(q.len() + 2);
	pattern.push('%');
	for c in q.chars() {
		// LIKE 通配符按字面匹配
		if matches!(c, '%' | '_' | '\\') {
			pattern.push('\\');
		}
		pattern.push(c);
	}
	pattern.push('%');
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} AND m.title LIKE ? ESCAPE '\\' ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(pattern)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let enc_q = url_query_escape(&q);
	let self_href = format!("{base}/opds/v1.2/search?q={enc_q}&page={page}");
	let next_href = format!("{base}/opds/v1.2/search?q={enc_q}&page={}", page + 1);
	Ok(manga_feed(
		&base,
		&format!("urn:suzuri:search:{q}"),
		&format!("Search: {q}"),
		&self_href,
		&next_href,
		rows,
	))
}

async fn series_feed(
	State(state): State<Arc<AppState>>,
	Path(manga_id): Path<i64>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let manga = manga_row(&state.pool, manga_id).await?;
	let base = base_url(&headers);
	let rows: Vec<OpdsChapterRow> = sqlx::query_as(&format!(
		"{CHAPTER_SELECT}
		 WHERE c.manga_id = ? ORDER BY c.source_order DESC, c.id DESC LIMIT ? OFFSET ?"
	))
	.bind(manga_id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let self_href = format!("{base}/opds/v1.2/series/{manga_id}?page={page}");
	let next_href = format!("{base}/opds/v1.2/series/{manga_id}?page={}", page + 1);
	Ok(chapter_feed(
		&base,
		&format!("urn:suzuri:manga:{manga_id}"),
		&manga.title,
		&self_href,
		&next_href,
		rows,
	))
}

async fn cover_image(
	State(state): State<Arc<AppState>>,
	Path(manga_id): Path<i64>,
) -> ApiResult<Response> {
	let (bytes, mime) = service::cover_bytes(&state, manga_id).await?;
	Ok(bytes_response(
		bytes,
		mime.as_deref().unwrap_or("image/jpeg"),
		COVER_MAX_AGE,
	))
}

async fn chapter_file(
	State(state): State<Arc<AppState>>,
	Path(chapter_id): Path<i64>,
) -> ApiResult<Response> {
	chapter_row(&state.pool, chapter_id).await?;
	let row: Option<(String, Option<String>)> =
		sqlx::query_as("SELECT status, path FROM download WHERE chapter_id = ?")
			.bind(chapter_id)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((status, Some(path))) = row
		&& status == "finished"
		&& !path.is_empty()
	{
		let full = state.config.downloads_dir.join(&path);
		let bytes = match tokio::fs::read(&full).await {
			Ok(bytes) => bytes,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
				return Err(ApiError::not_found("downloaded file missing"));
			}
			Err(e) => return Err(ApiError::from(sqlx::Error::Io(e))),
		};
		return Ok(Response::builder()
			.status(StatusCode::OK)
			.header(header::CONTENT_TYPE, "application/zip")
			.body(Body::from(bytes))
			.expect("file response"));
	}
	if !state.config.opds_lazy_download {
		return Err(ApiError::not_found("chapter not downloaded"));
	}
	// 只入库排队：运行期调度（下载 worker 拉起）在集成阶段统一接线
	let now = unix_now();
	sqlx::query(
		"INSERT INTO download (chapter_id, user_id, status, format, created_at, updated_at)
		 VALUES (?, ?, 'queued', ?, ?, ?)
		 ON CONFLICT (chapter_id) DO UPDATE SET
		 status = 'queued', error = NULL, updated_at = excluded.updated_at",
	)
	.bind(chapter_id)
	.bind(USER_ID)
	.bind(&state.config.download_format)
	.bind(now)
	.bind(now)
	.execute(&state.pool)
	.await?;
	Ok((
		StatusCode::ACCEPTED,
		Json(json!({"queued": true, "chapterId": chapter_id})),
	)
		.into_response())
}

async fn stream_page(
	State(state): State<Arc<AppState>>,
	Path((chapter_id, index)): Path<(i64, i64)>,
) -> ApiResult<Response> {
	let pages = service::load_pages(&state, chapter_id, false).await?;
	let Some((_, content)) = pages.iter().find(|(idx, _)| *idx == index) else {
		return Err(ApiError::not_found("page not found"));
	};
	let stored: Value = serde_json::from_str(content)?;
	let content = stored.get("content").cloned().unwrap_or(Value::Null);
	// 文本页给不了图流；ApiError 没有 415 构造，这里直接拼响应
	if content.get("Text").is_some() {
		return Ok((
			StatusCode::UNSUPPORTED_MEDIA_TYPE,
			Json(json!({"error": {"code": "unsupported", "message": "text page content cannot be streamed as image"}})),
		)
			.into_response());
	}
	let (bytes, mime) = service::page_content(&state, chapter_id, index).await?;
	Ok(bytes_response(
		bytes,
		mime.as_deref().unwrap_or("application/octet-stream"),
		PAGE_MAX_AGE,
	))
}
