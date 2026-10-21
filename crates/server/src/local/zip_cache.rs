//! LRU of open CBZ handles: reopening an archive re-reads the whole central directory, so extracting a chapter page by page would pay that per page.
//! Capacity pinned at 16 (same as kmrs); one global lock, each hold extracts a single entry, so contention cost is negligible.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

pub const ZIP_CACHE_CAPACITY: usize = 16;

pub struct ZipCache {
	inner: std::sync::Mutex<ZipCacheInner>,
}

struct ZipCacheInner {
	archives: HashMap<PathBuf, zip::ZipArchive<std::fs::File>>,
	/// Oldest at the front, newest at the back; a hit moves to the back, a full cache evicts the front
	recency: VecDeque<PathBuf>,
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

	/// Reads at most `limit` bytes from a single zip entry, decompressing only enough to satisfy the limit.
	pub fn read_entry_head(&self, path: &Path, name: &str, limit: usize) -> std::io::Result<Vec<u8>> {
		self.with_archive(path, |archive| {
			let mut entry = archive
				.by_name(name)
				.map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "zip entry not found"))?;
			let mut buf = vec![0u8; limit];
			let mut total = 0;
			while total < limit {
				match std::io::Read::read(&mut entry, &mut buf[total..])? {
					0 => break,
					n => total += n,
				}
			}
			buf.truncate(total);
			Ok(buf)
		})
	}

	fn with_archive<T>(
		&self,
		path: &Path,
		f: impl FnOnce(&mut zip::ZipArchive<std::fs::File>) -> std::io::Result<T>,
	) -> std::io::Result<T> {
		let mut inner = self.inner.lock().expect("zip cache lock");
		if !inner.archives.contains_key(path) {
			if inner.archives.len() >= ZIP_CACHE_CAPACITY
				&& let Some(old) = inner.recency.pop_front()
			{
				inner.archives.remove(&old);
			}
			let file = std::fs::File::open(path)?;
			let archive = zip::ZipArchive::new(file)
				.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
			inner.archives.insert(path.to_path_buf(), archive);
		}
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
	fn reads_entry_head_only() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("a.cbz");
		let payload = vec![b'x'; 256 * 1024];
		make_zip(&path, &[("big.bin", &payload)]);
		let cache = ZipCache::new();
		let head = cache.read_entry_head(&path, "big.bin", 64 * 1024).expect("read head");
		assert_eq!(head.len(), 64 * 1024);
		assert!(head.iter().all(|&b| b == b'x'));
		let small = cache.read_entry_head(&path, "big.bin", 1024).expect("read small head");
		assert_eq!(small.len(), 1024);
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
