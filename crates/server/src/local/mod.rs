//! 内置本地源的文件系统侧：实时遍历 local_dir（无索引表，打开/上架才落库），
//! 章节目录约定 = 漫画目录下的子目录（图片章）或单个 *.cbz/*.zip。
//! 失败目录保护（借 kmrs）：读失败的条目录 failed 由调用方带进对账保护集，
//! 整趟扫描中途失败直接报错——拿半份清单对账会把没扫到的章节误当已删除。

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

/// 本地源扫描出的一章：key 是条目全名（压缩包含扩展名），name 是展示名
pub struct LocalChapter {
	pub key: String,
	pub name: String,
	pub number: Option<f32>,
	pub date_uploaded: Option<i64>,
	pub is_archive: bool,
}

/// manga_update 的结果：源侧 Manga 快照 + 读失败的章节条目名（对账保护集）
pub struct LocalUpdate {
	pub manga: Manga,
	pub protected: Vec<String>,
}

/// key 必须恰好是一个路径分量：API 入参可任意构造，不挡就会逃出 local_dir
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

/// 页图/封面的 mime：按扩展名静态映射，与在线页 mime_for 同一组类型
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

/// 浏览入口（实扫 local 根目录）：漫画 = 一级子目录；点开头、@eaDir、
/// 不可读项跳过；按目录名去重。PR-2 的浏览/刷新按钮直接调这层
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
		let Ok(metadata) = entry.metadata().await else {
			continue;
		};
		if metadata.is_dir() {
			out.push(name);
		}
	}
	out.sort_by(|a, b| natural_sort::compare(a, b));
	Ok(out)
}

/// 单本漫画的章节扫描：返回排序后的章节列表与读失败的条目名。
/// 排序同 Suwayomi LocalSource：章节号降序（未解析出按 -1 沉底），平手按展示名降序自然序
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
		let metadata = match entry.metadata().await {
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

/// 「回源拉详情 + 章节」的本地对应物：扫描漫画目录，产出与 get_manga_update
/// 同形的 Manga 快照（含 chapters），供 upsert_manga + reconcile_chapters 复用
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

/// ComicInfo 读取顺序同 Suwayomi：漫画目录顶层优先；顶层文件存在即以其为准
/// （解析失败也算），不下探压缩包；
/// 否则按章节序从首个带 ComicInfo.xml 的压缩包抽取（只在内存解析，不回写用户目录）
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

/// 章节页清单：图片章列图片文件，压缩包章列图片条目；都按大小写不敏感自然序
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
			let Ok(meta) = entry.metadata().await else {
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

/// 压缩包内的图片条目：目录项、点开头、__MACOSX 资源叉都不算页
fn is_image_entry(name: &str) -> bool {
	!name.ends_with('/')
		&& !name.split('/').any(|part| part.starts_with('.') || part == "__MACOSX")
		&& has_ext(name, IMAGE_EXTS)
}

/// 读单页字节：图片章读单文件，压缩包章按条目名抽（句柄走 LRU）
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

/// 封面回退链（Suwayomi 语义）：漫画目录 cover.* 侧车 → 章节列表末尾章的
/// 第一页（按号降序的末尾即最旧一章）→ None（调用方回 404，界面出占位图）
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
		let Ok(meta) = entry.metadata().await else {
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

/// 页内容 JSON 的 Local 变体：值是章节内的页条目名（图片章的文件名 / 压缩包条目路径）
pub fn page_content_json(entry: &str) -> Value {
	serde_json::json!({
		"content": { "Local": entry },
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	})
}
