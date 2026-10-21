//! Filesystem side of the built-in local source: walks the local roots live (no index table; rows land in the DB only on open/shelve).
//!
//! The source has one or more root directories (config local_dirs, overridable at runtime by the local.dirs
//! setting — see state.rs). Manga and chapter keys are validated relative paths (`valid_key`): nested layouts
//! address their target from the owning root / the manga directory (`publisher/series`, `vol1/ch01`), while `..`,
//! absolute and drive-prefixed forms stay rejected so API input can never escape the roots. Across roots the key
//! carries the root index: a bare path addresses root 0 (the pre-multi-dir form, so rows persisted before
//! multiple roots keep resolving), and `N:<path>` addresses root N >= 1 (`0:` is rejected so a root-0 manga has
//! exactly one key form). Browsing unions the roots; the same relative path present in several roots is listed
//! once, from the lowest-indexed root (first directory wins — mirrors of the same content), while the shadowed
//! copies stay addressable by their numbered keys. One collision is inherent: a root-0 directory named like the
//! prefix form (`1:Foo`) resolves as root 1's `Foo` — don't name directories that way.
//!
//! Discovery is top-down with nearest-qualifying-ancestor
//! absorption: a directory directly holding chapter archives qualifies (pass 1, the Komga trigger), one with no
//! archive anywhere below qualifies by the backward-compatible relaxed rule (pass 2, nested levels included),
//! anything else is a pure container and discovery descends. A claimed manga absorbs its subtree: archives and
//! image-leaf directories (direct image files) at any depth become chapters keyed by their path relative to the
//! manga, never manga of their own — so a flat single-level library scans byte-identically to before.
//!
//! Archive chapters are CBZ/ZIP and divina-profile EPUB (image-based books; reflowable text is out of scope):
//! an EPUB takes its page list from the OPF spine restricted to image manifest items, falling back to the CBZ
//! rule (natural-sorted image entries) when the OPF is missing or unparseable (`local/epub.rs`).
//!
//! Manga metadata resolves by a first-hit-wins chain, no per-field merge (`read_manga_metadata`): the manga
//! directory's top-level ComicInfo.xml (authoritative when present, even unparsable) → the first chapter
//! archive carrying one, in chapter order → a series.json sidecar (flat schema; a broken file warns and
//! falls through) → the first EPUB chapter whose OPF parses, in chapter order.
//!
//! With a oneshots name configured (config local_oneshots_dir, overridable by the local.oneshots_dir setting),
//! the oneshots rule outranks both passes: a directory whose relative path contains the name (case-insensitive
//! substring, kmrs's ScanOptions::oneshots_dir) is never claimed — every archive directly inside becomes its
//! own one-chapter manga keyed by the archive path (`oneshots/Foo.cbz`) and titled by the file stem, and
//! nothing below the directory is picked up. Such a manga key names a file, not a directory: chapter listing,
//! pages and the cover treat it as the degenerate form whose only chapter is the archive itself (chapter key =
//! the leaf name), and series sidecars next to the archive (ComicInfo.xml, series.json, cover.*) are ignored,
//! so the cover is the single chapter's first page.
//!
//! Failed-entry protection (borrowed from kmrs): entries that fail to read are recorded in `failed` by full relative
//! path and the caller carries them into the reconcile protection set, where a failed container prefix-protects the
//! subtree that could not be walked; a scan that fails midway errors out outright — reconciling against half a
//! listing would mistake unscanned chapters for deleted ones.

pub mod chapter_number;
pub mod comic_info;
pub mod epub;
pub mod natural_sort;
pub mod series_json;
pub mod zip_cache;

pub use zip_cache::ZipCache;

use crate::error::{ApiError, ApiResult};
use aidoku::{Chapter, Manga, MangaStatus};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];
const ARCHIVE_EXTS: &[&str] = &["cbz", "zip", "epub"];

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

/// Parses the directory-index prefix on a key: `N:<path>` with N a canonical decimal (>= 1, no leading
/// zeros) and a non-empty path returns (N, path). `0:` never parses — a root-0 manga has exactly one key
/// form (the bare path), so no second spelling may become legal. A first segment whose colon head is not
/// all digits (`Note: 1`) is a plain root-0 name, not a prefix.
fn dir_prefix(key: &str) -> Option<(usize, &str)> {
	let (num, _) = key.split('/').next()?.split_once(':')?;
	if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
		return None;
	}
	let index: usize = num.parse().ok()?;
	if index == 0 || num != index.to_string() {
		return None;
	}
	let rel = &key[num.len() + 1..];
	if rel.is_empty() {
		return None;
	}
	Some((index, rel))
}

/// Manga/chapter keys are relative paths: API input can be anything, and without this guard it would escape
/// the local roots. Every '/'-separated segment must be a real component (non-empty, not . or ..); backslash,
/// a leading / (empty first segment) and the drive-letter form (C:) are rejected so the key can never
/// re-anchor the path. The first segment may carry the directory-index prefix (`N:<path>`, canonical form
/// only), with the path after it held to the same rules.
fn valid_key(key: &str) -> bool {
	if key.is_empty() || key.contains('\\') {
		return false;
	}
	for (index, segment) in key.split('/').enumerate() {
		if segment.is_empty() || segment == "." || segment == ".." {
			return false;
		}
		if index == 0
			&& let Some((num, _)) = segment.split_once(':')
		{
			if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
				// the digits-colon slot is reserved for the directory prefix: only the canonical form passes
				return dir_prefix(key).is_some_and(|(_, rel)| valid_key(rel));
			}
			let bytes = segment.as_bytes();
			if bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
				return false;
			}
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

/// Resolves an API-facing manga key to its root directory and the path relative to it: a bare key addresses
/// root 0, `N:<path>` root N; an out-of-range index is a not found, not a bad request
pub fn resolve_key<'a>(local_dirs: &'a [PathBuf], key: &'a str) -> ApiResult<(&'a Path, &'a str)> {
	require_key(key)?;
	let (index, rel) = dir_prefix(key).unwrap_or((0, key));
	let dir = local_dirs
		.get(index)
		.ok_or_else(|| ApiError::not_found("local manga not found"))?;
	Ok((dir, rel))
}

/// The path part of a manga key (its directory prefix stripped): titles and ComicInfo display names come
/// from the relative path, never from the `N:` slot
pub(crate) fn rel_of(key: &str) -> &str {
	dir_prefix(key).map_or(key, |(_, rel)| rel)
}

/// Display title of a manga key: the leaf segment of the relative path; a key naming an archive (a oneshot
/// from the oneshots directory) drops the extension, so the title is the file stem
pub fn title_of(key: &str) -> String {
	let rel = rel_of(key);
	let leaf = rel.rsplit('/').next().unwrap_or(rel);
	match leaf.rsplit_once('.') {
		Some((stem, ext)) if !stem.is_empty() && ARCHIVE_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) => stem.to_string(),
		_ => leaf.to_string(),
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

/// Browse entry point: discovers manga under every local root by the module-doc classification contract and
/// unions them into one listing. A relative path found in several roots lists once, from the lowest-indexed
/// root (first directory wins); keys from root N >= 1 carry the `N:` prefix, root 0 keeps the bare form.
/// A missing root is an empty contribution, not an error. PR-2's browse/refresh buttons call this layer directly
pub async fn scan_manga_dirs(local_dirs: &[PathBuf], oneshots: Option<&str>) -> ApiResult<Vec<String>> {
	// lowercased once here; discover lowercases each candidate relative path before the substring test
	let oneshots = oneshots.map(str::to_lowercase);
	let mut seen = HashSet::new();
	let mut out = Vec::new();
	for (index, dir) in local_dirs.iter().enumerate() {
		for rel in scan_one_dir(dir, oneshots.as_deref()).await? {
			if !seen.insert(rel.clone()) {
				continue;
			}
			out.push(match index {
				0 => rel,
				_ => format!("{index}:{rel}"),
			});
		}
	}
	out.sort_by(|a, b| natural_sort::compare(a, b));
	Ok(out)
}

/// Single-root scan: discovers manga under one root by the module-doc classification contract; a missing
/// root is an empty listing, not an error
async fn scan_one_dir(local_dir: &Path, oneshots: Option<&str>) -> ApiResult<Vec<String>> {
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
			discover(local_dir, item.name.clone(), items, &mut out, oneshots).await?;
		}
	}
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

/// Classifies one directory whose listing is `items`: the oneshots rule (the relative path contains the
/// configured name) emits each direct archive as its own manga and stops; otherwise pass 1 claims the
/// directory when it directly holds archives, pass 2 when no archive exists anywhere below. A claimed
/// directory absorbs its subtree, so children are classified only when it qualifies under no rule (a pure
/// container).
fn discover<'a>(
	local_dir: &'a Path,
	rel: String,
	items: Vec<DirItem>,
	out: &'a mut Vec<String>,
	oneshots: Option<&'a str>,
) -> DiscoverFuture<'a> {
	Box::pin(async move {
		// the oneshots rule outranks the archive trigger: the directory itself is never claimed, and
		// nothing below it is picked up
		if let Some(name) = oneshots
			&& rel.to_lowercase().contains(name)
		{
			out.extend(
				items
					.iter()
					.filter(|item| item.is_archive)
					.map(|item| join_rel(&rel, &item.name)),
			);
			return Ok(());
		}
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
				discover(local_dir, child, child_items, out, oneshots).await?;
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
async fn scan_chapters(local_dir: &Path, manga_key: &str) -> ApiResult<(Vec<LocalChapter>, Vec<String>)> {
	require_key(manga_key)?;
	// a key naming a file is a oneshot from the oneshots directory: the manga degenerates to that one
	// archive chapter, keyed by its leaf name like any top-level archive chapter
	if let Ok(metadata) = tokio::fs::metadata(local_dir.join(manga_key)).await
		&& metadata.is_file()
	{
		let name = manga_key.rsplit('/').next().unwrap_or(manga_key);
		let display = name
			.rsplit_once('.')
			.map(|(stem, _)| stem.to_string())
			.unwrap_or_else(|| name.to_string());
		let date_uploaded = metadata
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map(|d| d.as_secs() as i64);
		return Ok((
			vec![LocalChapter {
				number: chapter_number::parse_chapter_number(&display),
				key: name.to_string(),
				name: display,
				date_uploaded,
				is_archive: true,
			}],
			Vec::new(),
		));
	}
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
pub async fn manga_update(local_dirs: &[PathBuf], zip: &Arc<ZipCache>, manga_key: &str) -> ApiResult<LocalUpdate> {
	let (dir, rel) = resolve_key(local_dirs, manga_key)?;
	let (chapters, failed) = scan_chapters(dir, rel).await?;
	// the title is the leaf name (a oneshot drops the archive extension): the full key would repeat the
	// container path on every nested row; ComicInfo still overrides it below
	let mut manga = Manga {
		key: manga_key.to_string(),
		title: title_of(manga_key),
		..Manga::default()
	};
	if let Some(info) = read_manga_metadata(dir, zip, rel, &chapters).await {
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

/// Manga metadata resolution, first hit wins — there is no per-field merge. The chain:
/// 1. the manga directory's top-level ComicInfo.xml: authoritative when present, so a present-but-unparsable
///    file yields no metadata and nothing below is consulted;
/// 2. the first chapter archive carrying a ComicInfo.xml, in chapter order (parsed in memory only, never
///    written back to the user's directory);
/// 3. a series.json sidecar in the manga directory: a broken or unreadable file warns and falls through
///    instead of ending the chain;
/// 4. the first EPUB chapter whose package document parses, in chapter order — a broken EPUB counts as no
///    source and the next one is tried.
///
/// For a oneshot the "directory level" is the oneshots directory, whose ComicInfo.xml/series.json are series
/// sidecars the oneshots rule ignores: only archive-internal metadata (links 2 and 4) applies.
async fn read_manga_metadata(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str, chapters: &[LocalChapter]) -> Option<comic_info::ComicInfo> {
	let oneshot = tokio::fs::metadata(local_dir.join(manga_key))
		.await
		.is_ok_and(|metadata| metadata.is_file());
	if !oneshot
		&& let Some(info) = top_level_comic_info(local_dir, manga_key).await
	{
		return info;
	}
	if let Some(info) = archive_comic_info(local_dir, zip, manga_key, chapters, oneshot).await {
		return info;
	}
	if !oneshot
		&& let Some(info) = series_json_metadata(local_dir, manga_key).await
	{
		return Some(info);
	}
	epub_opf_metadata(local_dir, zip, manga_key, chapters, oneshot).await
}

/// Chain link 1: the manga directory's top-level ComicInfo.xml. Outer Some = the file is present and
/// therefore authoritative, even when its parse yielded nothing (inner None) — the whole chain then ends
/// with no metadata, the same behavior as before the chain grew more links.
async fn top_level_comic_info(local_dir: &Path, manga_key: &str) -> Option<Option<comic_info::ComicInfo>> {
	let top_level = local_dir.join(manga_key).join(comic_info::COMIC_INFO_FILE);
	match tokio::fs::read(&top_level).await {
		Ok(bytes) => Some(comic_info::parse(&bytes)),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
		Err(e) => {
			tracing::warn!(path = %top_level.display(), "read ComicInfo.xml failed: {e}");
			Some(None)
		}
	}
}

/// Chain link 2: the first archive in chapter order carrying a ComicInfo.xml. Same tri-state as link 1:
/// the first carrier is authoritative even when its file does not parse; archives without the file (or
/// with an unreadable one) are skipped.
async fn archive_comic_info(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str, chapters: &[LocalChapter], oneshot: bool) -> Option<Option<comic_info::ComicInfo>> {
	for chapter in chapters.iter().filter(|c| c.is_archive) {
		let path = chapter_archive_path(local_dir, manga_key, &chapter.key, oneshot);
		let read = {
			let zip = zip.clone();
			tokio::task::spawn_blocking(move || zip.read_entry(&path, comic_info::COMIC_INFO_FILE)).await
		};
		match read {
			Ok(Ok(bytes)) => return Some(comic_info::parse(&bytes)),
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

/// Chain link 3: the series.json sidecar in the manga directory. Unlike the ComicInfo links it is not
/// authoritative: a broken or unreadable file warns and the chain falls through, never failing the scan.
async fn series_json_metadata(local_dir: &Path, manga_key: &str) -> Option<comic_info::ComicInfo> {
	let path = local_dir.join(manga_key).join(series_json::SERIES_JSON_FILE);
	match tokio::fs::read(&path).await {
		Ok(bytes) => series_json::parse(&bytes),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
		Err(e) => {
			tracing::warn!(path = %path.display(), "read series.json failed: {e}");
			None
		}
	}
}

/// Chain link 4: the first EPUB chapter in chapter order whose package document parses. An EPUB that does
/// not parse counts as no source (read_package already warned) and the next EPUB is tried.
async fn epub_opf_metadata(local_dir: &Path, zip: &Arc<ZipCache>, manga_key: &str, chapters: &[LocalChapter], oneshot: bool) -> Option<comic_info::ComicInfo> {
	for chapter in chapters.iter().filter(|c| c.is_archive && has_ext(&c.key, &["epub"])) {
		let path = chapter_archive_path(local_dir, manga_key, &chapter.key, oneshot);
		let package = {
			let zip = zip.clone();
			tokio::task::spawn_blocking(move || epub::read_package(&zip, &path)).await
		};
		match package {
			Ok(Some(package)) => return Some(package.metadata.into_comic_info()),
			Ok(None) => continue,
			Err(e) => {
				tracing::warn!(chapter = %chapter.key, "epub metadata task failed: {e}");
				continue;
			}
		}
	}
	None
}

/// Archive chapter path on disk: for a oneshot the manga key names the archive itself, otherwise the
/// chapter key joins onto the manga directory
fn chapter_archive_path(local_dir: &Path, manga_key: &str, chapter_key: &str, oneshot: bool) -> PathBuf {
	if oneshot {
		local_dir.join(manga_key)
	} else {
		local_dir.join(manga_key).join(chapter_key)
	}
}

/// Chapter path on disk: `<manga dir>/<chapter key>` — except that a oneshot manga key names the archive
/// itself, so the file is its own single chapter and the only legal chapter key is its leaf name
async fn chapter_fs_path(local_dir: &Path, manga_key: &str, chapter_key: &str) -> ApiResult<PathBuf> {
	let manga_path = local_dir.join(manga_key);
	if tokio::fs::metadata(&manga_path).await.is_ok_and(|m| m.is_file()) {
		if chapter_key == manga_key.rsplit('/').next().unwrap_or(manga_key) {
			return Ok(manga_path);
		}
		return Err(ApiError::not_found("local chapter not found"));
	}
	Ok(manga_path.join(chapter_key))
}

/// Chapter page listing: image chapters list image files, archive chapters list image entries; both in
/// case-insensitive natural order — except a divina EPUB, whose pages come from the OPF spine (a missing
/// or unparseable OPF falls back to the natural-order rule)
pub async fn chapter_pages(local_dirs: &[PathBuf], zip: &Arc<ZipCache>, manga_key: &str, chapter_key: &str) -> ApiResult<Vec<String>> {
	let (dir, rel) = resolve_key(local_dirs, manga_key)?;
	require_key(chapter_key)?;
	let path = chapter_fs_path(dir, rel, chapter_key).await?;
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
		if has_ext(chapter_key, &["epub"]) {
			let pages = {
				let path = path.clone();
				let zip = zip.clone();
				tokio::task::spawn_blocking(move || epub::read_package(&zip, &path).map(|package| package.pages()))
					.await
					.map_err(|e| ApiError::internal(format!("read epub package failed: {e}")))?
			};
			if let Some(pages) = pages {
				return Ok(pages.into_iter().map(|page| page.entry).collect());
			}
		}
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

/// Reads one page's bytes and mime: image chapters read a single file, archive chapters extract by entry name
/// (handles come from the LRU). The mime is the extension mapping, except that a divina EPUB page takes the
/// media type its OPF manifest declares
pub async fn read_page(
	local_dirs: &[PathBuf],
	zip: &Arc<ZipCache>,
	manga_key: &str,
	chapter_key: &str,
	entry: &str,
) -> ApiResult<(Vec<u8>, String)> {
	let (dir, rel) = resolve_key(local_dirs, manga_key)?;
	require_key(chapter_key)?;
	let chapter_path = chapter_fs_path(dir, rel, chapter_key).await?;
	let metadata = tokio::fs::metadata(&chapter_path)
		.await
		.map_err(|_| ApiError::not_found("local chapter not found"))?;
	if metadata.is_dir() {
		require_component(entry)?;
		let bytes = tokio::fs::read(chapter_path.join(entry))
			.await
			.map_err(|_| ApiError::not_found("page not found"))?;
		return Ok((bytes, image_mime(entry).to_string()));
	}
	let entry_name = entry.to_string();
	let is_epub = has_ext(chapter_key, &["epub"]);
	let (bytes, declared) = {
		let chapter_path = chapter_path.clone();
		let zip = zip.clone();
		tokio::task::spawn_blocking(move || {
			let bytes = zip.read_entry(&chapter_path, &entry_name)?;
			let declared = if is_epub {
				epub::read_package(&zip, &chapter_path)
					.and_then(|package| package.pages().into_iter().find(|page| page.entry == entry_name))
					.map(|page| page.media_type)
			} else {
				None
			};
			Ok::<_, std::io::Error>((bytes, declared))
		})
		.await
		.map_err(|e| ApiError::internal(format!("read zip entry failed: {e}")))?
		.map_err(|e| {
			if e.kind() == std::io::ErrorKind::NotFound {
				ApiError::not_found("page not found")
			} else {
				ApiError::internal(format!("read local archive failed: {e}"))
			}
		})?
	};
	Ok((bytes, declared.unwrap_or_else(|| image_mime(entry).to_string())))
}

/// Cover fallback chain (Suwayomi semantics): a cover.* sidecar in the manga directory → the first page of the last
/// chapter in the list (descending by number, so the last is the oldest chapter) → None (the caller answers 404 and the UI shows a placeholder)
pub async fn resolve_cover(local_dirs: &[PathBuf], zip: &Arc<ZipCache>, manga_key: &str) -> ApiResult<Option<(Vec<u8>, String)>> {
	let (root, rel) = resolve_key(local_dirs, manga_key)?;
	let dir = root.join(rel);
	// a oneshot key names the archive file: the cover.* next to it is a series sidecar, which the
	// oneshots rule ignores — the chain falls through to the single chapter's first page
	if !tokio::fs::metadata(&dir).await.is_ok_and(|metadata| metadata.is_file()) {
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
	}
	let (chapters, _) = scan_chapters(root, rel).await?;
	let Some(last) = chapters.last() else {
		return Ok(None);
	};
	let pages = chapter_pages(local_dirs, zip, manga_key, &last.key).await?;
	let Some(first) = pages.first() else {
		return Ok(None);
	};
	let (bytes, mime) = read_page(local_dirs, zip, manga_key, &last.key, first).await?;
	Ok(Some((bytes, mime)))
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
