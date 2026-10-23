//! LRU of open CBZ handles: reopening an archive re-reads the whole central directory, so extracting a chapter page by page would pay that per page.
//! Capacity pinned at 16 (same as kmrs); one global lock, each hold extracts a single entry, so contention cost is negligible.
//! Cached handles are reopened when the file version changes, so atomic replacement cannot leave stale bytes.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

pub const ZIP_CACHE_CAPACITY: usize = 16;

pub struct ZipCache {
	inner: std::sync::Mutex<ZipCacheInner>,
}

struct ZipCacheInner {
	archives: HashMap<PathBuf, zip::ZipArchive<std::fs::File>>,
	versions: HashMap<PathBuf, FileVersion>,
	/// Oldest at the front, newest at the back; a hit moves to the back, a full cache evicts the front
	recency: VecDeque<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileVersion {
	modified: Option<std::time::SystemTime>,
	len: u64,
	#[cfg(unix)]
	dev: u64,
	#[cfg(unix)]
	ino: u64,
}

impl FileVersion {
	fn current(path: &Path) -> std::io::Result<Self> {
		let metadata = std::fs::metadata(path)?;
		Ok(Self {
			modified: metadata.modified().ok(),
			len: metadata.len(),
			#[cfg(unix)]
			dev: std::os::unix::fs::MetadataExt::dev(&metadata),
			#[cfg(unix)]
			ino: std::os::unix::fs::MetadataExt::ino(&metadata),
		})
	}
}

impl Default for ZipCache {
	fn default() -> Self {
		Self::new()
	}
}

impl ZipCache {
	pub fn new() -> Self {
		Self {
			inner: std::sync::Mutex::new(ZipCacheInner {
				archives: HashMap::new(),
				versions: HashMap::new(),
				recency: VecDeque::new(),
			}),
		}
	}

	pub fn entry_names(&self, path: &Path) -> std::io::Result<Vec<String>> {
		self.with_archive(path, |archive| {
			archive
				.file_names()
				.map(|name| {
					name.map(|n| n.into_owned())
						.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
				})
				.collect()
		})
	}

	pub fn read_entry(&self, path: &Path, name: &str) -> std::io::Result<Vec<u8>> {
		self.with_archive(path, |archive| {
			let mut entry = archive
				.by_name(name)
				.map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "zip entry not found"))?;
			let mut out = Vec::with_capacity(entry.size() as usize);
			std::io::Read::read_to_end(&mut entry, &mut out)?;
			Ok(out)
		})
	}

	fn with_archive<T>(
		&self,
		path: &Path,
		f: impl FnOnce(&mut zip::ZipArchive<std::fs::File>) -> std::io::Result<T>,
	) -> std::io::Result<T> {
		let mut inner = self.inner.lock().expect("zip cache lock");
		let version = FileVersion::current(path)?;
		if inner.versions.get(path).is_some_and(|cached| *cached != version) {
			inner.archives.remove(path);
			inner.versions.remove(path);
			inner.recency.retain(|p| p != path);
		}
		if !inner.archives.contains_key(path) {
			if inner.archives.len() >= ZIP_CACHE_CAPACITY
				&& let Some(old) = inner.recency.pop_front()
			{
				inner.archives.remove(&old);
				inner.versions.remove(&old);
			}
			let file = std::fs::File::open(path)?;
			let archive = zip::ZipArchive::new(file)
				.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
			inner.archives.insert(path.to_path_buf(), archive);
		}
		inner.versions.insert(path.to_path_buf(), version);
		inner.recency.retain(|p| p != path);
		inner.recency.push_back(path.to_path_buf());
		let archive = inner.archives.get_mut(path).expect("archive inserted");
		f(archive)
	}

	#[cfg(test)]
	fn cached_len(&self) -> usize {
		self.inner.lock().expect("zip cache lock").archives.len()
	}

	#[cfg(test)]
	fn is_cached(&self, path: &Path) -> bool {
		self.inner.lock().expect("zip cache lock").archives.contains_key(path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
		let file = std::fs::File::create(path).expect("create zip");
		let mut writer = zip::ZipWriter::new(file);
		for (name, bytes) in entries {
			writer
				.start_file(*name, zip::write::SimpleFileOptions::default())
				.expect("start entry");
			std::io::Write::write_all(&mut writer, bytes).expect("write entry");
		}
		writer.finish().expect("finish zip");
	}

	#[test]
	fn reads_entry_bytes_and_names() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("a.cbz");
		make_zip(&path, &[("b.png", b"bb"), ("a.png", b"aa")]);
		let cache = ZipCache::new();
		assert_eq!(cache.read_entry(&path, "a.png").expect("read"), b"aa");
		assert_eq!(cache.entry_names(&path).expect("names"), vec!["b.png", "a.png"]);
		assert!(cache.read_entry(&path, "missing.png").is_err());
	}

	#[test]
	fn reloads_atomically_replaced_archive() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("a.cbz");
		make_zip(&path, &[("p.png", b"old")]);
		let cache = ZipCache::new();
		assert_eq!(cache.read_entry(&path, "p.png").expect("read old"), b"old");
		let replacement = tmp.path().join("replacement.cbz");
		make_zip(&replacement, &[("p.png", b"new")]);
		std::fs::rename(&replacement, &path).expect("replace archive");
		assert_eq!(cache.read_entry(&path, "p.png").expect("read replacement"), b"new");
	}

	#[test]
	fn reloads_truncated_archive() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("a.cbz");
		make_zip(&path, &[("p.png", b"x")]);
		let cache = ZipCache::new();
		assert_eq!(cache.read_entry(&path, "p.png").expect("read old"), b"x");
		make_zip(&path, &[("p.png", b"longer")]);
		assert_eq!(cache.read_entry(&path, "p.png").expect("read truncated"), b"longer");
	}

	#[test]
	fn evicts_least_recently_used_at_capacity() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let cache = ZipCache::new();
		let mut paths = Vec::new();
		for i in 0..ZIP_CACHE_CAPACITY + 1 {
			let path = tmp.path().join(format!("{i}.cbz"));
			make_zip(&path, &[("p.png", b"x")]);
			paths.push(path);
		}
		for path in &paths {
			cache.read_entry(path, "p.png").expect("read");
		}
		assert_eq!(cache.cached_len(), ZIP_CACHE_CAPACITY);
		assert!(!cache.is_cached(&paths[0]), "最早打开的应被逐出");
		assert!(cache.is_cached(&paths[ZIP_CACHE_CAPACITY]));

		// A hit refreshes recency: touch #1 again, and the next capacity eviction removes #2
		cache.read_entry(&paths[1], "p.png").expect("read");
		let extra = tmp.path().join("extra.cbz");
		make_zip(&extra, &[("p.png", b"x")]);
		cache.read_entry(&extra, "p.png").expect("read");
		assert!(cache.is_cached(&paths[1]), "刚命中过的不应被逐出");
		assert!(!cache.is_cached(&paths[2]), "最久未用的应被逐出");
	}

	#[test]
	fn corrupt_file_is_invalid_data() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("bad.cbz");
		std::fs::write(&path, b"not a zip").expect("write");
		let cache = ZipCache::new();
		let err = cache.read_entry(&path, "p.png").expect_err("must fail");
		assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
		assert_eq!(cache.cached_len(), 0, "打不开的包不进缓存");
	}
}
