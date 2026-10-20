use crate::error::{ApiError, ApiResult};
use crate::model::{LibraryEntryRow, MangaRow};
use crate::opds::{
	CHAPTER_SELECT, FeedQuery, HistoryRow, LIBRARY_SELECT, OpdsChapterRow, base_url, chapter_title,
	like_pattern, page_of, rfc3339, url_query_escape,
};
use crate::source::manga_row;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use serde_json::{Value, json};
use std::sync::Arc;

const FEED: &str = "application/opds+json";

pub fn router(state: Arc<AppState>) -> Router {
	Router::new()
		.route("/root", get(root_feed))
		.route("/library", get(library_feed))
		.route("/categories", get(categories_feed))
		.route("/categories/{id}", get(category_feed))
		.route("/sources", get(sources_feed))
		.route("/sources/{id}", get(source_feed))
		.route("/history", get(history_feed))
		.route("/updates", get(updates_feed))
		.route("/search", get(search_feed))
		.route("/series/{manga_id}", get(series_feed))
		.layer(axum::middleware::from_fn_with_state(state.clone(), crate::opds::basic_auth))
		.with_state(state)
}

fn feed_response(feed: &Value) -> Response {
	Response::builder()
		.status(StatusCode::OK)
		.header(header::CONTENT_TYPE, FEED)
		.body(Body::from(feed.to_string()))
		.expect("feed response")
}

/// Every feed carries start/search, so the client can return to the root or start a search from any level
fn feed_links(base: &str, self_href: &str, next_href: Option<&str>) -> Vec<Value> {
	let mut links = vec![
		json!({"rel": "self", "href": self_href, "type": FEED}),
		json!({"rel": "start", "href": format!("{base}/opds/v2/root"), "type": FEED}),
		json!({"rel": "search", "href": format!("{base}/opds/v2/search{{?query}}"), "type": FEED, "templated": true}),
	];
	if let Some(next) = next_href {
		links.push(json!({"rel": "next", "href": next, "type": FEED}));
	}
	links
}

fn publications_feed(
	base: &str,
	title: &str,
	self_href: &str,
	next_href: Option<&str>,
	page: i64,
	publications: Vec<Value>,
) -> Response {
	feed_response(&json!({
		"metadata": {"title": title, "currentPage": page, "itemsPerPage": PAGE_SIZE},
		"links": feed_links(base, self_href, next_href),
		"publications": publications,
	}))
}

fn navigation_feed(base: &str, title: &str, self_href: &str, navigation: Vec<Value>) -> Response {
	feed_response(&json!({
		"metadata": {"title": title},
		"links": feed_links(base, self_href, None),
		"navigation": navigation,
	}))
}

fn nav_link(base: &str, title: &str, path: &str) -> Value {
	json!({"href": format!("{base}/opds/v2/{path}"), "title": title, "type": FEED})
}

fn manga_publication(base: &str, row: &MangaRow, modified: i64) -> Value {
	let mut metadata = json!({
		"@type": "http://schema.org/Book",
		"identifier": format!("urn:sumidoku:manga:{}", row.id),
		"title": row.title,
		"modified": rfc3339(modified),
	});
	let obj = metadata.as_object_mut().expect("metadata object");
	if !row.authors.0.is_empty() {
		obj.insert("author".into(), json!(row.authors.0));
	}
	if !row.artists.0.is_empty() {
		obj.insert("artist".into(), json!(row.artists.0));
	}
	if let Some(description) = &row.description {
		obj.insert("description".into(), json!(description));
	}
	json!({
		"metadata": metadata,
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/series/{}", row.id), "type": FEED}],
		"images": [{"href": format!("{base}/opds/v1.2/cover/{}", row.id), "type": "image/jpeg"}],
	})
}

fn chapter_publication(base: &str, row: &OpdsChapterRow) -> Value {
	let c = &row.chapter;
	let mut publication = json!({
		"metadata": {
			"@type": "http://schema.org/Book",
			"identifier": format!("urn:sumidoku:chapter:{}", c.id),
			"title": chapter_title(c.number, c.title.as_deref(), &c.key),
			"modified": rfc3339(c.date_uploaded.unwrap_or(c.fetched_at)),
			"belongsTo": {"series": {"name": row.manga_title, "links": [
				{"href": format!("{base}/opds/v2/series/{}", c.manga_id), "type": FEED},
			]}},
		},
	});
	let obj = publication.as_object_mut().expect("publication object");
	// An undownloaded chapter has no deliverable artifact, so no acquisition link can be given; leaving it out beats pointing at nothing
	if row.download_path.as_deref().is_some_and(|p| !p.is_empty()) {
		obj.insert(
			"links".into(),
			json!([{
				"rel": "http://opds-spec.org/acquisition",
				"type": "application/zip",
				"href": format!("{base}/opds/v1.2/file/{}", c.id),
			}]),
		);
	}
	if c.page_count > 0 {
		obj.insert(
			"images".into(),
			json!([{"href": format!("{base}/opds/v1.2/stream/{}/0", c.id), "type": "image/jpeg"}]),
		);
	}
	publication
}

async fn root_feed(headers: HeaderMap) -> Response {
	let base = base_url(&headers);
	let navigation = [
		("Library", "library"),
		("Categories", "categories"),
		("Sources", "sources"),
		("History", "history"),
		("Updates", "updates"),
	]
	.into_iter()
	.map(|(title, path)| nav_link(&base, title, path))
	.collect();
	let self_href = format!("{base}/opds/v2/root");
	navigation_feed(&base, "Sumidoku", &self_href, navigation)
}

async fn library_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(user.id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| {
			let modified = row.manga.last_fetched_at.or(row.in_library_at).unwrap_or(0);
			manga_publication(&base, &row.manga, modified)
		})
		.collect();
	let self_href = format!("{base}/opds/v2/library?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/library?page={}", page + 1));
	Ok(publications_feed(&base, "Library", &self_href, next_href.as_deref(), page, publications))
}

async fn categories_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let base = base_url(&headers);
	let rows: Vec<(i64, String)> =
		sqlx::query_as("SELECT id, name FROM category WHERE user_id = ? ORDER BY sort_order ASC, id ASC")
			.bind(user.id)
			.fetch_all(&state.pool)
			.await?;
	let navigation = rows
		.into_iter()
		.map(|(id, name)| nav_link(&base, &name, &format!("categories/{id}")))
		.collect();
	let self_href = format!("{base}/opds/v2/categories");
	Ok(navigation_feed(&base, "Categories", &self_href, navigation))
}

async fn category_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let category: Option<(String,)> =
		sqlx::query_as("SELECT name FROM category WHERE id = ? AND user_id = ?")
			.bind(id)
			.bind(user.id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((name,)) = category else {
		return Err(ApiError::not_found("category not found"));
	};
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} AND EXISTS (SELECT 1 FROM category_manga cm
		 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id = ?)
		 ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(user.id)
	.bind(user.id)
	.bind(id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| {
			let modified = row.manga.last_fetched_at.or(row.in_library_at).unwrap_or(0);
			manga_publication(&base, &row.manga, modified)
		})
		.collect();
	let self_href = format!("{base}/opds/v2/categories/{id}?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/categories/{id}?page={}", page + 1));
	Ok(publications_feed(&base, &name, &self_href, next_href.as_deref(), page, publications))
}

async fn sources_feed(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
	let base = base_url(&headers);
	let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM source ORDER BY id ASC")
		.fetch_all(&state.pool)
		.await?;
	let navigation = rows
		.into_iter()
		.map(|(id, name)| nav_link(&base, &name, &format!("sources/{id}")))
		.collect();
	let self_href = format!("{base}/opds/v2/sources");
	Ok(navigation_feed(&base, "Sources", &self_href, navigation))
}

async fn source_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
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
	.bind(user.id)
	.bind(id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| {
			let modified = row.manga.last_fetched_at.or(row.in_library_at).unwrap_or(0);
			manga_publication(&base, &row.manga, modified)
		})
		.collect();
	let self_href = format!("{base}/opds/v2/sources/{id}?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/sources/{id}?page={}", page + 1));
	Ok(publications_feed(&base, &name, &self_href, next_href.as_deref(), page, publications))
}

async fn history_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
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
	.bind(user.id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| manga_publication(&base, &row.manga, row.last_read_at.unwrap_or(0)))
		.collect();
	let self_href = format!("{base}/opds/v2/history?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/history?page={}", page + 1));
	Ok(publications_feed(&base, "History", &self_href, next_href.as_deref(), page, publications))
}

async fn updates_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let base = base_url(&headers);
	let rows: Vec<OpdsChapterRow> = sqlx::query_as(&format!(
		"{CHAPTER_SELECT}
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = ? AND mu.in_library = 1
		 ORDER BY c.fetched_at DESC, c.id DESC LIMIT ? OFFSET ?"
	))
	.bind(user.id)
	.bind(user.id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| chapter_publication(&base, row))
		.collect();
	let self_href = format!("{base}/opds/v2/updates?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/updates?page={}", page + 1));
	Ok(publications_feed(&base, "Updates", &self_href, next_href.as_deref(), page, publications))
}

async fn search_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<FeedQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let page = page_of(&query)?;
	let q = query.q.clone().unwrap_or_default();
	let base = base_url(&headers);
	let rows: Vec<LibraryEntryRow> = sqlx::query_as(&format!(
		"{LIBRARY_SELECT} AND m.title LIKE ? ESCAPE '\\' ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
	))
	.bind(user.id)
	.bind(like_pattern(&q))
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| {
			let modified = row.manga.last_fetched_at.or(row.in_library_at).unwrap_or(0);
			manga_publication(&base, &row.manga, modified)
		})
		.collect();
	let enc_q = url_query_escape(&q);
	let self_href = format!("{base}/opds/v2/search?q={enc_q}&page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/search?q={enc_q}&page={}", page + 1));
	Ok(publications_feed(
		&base,
		&format!("Search: {q}"),
		&self_href,
		next_href.as_deref(),
		page,
		publications,
	))
}

async fn series_feed(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
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
	.bind(user.id)
	.bind(manga_id)
	.bind(PAGE_SIZE + 1)
	.bind((page - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next = rows.len() as i64 > PAGE_SIZE;
	let publications = rows
		.iter()
		.take(PAGE_SIZE as usize)
		.map(|row| chapter_publication(&base, row))
		.collect();
	let self_href = format!("{base}/opds/v2/series/{manga_id}?page={page}");
	let next_href = has_next.then(|| format!("{base}/opds/v2/series/{manga_id}?page={}", page + 1));
	Ok(publications_feed(
		&base,
		&manga.title,
		&self_href,
		next_href.as_deref(),
		page,
		publications,
	))
}
