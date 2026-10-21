//! Filesystem side of the built-in local source: walks local_dir live (no index table; rows land in the DB only on open/shelve).
//!
//! Manga and chapter keys are validated relative paths (`valid_key`): nested layouts address their target from
//! local_dir / the manga directory (`publisher/series`, `vol1/ch01`), while `..`, absolute and drive-prefixed forms
//! stay rejected so API input can never escape local_dir. Discovery is top-down with nearest-qualifying-ancestor
//! absorption: a directory directly holding chapter archives qualifies (pass 1, the Komga trigger), one with no
//! archive anywhere below qualifies by the backward-compatible relaxed rule (pass 2, nested levels included),
//! anything else is a pure container and discovery descends. A claimed manga absorbs its subtree: archives and
//! image-leaf directories (direct image files) at any depth become chapters keyed by their path relative to the
//! manga, never manga of their own — so a flat single-level library scans byte-identically to before.
//!
//! Failed-entry protection (borrowed from kmrs): entries that fail to read are recorded in `failed` by full relative
//! path and the caller carries them into the reconcile protection set, where a failed container prefix-protects the
//! subtree that could not be walked; a scan that fails midway errors out outright — reconciling against half a
//! listing would mistake unscanned chapters for deleted ones.

pub mod chapter_number;
pub mod comic_info;
pub mod natural_sort;
pub mod zip_cache;

pub use zip_cache::ZipCache;

use crate::error::{ApiError, ApiResult};
use aidoku::{Chapter, Manga, MangaStatus};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];
const ARCHIVE_EXTS: &[&str] = &["cbz", "zip"];

/// The walk follows directory symlinks (flat-layout symlink chapters predate nesting), so a symlink cycle would
/// never terminate without a component budget; real libraries nest a handful of levels
const MAX_KEY_DEPTH: usize = 32;

/// One chapter scanned by the local source: key is the path relative to the manga directory (archive extension
/// included), name is the display name (the same path, archives sans extension)
pub struct LocalChapter {
	pub key: String,
	pub name: String,
	pub number: Option<f32>,
	pub date_uploaded: Option<i64>,
	pub is_archive: bool,
}

/// Result of manga_update: the source-side Manga snapshot + chapter paths (relative to the manga directory) that
/// failed to read (reconcile protection set)
pub struct LocalUpdate {
	pub manga: Manga,
	pub protected: Vec<String>,
}

/// Manga/chapter keys are relative paths: API input can be anything, and without this guard it would escape
/// local_dir. Every '/'-separated segment must be a real component (non-empty, not . or ..); backslash, a leading /
/// (empty first segment) and the drive-letter form (C:) are rejected so the key can never re-anchor the path.
fn valid_key(key: &str) -> bool {
	if key.is_empty() || key.contains('\\') {
		return false;
	}
	for (index, segment) in key.split('/').enumerate() {
		if segment.is_empty() || segment == "." || segment == ".." {
			return false;
		}
		let bytes = segment.as_bytes();
		if index == 0 && bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
			return false;
		}
	}
	true
}

fn require_key(key: &str) -> ApiResult<()> {
	if valid_key(key) {
		Ok(())
	} else {
		Err(ApiError::bad_request("invalid local path"))
	}
}

/// Page entries of an image chapter join onto the chapter directory, so unlike keys they must stay one component
fn valid_component(key: &str) -> bool {
	!key.is_empty() && key != "." && key != ".." && !key.contains(['/', '\\'])
}

fn require_component(key: &str) -> ApiResult<()> {
	if valid_component(key) {
		Ok(())
	} else {
		Err(ApiError::bad_request("invalid local path component"))
	}
}

/// The key form has no leading component for the walked root; joining a child name keeps the '/'-separated form
fn join_rel(rel: &str, name: &str) -> String {
	if rel.is_empty() {
		name.to_string()
	} else {
		format!("{rel}/{name}")
	}
}

fn has_ext(name: &str, exts: &[&str]) -> bool {
	name.rsplit('.')
		.next()
		.is_some_and(|ext| exts.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// mime for page images/covers: static mapping by extension, the same set of types as the online-page mime_for
pub fn image_mime(path: &str) -> &'static str {
	let ext = path
		.rsplit('.')
		.next()
		.map(str::to_ascii_lowercase)
		.unwrap_or_default();
	match ext.as_str() {
		"jpg" | "jpeg" => "image/jpeg",
		"png" => "image/png",
		"webp" => "image/webp",
		"gif" => "image/gif",
		"bmp" => "image/bmp",
		"avif" => "image/avif",
		_ => "application/octet-stream",
	}
}

fn io_scan_err(e: &std::io::Error) -> ApiError {
	if e.kind() == std::io::ErrorKind::NotFound {
		ApiError::not_found("local manga not found")
	} else {
		ApiError::internal(format!("read local directory failed: {e}"))
	}
}

/// One directory child, classified once for both manga discovery and chapter collection
struct DirItem {
	name: String,
	is_dir: bool,
	is_archive: bool,
	is_image: bool,
	date_uploaded: Option<i64>,
}

async fn open_dir(dir: &Path) -> ApiResult<tokio::fs::ReadDir> {
	tokio::fs::read_dir(dir).await.map_err(|e| io_scan_err(&e))
}

/// Lists one directory's children. Unreadable entries degrade into `failed` by full relative path (rel is the listed
/// directory's key form); iterating the directory errors the scan — reconciling against half a listing would mistake
/// unscanned chapters for deleted ones.
async fn read_items(read_dir: &mut tokio::fs::ReadDir, rel: &str, failed: &mut Vec<String>) -> ApiResult<Vec<DirItem>> {
	let mut seen = HashSet::new();
	let mut out = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::internal(format!("read local directory failed: {e}")))?
	{
		let name = entry.file_name().to_string_lossy().into_owned();
		if name.starts_with('.') || name == "@eaDir" || !seen.insert(name.clone()) {
			continue;
		}
		// DirEntry::metadata does not follow symlinks; stat by path does, so a dangling link lands in failed —
		// exactly the failed → protected per-entry failed-directory protection path
		let metadata = match tokio::fs::metadata(entry.path()).await {
			Ok(metadata) => metadata,
			Err(e) => {
				tracing::warn!(dir = rel, name, "skip unreadable local entry: {e}");
				failed.push(join_rel(rel, &name));
				continue;
			}
		};
		let date_uploaded = metadata
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map(|d| d.as_secs() as i64);
		out.push(DirItem {
			is_dir: metadata.is_dir(),
			is_archive: metadata.is_file() && has_ext(&name, ARCHIVE_EXTS),
			is_image: metadata.is_file() && has_ext(&name, IMAGE_EXTS),
			name,
			date_uploaded,
		});
	}
	Ok(out)
}

/// Browse entry point: discovers manga under the local root by the module-doc classification contract; a missing
/// local root is an empty listing, not an error. PR-2's browse/refresh buttons call this layer directly
pub async fn scan_manga_dirs(local_dir: &Path) -> ApiResult<Vec<String>> {
	let mut read_dir = match tokio::fs::read_dir(local_dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(e) => return Err(io_scan_err(&e)),
	};
	// browse has no reconcile protection set: failures only narrow the listing
	let mut failed = Vec::new();
	let first = read_items(&mut read_dir, "", &mut failed).await?;
	let mut out = Vec::new();
	for item in first.iter().filter(|item| item.is_dir) {
		if let Some(items) = classify_items(local_dir, &item.name).await? {
			discover(local_dir, item.name.clone(), items, &mut out).await?;
		}
	}
	out.sort_by(|a, b| natural_sort::compare(a, b));
	Ok(out)
}

type DiscoverFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ApiResult<()>> + Send + 'a>>;

/// Lists one directory for classification; a directory that cannot be opened is unclassifiable (None): never
/// claimed, while the pass-2 predicate keeps its ancestors from absorbing what it may hide
async fn classify_items(local_dir: &Path, rel: &str) -> ApiResult<Option<Vec<DirItem>>> {
	let mut read_dir = match tokio::fs::read_dir(local_dir.join(rel)).await {
		Ok(read_dir) => read_dir,
		Err(e) => {
			tracing::warn!(rel, "skip unreadable local directory: {e}");
			return Ok(None);
		}
	};
	let mut failed = Vec::new();
	read_items(&mut read_dir, rel, &mut failed).await.map(Some)
}

/// Whether any archive exists below the directory holding `items` (the pass-2 trigger). Directories that cannot be
/// inspected — unreadable or past the depth budget — answer true: a subtree that may hide archives must never be
/// pass-2-absorbed by an ancestor.
async fn subtree_has_archive(local_dir: &Path, rel: &str, items: &[DirItem]) -> ApiResult<bool> {
	let mut stack: Vec<String> = items
		.iter()
		.filter(|item| item.is_dir)
		.map(|item| join_rel(rel, &item.name))
		.collect();
	while let Some(rel) = stack.pop() {
		if rel.matches('/').count() >= MAX_KEY_DEPTH {
			return Ok(true);
		}
		let Some(items) = classify_items(local_dir, &rel).await? else {
			return Ok(true);
		};
		if items.iter().any(|item| item.is_archive) {
			return Ok(true);
		}
		stack.extend(
			items
				.iter()
				.filter(|item| item.is_dir)
				.map(|item| join_rel(&rel, &item.name)),
		);
	}
	Ok(false)
}

/// Classifies one directory whose listing is `items`: pass 1 claims it when it directly holds archives, pass 2 when
/// no archive exists anywhere below; a claimed directory absorbs its subtree, so children are classified only when
/// it qualifies under neither rule (a pure container).
fn discover<'a>(local_dir: &'a Path, rel: String, items: Vec<DirItem>, out: &'a mut Vec<String>) -> DiscoverFuture<'a> {
	Box::pin(async move {
		if items.iter().any(|item| item.is_archive) || !subtree_has_archive(local_dir, &rel, &items).await? {
			out.push(rel);
			return Ok(());
		}
		for item in items.iter().filter(|item| item.is_dir) {
			let child = join_rel(&rel, &item.name);
			if child.matches('/').count() >= MAX_KEY_DEPTH {
				tracing::warn!(rel = child, "skip over-deep local directory");
				continue;
			}
			if let Some(child_items) = classify_items(local_dir, &child).await? {
				discover(local_dir, child, child_items, out).await?;
			}
		}
		Ok(())
	})
}

/// Chapter scan of a single manga: walks the manga's whole subtree — archives and image-leaf directories (direct
/// image files) at any depth become chapters keyed by their path relative to the manga directory; directories
/// holding only subdirectories are containers, not chapters. Returns the sorted chapter list plus the failed entries
/// by full relative path. Sort order matches Suwayomi LocalSource: chapter number descending (unparsed counts as -1
/// and sinks to the bottom), ties broken by display name in descending natural order
pub async fn scan_chapters(local_dir: &Path, manga_key: &str) -> ApiResult<(Vec<LocalChapter>, Vec<String>)> {
	require_key(manga_key)?;
	let mut chapters = Vec::new();
	let mut failed = Vec::new();
	let mut read_dir = open_dir(&local_dir.join(manga_key)).await?;
	let first = read_items(&mut read_dir, "", &mut failed).await?;
	collect_chapters(local_dir, manga_key, first, &mut chapters, &mut failed).await?;
	chapters.sort_by(|a, b| {
		b.number
			.unwrap_or(-1.0)
			.total_cmp(&a.number.unwrap_or(-1.0))
			.then_with(|| natural_sort::compare(&b.name, &a.name))
	});
	Ok((chapters, failed))
}

/// Worklist walk of the manga subtree: every directory is listed once — becoming an image-leaf chapter when it
/// directly holds image files — and its subdirectories queue for the same treatment. A directory that cannot be
/// listed degrades into `failed`, where the reconcile prefix rule protects everything known below it.
async fn collect_chapters(
	local_dir: &Path,
	manga_key: &str,
	first: Vec<DirItem>,
	chapters: &mut Vec<LocalChapter>,
	failed: &mut Vec<String>,
) -> ApiResult<()> {
	let mut stack = vec![(String::new(), first)];
	while let Some((rel, items)) = stack.pop() {
		for item in items {
			let key = join_rel(&rel, &item.name);
			if item.is_archive {
				let display = key
					.rsplit_once('.')
					.map(|(stem, _)| stem.to_string())
					.unwrap_or_else(|| key.clone());
				chapters.push(LocalChapter {
					number: chapter_number::parse_chapter_number(&display),
					key,
					name: display,
					date_uploaded: item.date_uploaded,
					is_archive: true,
				});
				continue;
			}
			if !item.is_dir {
				continue;
			}
			let mut child_dir = match open_dir(&local_dir.join(manga_key).join(&key)).await {
				Ok(child_dir) => child_dir,
				Err(e) => {
					tracing::warn!(dir = %key, "skip unreadable local directory: {}", e.message());
					failed.push(key);
					continue;
				}
			};
			let child_items = read_items(&mut child_dir, &key, failed).await?;
			if child_items.iter().any(|child| child.is_image) {
				chapters.push(LocalChapter {
					number: chapter_number::parse_chapter_number(&key),
					key: key.clone(),
					name: key.clone(),
					date_uploaded: item.date_uploaded,
					is_archive: false,
				});
			}
			if key.matches('/').count() >= MAX_KEY_DEPTH {
				// past the depth budget the subtree is not walked; recording the directory keeps it protected
				failed.push(key);
			} else {
				stack.push((key, child_items));
			}
		}
	}
	Ok(())
}

fn to_aidoku_chapter(chapter: &LocalChapter) -> Chapter {
	Chapter {
		key: chapter.key.clone(),
		title: Some(chapter.name.clone()),
		chapter_number: chapter.number,
		volume_number: None,
		date_uploaded: chapter.date_uploaded,
		scanlators: None,
		url: None,
		language: None,
		thumbnail: None,
		locked: false,
	}
}

/// The local counterpart of "fetch details + chapters from the source": scans the manga directory and produces a
/// Manga snapshot (including chapters) of the same shape as get_manga_update, for upsert_manga + reconcile_chapters to reuse
pub async fn manga_update(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str) -> ApiResult<LocalUpdate> {
	let (chapters, failed) = scan_chapters(local_dir, manga_key).await?;
	// the title is the leaf name: the full key would repeat the container path on every nested row; ComicInfo still overrides it below
	let mut manga = Manga {
		key: manga_key.to_string(),
		title: manga_key.rsplit('/').next().unwrap_or(manga_key).to_string(),
		..Manga::default()
	};
	if let Some(info) = read_comic_info(local_dir, zip, manga_key, &chapters).await {
		if let Some(title) = info.title() {
			manga.title = title.to_string();
		}
		manga.authors = info.authors();
		manga.artists = info.artists();
		manga.description = info.description();
		manga.tags = info.tags();
		manga.status = match info.status() {
			1 => MangaStatus::Ongoing,
			2 => MangaStatus::Completed,
			3 => MangaStatus::Cancelled,
			4 => MangaStatus::Hiatus,
			_ => MangaStatus::Unknown,
		};
	}
	manga.chapters = Some(chapters.iter().map(to_aidoku_chapter).collect());
	Ok(LocalUpdate {
		manga,
		protected: failed,
	})
}

/// ComicInfo read order, same as Suwayomi: the manga directory's top level wins; if the top-level file exists it is
/// authoritative (even when parsing fails) and archives are not consulted;
/// otherwise extract from the first archive with a ComicInfo.xml in chapter order (parsed in memory only, never written back to the user's directory)
async fn read_comic_info(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str, chapters: &[LocalChapter]) -> Option<comic_info::ComicInfo> {
	let top_level = local_dir.join(manga_key).join(comic_info::COMIC_INFO_FILE);
	match tokio::fs::read(&top_level).await {
		Ok(bytes) => return comic_info::parse(&bytes),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
		Err(e) => {
			tracing::warn!(path = %top_level.display(), "read ComicInfo.xml failed: {e}");
			return None;
		}
	}
	for chapter in chapters.iter().filter(|c| c.is_archive) {
		let path = local_dir.join(manga_key).join(&chapter.key);
		let read = {
			let zip = zip.clone();
			tokio::task::spawn_blocking(move || zip.read_entry(&path, comic_info::COMIC_INFO_FILE)).await
		};
		match read {
			Ok(Ok(bytes)) => return comic_info::parse(&bytes),
			Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
			Ok(Err(e)) => {
				tracing::warn!(chapter = %chapter.key, "read archive ComicInfo.xml failed: {e}");
				continue;
			}
			Err(e) => {
				tracing::warn!(chapter = %chapter.key, "archive ComicInfo.xml task failed: {e}");
				continue;
			}
		}
	}
	None
}

/// Chapter page listing: image chapters list image files, archive chapters list image entries; both in case-insensitive natural order
pub async fn chapter_pages(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str, chapter_key: &str) -> ApiResult<Vec<String>> {
	require_key(manga_key)?;
	require_key(chapter_key)?;
	let path = local_dir.join(manga_key).join(chapter_key);
	let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
		if e.kind() == std::io::ErrorKind::NotFound {
			ApiError::not_found("local chapter not found")
		} else {
			ApiError::internal(format!("read local chapter failed: {e}"))
		}
	})?;
	if metadata.is_dir() {
		let mut read_dir = tokio::fs::read_dir(&path)
			.await
			.map_err(|e| ApiError::internal(format!("read local chapter failed: {e}")))?;
		let mut pages = Vec::new();
		while let Some(entry) = read_dir
			.next_entry()
			.await
			.map_err(|e| ApiError::internal(format!("read local chapter failed: {e}")))?
		{
			let name = entry.file_name().to_string_lossy().into_owned();
			if name.starts_with('.') {
				continue;
			}
			let Ok(meta) = tokio::fs::metadata(entry.path()).await else {
				continue;
			};
			if meta.is_file() && has_ext(&name, IMAGE_EXTS) {
				pages.push(name);
			}
		}
		pages.sort_by(|a, b| natural_sort::compare(a, b));
		return Ok(pages);
	}
	if metadata.is_file() && has_ext(chapter_key, ARCHIVE_EXTS) {
		let names = {
			let path = path.clone();
			let zip = zip.clone();
			tokio::task::spawn_blocking(move || zip.entry_names(&path))
				.await
				.map_err(|e| ApiError::internal(format!("list zip entries failed: {e}")))?
				.map_err(|e| ApiError::internal(format!("read local archive failed: {e}")))?
		};
		let mut pages: Vec<String> = names.into_iter().filter(|n| is_image_entry(n)).collect();
		pages.sort_by(|a, b| natural_sort::compare(a, b));
		return Ok(pages);
	}
	Err(ApiError::bad_request("unsupported local chapter format"))
}

/// Image entries inside an archive: directory entries, dot-prefixed names, and __MACOSX resource forks are not pages (the opds source's CBZ page splitting shares this rule)
pub(crate) fn is_image_entry(name: &str) -> bool {
	!name.ends_with('/')
		&& !name.split('/').any(|part| part.starts_with('.') || part == "__MACOSX")
		&& has_ext(name, IMAGE_EXTS)
}

/// Reads one page's bytes: image chapters read a single file, archive chapters extract by entry name (handles come from the LRU)
pub async fn read_page(
	local_dir: &Path,
	zip: &Arc<ZipCache>,
	manga_key: &str,
	chapter_key: &str,
	entry: &str,
) -> ApiResult<Vec<u8>> {
	require_key(manga_key)?;
	require_key(chapter_key)?;
	let chapter_path = local_dir.join(manga_key).join(chapter_key);
	let metadata = tokio::fs::metadata(&chapter_path)
		.await
		.map_err(|_| ApiError::not_found("local chapter not found"))?;
	if metadata.is_dir() {
		require_component(entry)?;
		return tokio::fs::read(chapter_path.join(entry))
			.await
			.map_err(|_| ApiError::not_found("page not found"));
	}
	let entry = entry.to_string();
	let zip = zip.clone();
	tokio::task::spawn_blocking(move || zip.read_entry(&chapter_path, &entry))
		.await
		.map_err(|e| ApiError::internal(format!("read zip entry failed: {e}")))?
		.map_err(|e| {
			if e.kind() == std::io::ErrorKind::NotFound {
				ApiError::not_found("page not found")
			} else {
				ApiError::internal(format!("read local archive failed: {e}"))
			}
		})
}

/// Cover fallback chain (Suwayomi semantics): a cover.* sidecar in the manga directory → the first page of the last
/// chapter in the list (descending by number, so the last is the oldest chapter) → None (the caller answers 404 and the UI shows a placeholder)
pub async fn resolve_cover(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str) -> ApiResult<Option<(Vec<u8>, String)>> {
	require_key(manga_key)?;
	let dir = local_dir.join(manga_key);
	let mut read_dir = match tokio::fs::read_dir(&dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(e) => return Err(io_scan_err(&e)),
	};
	let mut sidecars = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::internal(format!("read local directory failed: {e}")))?
	{
		let name = entry.file_name().to_string_lossy().into_owned();
		let is_cover = name
			.rsplit_once('.')
			.is_some_and(|(stem, _)| stem.eq_ignore_ascii_case("cover"));
		if name.starts_with('.') || !is_cover || !has_ext(&name, IMAGE_EXTS) {
			continue;
		}
		let Ok(meta) = tokio::fs::metadata(entry.path()).await else {
			continue;
		};
		if meta.is_file() {
			sidecars.push(name);
		}
	}
	sidecars.sort_by(|a, b| natural_sort::compare(a, b));
	if let Some(name) = sidecars.into_iter().next() {
		let bytes = tokio::fs::read(dir.join(&name))
			.await
			.map_err(|e| ApiError::internal(format!("read local cover failed: {e}")))?;
		return Ok(Some((bytes, image_mime(&name).to_string())));
	}
	let (chapters, _) = scan_chapters(local_dir, manga_key).await?;
	let Some(last) = chapters.last() else {
		return Ok(None);
	};
	let pages = chapter_pages(local_dir, zip, manga_key, &last.key).await?;
	let Some(first) = pages.first() else {
		return Ok(None);
	};
	let bytes = read_page(local_dir, zip, manga_key, &last.key, first).await?;
	Ok(Some((bytes, image_mime(first).to_string())))
}

/// The Local variant of page-content JSON: the value is the page entry name within the chapter (image chapter's file name / archive entry path)
pub fn page_content_json(entry: &str) -> Value {
	serde_json::json!({
		"content": { "Local": entry },
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	})
}
