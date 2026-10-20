//! Filesystem side of the built-in local source: walks local_dir live (no index table; rows land in the DB only on open/shelve),
//! chapter convention = a subdirectory of the manga directory (image chapter) or a single *.cbz/*.zip.
//! Failed-entry protection (borrowed from kmrs): entries that fail to read are recorded in `failed` and the caller carries them into the reconcile protection set;
//! a scan that fails midway errors out outright — reconciling against half a listing would mistake unscanned chapters for deleted ones.

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

/// One chapter scanned by the local source: key is the full entry name (archive extension included), name is the display name
pub struct LocalChapter {
	pub key: String,
	pub name: String,
	pub number: Option<f32>,
	pub date_uploaded: Option<i64>,
	pub is_archive: bool,
}

/// Result of manga_update: the source-side Manga snapshot + chapter entry names that failed to read (reconcile protection set)
pub struct LocalUpdate {
	pub manga: Manga,
	pub protected: Vec<String>,
}

/// key must be exactly one path component: API input can be anything, and without this guard it would escape local_dir
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

/// Browse entry point (actually scans the local root): manga = first-level subdirectories; dot-prefixed, @eaDir, and
/// unreadable entries are skipped; deduped by directory name. PR-2's browse/refresh buttons call this layer directly
pub async fn scan_manga_dirs(local_dir: &Path) -> ApiResult<Vec<String>> {
	let mut read_dir = match tokio::fs::read_dir(local_dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(e) => return Err(io_scan_err(&e)),
	};
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
		// DirEntry::metadata does not follow symlinks (a dangling link still yields the link itself);
		// matching upstream isDirectory semantics requires stat by path
		let Ok(metadata) = tokio::fs::metadata(entry.path()).await else {
			continue;
		};
		if metadata.is_dir() {
			out.push(name);
		}
	}
	out.sort_by(|a, b| natural_sort::compare(a, b));
	Ok(out)
}

/// Chapter scan of a single manga: returns the sorted chapter list and the entry names that failed to read.
/// Sort order matches Suwayomi LocalSource: chapter number descending (unparsed counts as -1 and sinks to the bottom), ties broken by display name in descending natural order
pub async fn scan_chapters(local_dir: &Path, manga_key: &str) -> ApiResult<(Vec<LocalChapter>, Vec<String>)> {
	require_component(manga_key)?;
	let dir = local_dir.join(manga_key);
	let mut read_dir = tokio::fs::read_dir(&dir).await.map_err(|e| io_scan_err(&e))?;
	let mut chapters = Vec::new();
	let mut failed = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::internal(format!("read local directory failed: {e}")))?
	{
		let name = entry.file_name().to_string_lossy().into_owned();
		if name.starts_with('.') || name == "@eaDir" {
			continue;
		}
		// Same as scan_manga_dirs: symlinks must be followed (stat fails when a dangling link's target is gone —
		// exactly the failed → protected per-entry failed-directory protection path)
		let metadata = match tokio::fs::metadata(entry.path()).await {
			Ok(metadata) => metadata,
			Err(e) => {
				tracing::warn!(dir = %dir.display(), name, "skip unreadable local entry: {e}");
				failed.push(name);
				continue;
			}
		};
		let is_archive = metadata.is_file() && has_ext(&name, ARCHIVE_EXTS);
		if !metadata.is_dir() && !is_archive {
			continue;
		}
		let display = if is_archive {
			name.rsplit_once('.')
				.map(|(stem, _)| stem.to_string())
				.unwrap_or_else(|| name.clone())
		} else {
			name.clone()
		};
		let date_uploaded = metadata
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map(|d| d.as_secs() as i64);
		chapters.push(LocalChapter {
			number: chapter_number::parse_chapter_number(&display),
			key: name,
			name: display,
			date_uploaded,
			is_archive,
		});
	}
	chapters.sort_by(|a, b| {
		b.number
			.unwrap_or(-1.0)
			.total_cmp(&a.number.unwrap_or(-1.0))
			.then_with(|| natural_sort::compare(&b.name, &a.name))
	});
	Ok((chapters, failed))
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
	let mut manga = Manga {
		key: manga_key.to_string(),
		title: manga_key.to_string(),
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
	require_component(manga_key)?;
	require_component(chapter_key)?;
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
	require_component(manga_key)?;
	require_component(chapter_key)?;
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
	require_component(manga_key)?;
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
