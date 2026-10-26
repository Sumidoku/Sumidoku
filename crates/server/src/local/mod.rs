//! Filesystem side of the built-in local sources. Browsing reads the scan index (rows in the manga table),
//! never the live filesystem: rows come from two directions — opening/shelving a manga registers it through
//! manga_update, and the scheduled full scan (`scan.rs`, kmrs-style) indexes every discovered manga/chapter
//! at enumeration cost and soft-deletes (trashes) what vanished, with failed-directory protection. Post-scan
//! background analysis (`analyze.rs`) fills metadata and page rows; purge/restore live on the trash endpoints
//! (`api/local.rs`).
//!
//! A local instance (a source row with kind = local, registered through `POST /api/v1/sources/internal`)
//! has one or more root directories in its own `paths` source setting; the default instance
//! (local.local, seeded by migration 0011) additionally falls back to the legacy global local.dirs
//! setting and then the config layer — see state.rs. Manga and chapter keys are validated relative
//! paths (`valid_key`): nested layouts address their target from the owning root / the manga directory
//! (`publisher/series`, `vol1/ch01`), while `..`, absolute and drive-prefixed forms stay rejected so
//! API input can never escape the roots. Across an instance's roots the key carries the root index: a
//! bare path addresses root 0 (the pre-multi-dir form, so rows persisted before multiple roots keep
//! resolving), and `N:<path>` addresses root N >= 1 (`0:` is rejected so a root-0 manga has exactly one
//! key form). Browsing unions the instance's roots; the same relative path present in several roots is
//! listed once, from the lowest-indexed root (first directory wins — mirrors of the same content),
//! while the shadowed copies stay addressable by their numbered keys. One collision is inherent: a
//! root-0 directory named like the prefix form (`1:Foo`) resolves as root 1's `Foo` — don't name
//! directories that way. Roots never span instances: two instances over the same directory produce
//! independent manga rows.
//!
//! Discovery is top-down with nearest-qualifying-ancestor
//! absorption: a directory directly holding chapter archives qualifies (pass 1, the Komga trigger), one with no
//! archive anywhere below qualifies by the backward-compatible relaxed rule (pass 2, nested levels included),
//! anything else is a pure container and discovery descends. A claimed manga absorbs its subtree: archives and
//! image-leaf directories (direct image files) at any depth become chapters keyed by their path relative to the
//! manga, never manga of their own — so a flat single-level library scans byte-identically to before.
//!
//! Archive chapters are CBZ/ZIP and divina-profile EPUB (image-based books; reflowable text is out of scope):
//! EPUB pages follow the OPF spine through direct image items or XHTML/SVG single-image pages. The mimetype,
//! manifest, spine and zip entries must agree; invalid or non-divina packages yield no pages (`local/epub.rs`).
//! Each chapter's computed compatibility is persisted as true/false for EPUB and NULL for other formats.
//!
//! Manga metadata resolves by a first-hit-wins chain, no per-field merge (`read_manga_metadata`): the manga
//! directory's top-level ComicInfo.xml (authoritative when present, even unparsable) → the first chapter
//! archive carrying one, in chapter order → a series.json sidecar (flat or Mylar nested schema; a broken
//! file warns and falls through) → the first EPUB chapter whose OPF parses, in chapter order. JSON arrays and
//! OPF author/artist/tag lists stay structured, so their commas are literal; flat series.json scalars and
//! ComicInfo.xml keep comma-separated list semantics. Mylar readingDirection and OPF spine
//! page-progression-direction map to the manga viewer.
//!
//! With a oneshots name configured on the instance (its `oneshotsDir` source setting; the default
//! instance falls back to the legacy global setting, then config local_oneshots_dir — an explicit blank
//! turns the rule off for that instance),
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

pub mod analyze;
pub mod chapter_number;
pub mod comic_info;
pub mod epub;
pub mod home;
pub mod natural_sort;
pub mod scan;
pub mod series_json;
pub mod zip_cache;

pub use zip_cache::ZipCache;

use crate::error::{ApiError, ApiResult};
use crate::model::SourceRow;
use crate::state::AppState;
use aidoku::{Chapter, Manga, MangaStatus};
use aidoku_wasm::DefaultValue;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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

/// Per-chapter metadata extracted from inside the archive (ComicInfo.xml in CBZ/ZIP, the OPF package
/// in EPUB). scanlators persists the writers (Aidoku's only per-chapter people slot); artists and tags
/// feed the manga-level aggregation only — the chapter table has no columns for them
pub struct ChapterMeta {
	pub title: Option<String>,
	pub authors: Vec<String>,
	pub artists: Vec<String>,
	pub tags: Vec<String>,
}

/// Result of manga_update: the source-side Manga snapshot, EPUB divina results, per-chapter in-archive
/// metadata, and failed paths
pub struct LocalUpdate {
	pub manga: Manga,
	pub epub_divina: HashMap<String, bool>,
	pub chapter_meta: HashMap<String, ChapterMeta>,
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
	/// Millisecond mtime: the scheduled scan's change detector (date_uploaded is a second-precision display field)
	mtime_ms: Option<i64>,
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
		let mtime = metadata
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
		let date_uploaded = mtime.map(|d| d.as_secs() as i64);
		let mtime_ms = mtime.map(|d| d.as_millis() as i64);
		out.push(DirItem {
			is_dir: metadata.is_dir(),
			is_archive: metadata.is_file() && has_ext(&name, ARCHIVE_EXTS),
			is_image: metadata.is_file() && has_ext(&name, IMAGE_EXTS),
			name,
			date_uploaded,
			mtime_ms,
		});
	}
	Ok(out)
}

/// Browse entry point: discovers manga under every local root by the module-doc classification contract and
/// unions them into one listing. A relative path found in several roots lists once, from the lowest-indexed
/// root (first directory wins); keys from root N >= 1 carry the `N:` prefix, root 0 keeps the bare form.
/// A missing root is an empty contribution, not an error. PR-2's browse/refresh buttons call this layer directly
pub async fn scan_manga_dirs(local_dirs: &[PathBuf], oneshots: Option<&str>, exclusions: &[String]) -> ApiResult<Vec<String>> {
	Ok(scan_roots_full(local_dirs, oneshots, exclusions).await?.0)
}

/// The collecting variant for the scheduled scan: the same union as scan_manga_dirs plus every failed entry
/// with its root's key prefix, so the diff can protect unreadable subtrees instead of trashing them
pub async fn scan_roots_full(
	local_dirs: &[PathBuf],
	oneshots: Option<&str>,
	exclusions: &[String],
) -> ApiResult<(Vec<String>, Vec<String>)> {
	// exclusions arrive lowercased from the resolver (scan_one_dir lowercases oneshots once for both callers)
	let mut seen = HashSet::new();
	let mut out = Vec::new();
	let mut failed_out = Vec::new();
	for (index, dir) in local_dirs.iter().enumerate() {
		let (keys, failed) = scan_one_dir(dir, oneshots, exclusions, false).await?;
		for rel in keys {
			if !seen.insert(rel.clone()) {
				continue;
			}
			out.push(match index {
				0 => rel,
				_ => format!("{index}:{rel}"),
			});
		}
		// protection is per root: a failed `Foo` in root 1 protects the `1:Foo` rows, never root 0's `Foo`
		failed_out.extend(failed.into_iter().map(|rel| match index {
			0 => rel,
			_ => format!("{index}:{rel}"),
		}));
	}
	out.sort_by(|a, b| natural_sort::compare(a, b));
	Ok((out, failed_out))
}

/// Single-root scan: discovers manga under one root by the module-doc classification contract. `strict`
/// selects the caller's root-missing contract: browse (scan_roots_full) treats a missing root as an empty
/// listing, while the lifecycle scan (scan_instance) fails outright — a root that disappears between the
/// instance's precheck and this traversal must not degrade into an empty contribution that trashes the
/// root's whole index. Failed entries come back alongside, for the reconcile protection set
async fn scan_one_dir(
	local_dir: &Path,
	oneshots: Option<&str>,
	exclusions: &[String],
	strict: bool,
) -> ApiResult<(Vec<String>, Vec<String>)> {
	// lowercased here so both callers (scan_roots_full and the per-root scan_instance loop) apply the
	// case-insensitive substring rule the same way; discover lowercases each candidate relative path
	let oneshots = oneshots.map(str::to_lowercase);
	let mut read_dir = match tokio::fs::read_dir(local_dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			if strict {
				return Err(ApiError::conflict(
					"root_unavailable",
					format!("local root is not a readable directory: {}", local_dir.display()),
				));
			}
			return Ok((Vec::new(), Vec::new()));
		}
		Err(e) => return Err(io_scan_err(&e)),
	};
	let mut failed = Vec::new();
	let first = read_items(&mut read_dir, "", &mut failed).await?;
	let mut out = Vec::new();
	for item in first.iter().filter(|item| item.is_dir) {
		if let Some(items) = classify_items(local_dir, &item.name, &mut failed).await? {
			discover(local_dir, item.name.clone(), items, &mut out, oneshots.as_deref(), exclusions, &mut failed).await?;
		}
	}
	Ok((out, failed))
}

type DiscoverFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ApiResult<()>> + Send + 'a>>;

/// Lists one directory for classification; a directory that cannot be opened is unclassifiable (None): never
/// claimed, while the pass-2 predicate keeps its ancestors from absorbing what it may hide. The unreadable
/// path lands in `failed`, so the scheduled scan's diff protects everything known below it (kmrs's
/// failed_directories) — browse passes a throwaway vec, narrowing only its listing
async fn classify_items(local_dir: &Path, rel: &str, failed: &mut Vec<String>) -> ApiResult<Option<Vec<DirItem>>> {
	let mut read_dir = match tokio::fs::read_dir(local_dir.join(rel)).await {
		Ok(read_dir) => read_dir,
		Err(e) => {
			tracing::warn!(rel, "skip unreadable local directory: {e}");
			failed.push(rel.to_string());
			return Ok(None);
		}
	};
	read_items(&mut read_dir, rel, failed).await.map(Some)
}

/// Whether any archive exists below the directory holding `items` (the pass-2 trigger). Directories that cannot be
/// inspected — unreadable or past the depth budget — answer true: a subtree that may hide archives must never be
/// pass-2-absorbed by an ancestor.
async fn subtree_has_archive(
	local_dir: &Path,
	rel: &str,
	items: &[DirItem],
	failed: &mut Vec<String>,
) -> ApiResult<bool> {
	let mut stack: Vec<String> = items
		.iter()
		.filter(|item| item.is_dir)
		.map(|item| join_rel(rel, &item.name))
		.collect();
	while let Some(rel) = stack.pop() {
		if rel.matches('/').count() >= MAX_KEY_DEPTH {
			return Ok(true);
		}
		let Some(items) = classify_items(local_dir, &rel, failed).await? else {
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

/// Classifies one directory whose listing is `items`: an exclusion match (case-insensitive substring of the
/// relative path, kmrs's scan_directory_exclusions) skips the subtree outright — never claimed, never descended,
/// and not a failure; the oneshots rule (the relative path contains the configured name) emits each direct
/// archive as its own manga and stops; otherwise pass 1 claims the directory when it directly holds archives,
/// pass 2 when no archive exists anywhere below. A claimed directory absorbs its subtree, so children are
/// classified only when it qualifies under no rule (a pure container).
fn discover<'a>(
	local_dir: &'a Path,
	rel: String,
	items: Vec<DirItem>,
	out: &'a mut Vec<String>,
	oneshots: Option<&'a str>,
	exclusions: &'a [String],
	failed: &'a mut Vec<String>,
) -> DiscoverFuture<'a> {
	Box::pin(async move {
		let lowered = rel.to_lowercase();
		if exclusions.iter().any(|exclusion| lowered.contains(exclusion)) {
			tracing::debug!(rel, "skip excluded local directory");
			return Ok(());
		}
		// the oneshots rule outranks the archive trigger: the directory itself is never claimed, and
		// nothing below it is picked up
		if let Some(name) = oneshots
			&& lowered.contains(name)
		{
			out.extend(
				items
					.iter()
					.filter(|item| item.is_archive)
					.map(|item| join_rel(&rel, &item.name)),
			);
			return Ok(());
		}
		if items.iter().any(|item| item.is_archive) || !subtree_has_archive(local_dir, &rel, &items, failed).await? {
			out.push(rel);
			return Ok(());
		}
		for item in items.iter().filter(|item| item.is_dir) {
			let child = join_rel(&rel, &item.name);
			if child.matches('/').count() >= MAX_KEY_DEPTH {
				tracing::warn!(rel = child, "skip over-deep local directory");
				continue;
			}
			if let Some(child_items) = classify_items(local_dir, &child, failed).await? {
				discover(local_dir, child, child_items, out, oneshots, exclusions, failed).await?;
			}
		}
		Ok(())
	})
}

/// Result of scan_chapters: the sorted chapter list, failed entries by manga-relative path, and the mtime
/// accumulators for the scan's change detector (kmrs's series mtime, generalized for nesting: chapters sit at
/// any depth, so dir_mtime_ms is the max over the manga directory and every descendant directory — a deep
/// addition bumps its immediate parent — while all_mtime_ms also folds in file mtimes for the instance's
/// forceModifiedTime mode, catching in-place archive edits that never bump a directory)
pub struct ChapterScan {
	pub chapters: Vec<LocalChapter>,
	pub failed: Vec<String>,
	pub entry_kind: &'static str,
	pub dir_mtime_ms: i64,
	pub all_mtime_ms: i64,
}

/// Chapter scan of a single manga: walks the manga's whole subtree — archives and image-leaf directories (direct
/// image files) at any depth become chapters keyed by their path relative to the manga directory; directories
/// holding only subdirectories are containers, not chapters. Sort order matches Suwayomi LocalSource: chapter
/// number descending (unparsed counts as -1 and sinks to the bottom), ties broken by display name in descending
/// natural order
async fn scan_chapters(local_dir: &Path, manga_key: &str, exclusions: &[String]) -> ApiResult<ChapterScan> {
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
		let mtime_ms = metadata
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map(|d| d.as_millis() as i64)
			.unwrap_or(0);
		let date_uploaded = Some(mtime_ms / 1000);
		// the oneshot's "directory level" is its containing directory: forceModifiedTime off means an
		// in-place rewrite is invisible (same rule as ordinary manga); on joins the file mtime
		let parent_ms = tokio::fs::metadata(local_dir.join(manga_key).parent().map(Path::to_path_buf).unwrap_or_else(|| local_dir.to_path_buf()))
			.await
			.ok()
			.and_then(|metadata| metadata.modified().ok())
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map(|d| d.as_millis() as i64)
			.unwrap_or(mtime_ms);
		return Ok(ChapterScan {
			chapters: vec![LocalChapter {
				number: chapter_number::parse_chapter_number(&display),
				key: name.to_string(),
				name: display,
				date_uploaded,
				is_archive: true,
			}],
			failed: Vec::new(),
			entry_kind: "archive",
			dir_mtime_ms: parent_ms,
			all_mtime_ms: parent_ms.max(mtime_ms),
		});
	}
	let mut chapters = Vec::new();
	let mut failed = Vec::new();
	// the manga directory's own mtime heads the dir accumulator: deleting a direct child bumps only it
	let mut dir_mtime_ms = tokio::fs::metadata(local_dir.join(manga_key))
		.await
		.ok()
		.and_then(|metadata| metadata.modified().ok())
		.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
		.map(|d| d.as_millis() as i64)
		.unwrap_or(0);
	let mut all_mtime_ms = dir_mtime_ms;
	let mut read_dir = open_dir(&local_dir.join(manga_key)).await?;
	let first = read_items(&mut read_dir, "", &mut failed).await?;
	collect_chapters(
		local_dir,
		manga_key,
		first,
		exclusions,
		&mut chapters,
		&mut failed,
		&mut dir_mtime_ms,
		&mut all_mtime_ms,
	)
	.await?;
	chapters.sort_by(|a, b| {
		b.number
			.unwrap_or(-1.0)
			.total_cmp(&a.number.unwrap_or(-1.0))
			.then_with(|| natural_sort::compare(&b.name, &a.name))
	});
	Ok(ChapterScan {
		chapters,
		failed,
		entry_kind: "directory",
		dir_mtime_ms,
		all_mtime_ms,
	})
}

/// Worklist walk of the manga subtree: every directory is listed once — becoming an image-leaf chapter when it
/// directly holds image files — and its subdirectories queue for the same treatment. A directory that cannot be
/// listed degrades into `failed`, where the reconcile prefix rule protects everything known below it. Excluded
/// subdirectories (the same substring rule as discovery, matched on the full root-relative path) are skipped
/// outright. mtimes accumulate per entry: directories into dir_mtime, everything into all_mtime.
#[allow(clippy::too_many_arguments)]
async fn collect_chapters(
	local_dir: &Path,
	manga_key: &str,
	first: Vec<DirItem>,
	exclusions: &[String],
	chapters: &mut Vec<LocalChapter>,
	failed: &mut Vec<String>,
	dir_mtime_ms: &mut i64,
	all_mtime_ms: &mut i64,
) -> ApiResult<()> {
	let mut stack = vec![(String::new(), first)];
	while let Some((rel, items)) = stack.pop() {
		for item in items {
			let key = join_rel(&rel, &item.name);
			if let Some(mtime) = item.mtime_ms {
				*all_mtime_ms = (*all_mtime_ms).max(mtime);
				if item.is_dir {
					*dir_mtime_ms = (*dir_mtime_ms).max(mtime);
				}
			}
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
			let full_rel = join_rel(manga_key, &key).to_lowercase();
			if exclusions.iter().any(|exclusion| full_rel.contains(exclusion)) {
				// an excluded subtree is skipped for content purposes, but rows already persisted under it
				// must not look vanished to the reconcile: record it in the protection set
				failed.push(key);
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

/// One archive chapter's single-open analysis: everything manga_update needs from inside the file, produced
/// by one spawn_blocking per archive (the chain's link 2/4, the divina verdict and the chapter-level metadata
/// all read from this — no archive is opened twice)
struct ArchiveAnalysis {
	/// CBZ/ZIP ComicInfo carrier tri-state (chain link 2's authoritative semantics): None = no file,
	/// Some(None) = present but unparsable, Some(Some) = parsed
	comic: Option<Option<comic_info::ComicInfo>>,
	/// EPUB: the OPF package as ComicInfo (a broken package counts as no source, chain link 4 skips it)
	epub_info: Option<comic_info::ComicInfo>,
	/// EPUB only: the divina compatibility verdict, false included for a broken package
	epub_divina: Option<bool>,
}

fn analyze_archive(zip: &Arc<ZipCache>, path: &Path) -> ArchiveAnalysis {
	if has_ext(&path.to_string_lossy(), &["epub"]) {
		return match epub::read_package(zip, path) {
			Some(package) => {
				let divina = !package.pages(zip, path).is_empty();
				ArchiveAnalysis {
					comic: None,
					epub_info: Some(package.into_comic_info()),
					epub_divina: Some(divina),
				}
			}
			None => ArchiveAnalysis {
				comic: None,
				epub_info: None,
				epub_divina: Some(false),
			},
		};
	}
	let comic = match zip.read_entry(path, comic_info::COMIC_INFO_FILE) {
		Ok(bytes) => Some(comic_info::parse(&bytes)),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
		Err(e) => {
			tracing::warn!(path = %path.display(), "read archive ComicInfo.xml failed: {e}");
			None
		}
	};
	ArchiveAnalysis {
		comic,
		epub_info: None,
		epub_divina: None,
	}
}

/// The local counterpart of "fetch details + chapters from the source": scans the manga directory and produces a
/// Manga snapshot (including chapters) of the same shape as get_manga_update, for upsert_manga + reconcile_chapters to reuse
pub async fn manga_update(
	local_dirs: &[PathBuf],
	zip: &Arc<ZipCache>,
	manga_key: &str,
	exclusions: &[String],
) -> ApiResult<LocalUpdate> {
	let (dir, rel) = resolve_key(local_dirs, manga_key)?;
	let scanned = scan_chapters(dir, rel, exclusions).await?;
	let chapters = scanned.chapters;
	let failed = scanned.failed;
	let oneshot = tokio::fs::metadata(dir.join(rel)).await.is_ok_and(|metadata| metadata.is_file());
	let mut analyses = HashMap::new();
	for chapter in chapters.iter().filter(|chapter| chapter.is_archive) {
		let path = chapter_archive_path(dir, rel, &chapter.key, oneshot);
		let analysis = {
			let zip = zip.clone();
			tokio::task::spawn_blocking(move || analyze_archive(&zip, &path))
				.await
				.map_err(|e| ApiError::internal(format!("analyze local archive failed: {e}")))?
		};
		analyses.insert(chapter.key.clone(), analysis);
	}
	// the title is the leaf name (a oneshot drops the archive extension): the full key would repeat the
	// container path on every nested row; ComicInfo still overrides it below
	let mut manga = Manga {
		key: manga_key.to_string(),
		title: title_of(manga_key),
		..Manga::default()
	};
	if let Some(info) = read_manga_metadata(dir, rel, &chapters, &analyses).await {
		if let Some(title) = info.title() {
			manga.title = title.to_string();
		}
		manga.authors = info.authors();
		manga.artists = info.artists();
		manga.description = info.description();
		manga.tags = info.tags();
		manga.content_rating = info.content_rating;
		manga.viewer = info.viewer;
		manga.status = match info.status() {
			1 => MangaStatus::Ongoing,
			2 => MangaStatus::Completed,
			3 => MangaStatus::Cancelled,
			4 => MangaStatus::Hiatus,
			_ => MangaStatus::Unknown,
		};
	}
	manga.chapters = Some(chapters.iter().map(to_aidoku_chapter).collect());
	let mut epub_divina = HashMap::new();
	let mut chapter_meta = HashMap::new();
	for chapter in chapters.iter().filter(|chapter| chapter.is_archive) {
		let Some(analysis) = analyses.get(&chapter.key) else {
			continue;
		};
		if let Some(divina) = analysis.epub_divina {
			epub_divina.insert(chapter.key.clone(), divina);
		}
		let info = analysis.epub_info.as_ref().or(analysis.comic.as_ref().and_then(|carrier| carrier.as_ref()));
		if let Some(info) = info {
			// ComicInfo.xml's chapter title is Title (Series is the manga-level field); an EPUB's dc:title
			// lands in series through into_comic_info and is the chapter's book title
			let title = if analysis.epub_info.is_some() {
				info.title()
			} else {
				info.chapter_title()
			};
			let meta = ChapterMeta {
				title: title.map(str::to_string),
				authors: info.authors().unwrap_or_default(),
				artists: info.artists().unwrap_or_default(),
				tags: info.tags().unwrap_or_default(),
			};
			if meta.title.is_some() || !meta.authors.is_empty() || !meta.artists.is_empty() || !meta.tags.is_empty() {
				chapter_meta.insert(chapter.key.clone(), meta);
			}
		}
	}
	// Manga-level authors/artists/tags aggregate from the chapters when the chain found none (Komga's
	// series-authors union); a chain hit stays authoritative for the fields it produced
	if manga.authors.is_none() {
		manga.authors = comic_info::normalized_names(
			chapters
				.iter()
				.filter_map(|chapter| chapter_meta.get(&chapter.key))
				.flat_map(|meta| meta.authors.iter().cloned())
				.collect(),
		);
	}
	if manga.artists.is_none() {
		manga.artists = comic_info::normalized_names(
			chapters
				.iter()
				.filter_map(|chapter| chapter_meta.get(&chapter.key))
				.flat_map(|meta| meta.artists.iter().cloned())
				.collect(),
		);
	}
	if manga.tags.is_none() {
		manga.tags = comic_info::normalized_names(
			chapters
				.iter()
				.filter_map(|chapter| chapter_meta.get(&chapter.key))
				.flat_map(|meta| meta.tags.iter().cloned())
				.collect(),
		);
	}
	Ok(LocalUpdate {
		manga,
		epub_divina,
		chapter_meta,
		protected: failed,
	})
}

/// Persists per-chapter metadata after a reconcile (which always rewrites filename-derived titles first):
/// an extracted title overrides, writers land in scanlators; chapters without metadata are untouched
pub async fn store_chapter_metadata(
	conn: &mut sqlx::SqliteConnection,
	manga_id: i64,
	values: &HashMap<String, ChapterMeta>,
) -> ApiResult<()> {
	for (key, meta) in values {
		if let Some(title) = meta.title.as_deref() {
			sqlx::query("UPDATE chapter SET title = ? WHERE manga_id = ? AND key = ?")
				.bind(title)
				.bind(manga_id)
				.bind(key)
				.execute(&mut *conn)
				.await?;
		}
		if !meta.authors.is_empty() {
			sqlx::query("UPDATE chapter SET scanlators = ? WHERE manga_id = ? AND key = ?")
				.bind(serde_json::to_string(&meta.authors)?)
				.bind(manga_id)
				.bind(key)
				.execute(&mut *conn)
				.await?;
		}
	}
	Ok(())
}

pub async fn store_epub_divina(
	conn: &mut sqlx::SqliteConnection,
	manga_id: i64,
	chapters: &[Chapter],
	values: &HashMap<String, bool>,
) -> ApiResult<()> {
	for chapter in chapters {
		let compatible = if has_ext(&chapter.key, &["epub"]) {
			values.get(&chapter.key).copied()
		} else {
			None
		};
		sqlx::query("UPDATE chapter SET epub_divina_compatible = ? WHERE manga_id = ? AND key = ?")
			.bind(compatible)
			.bind(manga_id)
			.bind(&chapter.key)
			.execute(&mut *conn)
			.await?;
	}
	Ok(())
}

/// Manga metadata resolution, first hit wins — there is no per-field merge. The chain:
/// 1. the manga directory's top-level ComicInfo.xml: authoritative when present, so a present-but-unparsable
///    file yields no metadata and nothing below is consulted;
/// 2. the first chapter archive carrying a ComicInfo.xml, in chapter order (from the per-archive analyses —
///    parsed in memory only, never written back to the user's directory);
/// 3. a series.json sidecar in the manga directory: a broken or unreadable file warns and falls through
///    instead of ending the chain;
/// 4. the first EPUB chapter whose package document parses, in chapter order — a broken EPUB counts as no
///    source and the next one is tried.
///
/// For a oneshot the "directory level" is the oneshots directory, whose ComicInfo.xml/series.json are series
/// sidecars the oneshots rule ignores: only archive-internal metadata (links 2 and 4) applies.
async fn read_manga_metadata(
	local_dir: &Path,
	manga_key: &str,
	chapters: &[LocalChapter],
	analyses: &HashMap<String, ArchiveAnalysis>,
) -> Option<comic_info::ComicInfo> {
	let oneshot = tokio::fs::metadata(local_dir.join(manga_key))
		.await
		.is_ok_and(|metadata| metadata.is_file());
	if !oneshot
		&& let Some(info) = top_level_comic_info(local_dir, manga_key).await
	{
		return info;
	}
	// link 2: the first carrier is authoritative even when its file does not parse (Some(None) ends the
	// chain with no metadata); archives without the file (or with an unreadable one) are skipped
	for chapter in chapters.iter().filter(|c| c.is_archive) {
		if let Some(ArchiveAnalysis { comic: Some(carrier), .. }) = analyses.get(&chapter.key) {
			return carrier.clone();
		}
	}
	if !oneshot
		&& let Some(info) = series_json_metadata(local_dir, manga_key).await
	{
		return Some(info);
	}
	// link 4: the first parseable EPUB package in chapter order
	for chapter in chapters.iter().filter(|c| c.is_archive) {
		if let Some(ArchiveAnalysis { epub_info: Some(info), .. }) = analyses.get(&chapter.key) {
			return Some(info.clone());
		}
	}
	None
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

/// Chapter page listing and divina compatibility: image chapters and CBZ/ZIP archives use natural order;
/// divina EPUBs use OPF spine order, and invalid or non-divina EPUBs have no pages
pub async fn chapter_pages(local_dirs: &[PathBuf], zip: &Arc<ZipCache>, manga_key: &str, chapter_key: &str) -> ApiResult<(Vec<String>, Option<bool>)> {
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
		return Ok((pages, None));
	}
	if metadata.is_file() && has_ext(chapter_key, ARCHIVE_EXTS) {
		if has_ext(chapter_key, &["epub"]) {
			let pages = {
				let path = path.clone();
				let zip = zip.clone();
				tokio::task::spawn_blocking(move || epub::divina_pages(&zip, &path))
					.await
					.map_err(|e| ApiError::internal(format!("read epub package failed: {e}")))?
			};
			let compatible = Some(!pages.is_empty());
			return Ok((pages.into_iter().map(|page| page.entry).collect(), compatible));
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
		return Ok((pages, None));
	}
	Err(ApiError::bad_request("unsupported local chapter format"))
}

/// Image entries inside an archive: directory entries, dot-prefixed names, and __MACOSX resource forks are not pages
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
	let (bytes, declared) = read_page_impl(local_dirs, zip, manga_key, chapter_key, entry, true).await?;
	Ok((bytes, declared.unwrap_or_else(|| image_mime(entry).to_string())))
}

async fn read_page_impl(
	local_dirs: &[PathBuf],
	zip: &Arc<ZipCache>,
	manga_key: &str,
	chapter_key: &str,
	entry: &str,
	read_declared: bool,
) -> ApiResult<(Vec<u8>, Option<String>)> {
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
		return Ok((bytes, None));
	}
	let entry_name = entry.to_string();
	let is_epub = read_declared && has_ext(chapter_key, &["epub"]);
	let (bytes, declared) = {
		let chapter_path = chapter_path.clone();
		let zip = zip.clone();
		tokio::task::spawn_blocking(move || {
			let bytes = zip.read_entry(&chapter_path, &entry_name)?;
			let declared = if is_epub {
				epub::read_package(&zip, &chapter_path).and_then(|package| package.declared_media_type(&entry_name))
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
	Ok((bytes, declared))
}

/// Cover fallback chain (Suwayomi semantics): a cover.* sidecar in the manga directory → the first page of the last
/// chapter in the list (descending by number, so the last is the oldest chapter) → None (the caller answers 404 and the UI shows a placeholder)
pub async fn resolve_cover(
	local_dirs: &[PathBuf],
	zip: &Arc<ZipCache>,
	manga_key: &str,
	exclusions: &[String],
) -> ApiResult<Option<(Vec<u8>, String)>> {
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
	let chapters = scan_chapters(root, rel, exclusions).await?.chapters;
	let Some(last) = chapters.last() else {
		return Ok(None);
	};
	let (pages, _) = chapter_pages(local_dirs, zip, manga_key, &last.key).await?;
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

/// Registration/settings validation for an instance's roots: at least one entry, every entry a
/// non-empty path that exists and is a directory. The scan treats a vanished root as empty, so this
/// check is the only guard against a typo silently emptying the browse list
pub async fn validate_paths(paths: &[String]) -> ApiResult<Vec<String>> {
	if paths.is_empty() {
		return Err(ApiError::bad_request("paths must not be empty"));
	}
	let mut dirs = Vec::with_capacity(paths.len());
	for path in paths {
		let trimmed = path.trim();
		if trimmed.is_empty() {
			return Err(ApiError::bad_request("paths must not contain empty entries"));
		}
		match tokio::fs::metadata(trimmed).await {
			Ok(metadata) if metadata.is_dir() => dirs.push(trimmed.to_string()),
			Ok(_) => return Err(ApiError::bad_request(format!("local path is not a directory: {trimmed}"))),
			Err(e) => return Err(ApiError::bad_request(format!("local path is not readable: {trimmed} ({e})"))),
		}
	}
	Ok(dirs)
}

const SOURCE_ROW_SELECT: &str = "SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
	 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source WHERE id = ?";

/// Instance registration: key takes local.<slug>, collisions append a -2/-3 counter (the opds rule;
/// a name slugging to "local" collides with the reserved default key and takes the counter form).
/// Paths land in the instance's own source setting; the oneshots marker and the other per-instance
/// overrides (exclusions, scan/analysis toggles) are written only when provided, mirroring the
/// settings panel so absent values keep the config-layer fallback
#[allow(clippy::too_many_arguments)]
pub async fn register_instance(
	state: &AppState,
	name: &str,
	paths: &[String],
	oneshots_dir: Option<&str>,
	exclusions: Option<&[String]>,
	force_modified_time: Option<bool>,
	analyze_pages: Option<bool>,
	analyze_metadata: Option<bool>,
) -> ApiResult<SourceRow> {
	let name = name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	let dirs = validate_paths(paths).await?;
	let slug = crate::source::slugify(name).ok_or_else(|| ApiError::bad_request("name has no usable characters"))?;
	let mut key = format!("local.{slug}");
	let mut counter = 2;
	loop {
		let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM source WHERE key = ?")
			.bind(&key)
			.fetch_optional(&state.pool)
			.await?;
		if exists.is_none() {
			break;
		}
		key = format!("local.{slug}-{counter}");
		counter += 1;
	}
	let now = crate::model::unix_now();
	let result = sqlx::query(
		"INSERT INTO source (key, name, kind, version, languages, content_rating, base_url,
		 breaking_change_version, wasm_path, enabled, installed_at, updated_at)
		 VALUES (?, ?, ?, 0, '[]', 0, NULL, 0, '', 1, ?, ?)",
	)
	.bind(&key)
	.bind(name)
	.bind(crate::model::SOURCE_KIND_LOCAL)
	.bind(now)
	.bind(now)
	.execute(&state.pool)
	.await?;
	let source_id = result.last_insert_rowid();
	let raw = serde_json::to_string(&DefaultValue::StringArray(dirs))?;
	sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, 'paths', ?)")
		.bind(source_id)
		.bind(raw)
		.execute(&state.pool)
		.await?;
	if let Some(oneshots) = oneshots_dir.map(str::trim) {
		let raw = serde_json::to_string(&DefaultValue::String(oneshots.to_string()))?;
		sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, 'oneshotsDir', ?)")
			.bind(source_id)
			.bind(raw)
			.execute(&state.pool)
			.await?;
	}
	if let Some(exclusions) = exclusions {
		let cleaned: Vec<String> = exclusions.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
		if !cleaned.is_empty() {
			let raw = serde_json::to_string(&DefaultValue::StringArray(cleaned))?;
			sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, 'exclusions', ?)")
				.bind(source_id)
				.bind(raw)
				.execute(&state.pool)
				.await?;
		}
	}
	for (key, value) in [
		("forceModifiedTime", force_modified_time),
		("analyzePages", analyze_pages),
		("analyzeMetadata", analyze_metadata),
	] {
		if let Some(v) = value {
			let raw = serde_json::to_string(&DefaultValue::Bool(v))?;
			sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)")
				.bind(source_id)
				.bind(key)
				.bind(raw)
				.execute(&state.pool)
				.await?;
		}
	}
	let row: Option<SourceRow> = sqlx::query_as(SOURCE_ROW_SELECT)
		.bind(source_id)
		.fetch_optional(&state.pool)
		.await?;
	row.ok_or_else(|| ApiError::internal("registered local source missing"))
}

/// Startup guarantee: a deployment whose config explicitly names local roots (file/env) always has the
/// default local instance. Deleting local.local through the API is refused while that config stands, so
/// a missing row here means the config appeared after an earlier deletion — recreate the instance fresh
pub async fn ensure_default_instance(state: &AppState) -> ApiResult<()> {
	if state.config.local_dirs.is_none() {
		return Ok(());
	}
	let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
		.bind(crate::model::LOCAL_SOURCE_KEY)
		.fetch_optional(&state.pool)
		.await?;
	if exists.is_some() {
		return Ok(());
	}
	let now = crate::model::unix_now();
	sqlx::query(
		"INSERT INTO source (key, name, kind, version, languages, wasm_path, enabled, installed_at, updated_at)
		 VALUES (?, 'Local', ?, 0, '[]', '', 1, ?, ?)",
	)
	.bind(crate::model::LOCAL_SOURCE_KEY)
	.bind(crate::model::SOURCE_KIND_LOCAL)
	.bind(now)
	.bind(now)
	.execute(&state.pool)
	.await?;
	Ok(())
}
