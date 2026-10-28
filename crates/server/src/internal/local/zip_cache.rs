//! LRU of open CBZ handles: reopening an archive re-reads the whole central directory, so extracting a chapter page by page would pay that per page.
//! Capacity pinned at 16 (same as kmrs). The cache lock covers only the lookup; each archive has its own lock held
//! while one entry is extracted, so reads of different archives (and archive opens) run in parallel on slow disks.
//! A panic while extracting drops that archive from the cache instead of poisoning every later read.
//! Cached handles are reopened when the file version changes, so atomic replacement cannot leave stale bytes.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub const ZIP_CACHE_CAPACITY: usize = 16;
/// The entry's declared size is untrusted header data: it only hints the first allocation, never sizes it
const ENTRY_CAPACITY_HINT_MAX: u64 = 32 * 1024 * 1024;

type Archive = zip::ZipArchive<std::fs::File>;

pub struct ZipCache {
	inner: Mutex<ZipCacheInner>,
}

struct ZipCacheInner {
	archives: HashMap<PathBuf, (Arc<Mutex<Archive>>, FileVersion)>,
	/// Oldest at the front, newest at the back; a hit moves to the back, a full cache evicts the front
	recency: VecDeque<PathBuf>,
}

impl ZipCacheInner {
	fn forget(&mut self, path: &Path) {
		self.archives.remove(path);
		self.recency.retain(|p| p != path);
	}

	fn touch(&mut self, path: &Path) {
		self.recency.retain(|p| p != path);
		self.recency.push_back(path.to_path_buf());
	}
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
			inner: Mutex::new(ZipCacheInner {
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
			let mut out = Vec::with_capacity(entry.size().min(ENTRY_CAPACITY_HINT_MAX) as usize);
			std::io::Read::read_to_end(&mut entry, &mut out)?;
			Ok(out)
		})
	}

	/// The cache map holds no invariant a panic could break mid-update, so a poisoned lock is still usable
	fn lock(&self) -> MutexGuard<'_, ZipCacheInner> {
		self.inner.lock().unwrap_or_else(PoisonError::into_inner)
	}

	fn with_archive<T>(&self, path: &Path, f: impl FnOnce(&mut Archive) -> std::io::Result<T>) -> std::io::Result<T> {
		let handle = self.handle(path)?;
		let mut archive = match handle.lock() {
			Ok(archive) => archive,
			// A panic mid-extraction left this reader in an unknown position: drop it and read through a fresh open
			Err(_) => {
				let mut inner = self.lock();
				if inner.archives.get(path).is_some_and(|(cached, _)| Arc::ptr_eq(cached, &handle)) {
					inner.forget(path);
				}
				drop(inner);
				let fresh = self.handle(path)?;
				let mut archive = fresh.lock().unwrap_or_else(PoisonError::into_inner);
				return f(&mut archive);
			}
		};
		f(&mut archive)
	}

	fn handle(&self, path: &Path) -> std::io::Result<Arc<Mutex<Archive>>> {
		let version = FileVersion::current(path)?;
		{
			let mut inner = self.lock();
			match inner.archives.get(path) {
				Some((archive, cached)) if *cached == version => {
					let archive = archive.clone();
					inner.touch(path);
					return Ok(archive);
				}
				Some(_) => inner.forget(path),
				None => {}
			}
		}
		// Opening reads the central directory: done outside the cache lock so a slow archive blocks no other reads
		let file = std::fs::File::open(path)?;
		let archive = zip::ZipArchive::new(file).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
		let archive = Arc::new(Mutex::new(archive));
		let mut inner = self.lock();
		if !inner.archives.contains_key(path)
			&& inner.archives.len() >= ZIP_CACHE_CAPACITY
			&& let Some(old) = inner.recency.pop_front()
		{
			inner.archives.remove(&old);
		}
		inner.archives.insert(path.to_path_buf(), (archive.clone(), version));
		inner.touch(path);
		Ok(archive)
	}

	#[cfg(test)]
	fn cached_len(&self) -> usize {
		self.lock().archives.len()
	}

	#[cfg(test)]
	fn is_cached(&self, path: &Path) -> bool {
		self.lock().archives.contains_key(path)
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

	#[test]
	fn a_panic_while_extracting_does_not_break_later_reads() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("a.cbz");
		make_zip(&path, &[("p.png", b"x")]);
		let cache = ZipCache::new();
		let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
			let _ = cache.with_archive(&path, |_| -> std::io::Result<()> { panic!("extraction failed") });
		}));
		assert!(panicked.is_err());
		assert_eq!(cache.read_entry(&path, "p.png").expect("read after panic"), b"x");
	}

	#[test]
	fn a_held_archive_does_not_block_reads_of_another() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let (slow, fast) = (tmp.path().join("slow.cbz"), tmp.path().join("fast.cbz"));
		make_zip(&slow, &[("p.png", b"s")]);
		make_zip(&fast, &[("p.png", b"f")]);
		let cache = Arc::new(ZipCache::new());
		let (entered_tx, entered_rx) = std::sync::mpsc::channel();
		let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
		let holder = {
			let cache = cache.clone();
			std::thread::spawn(move || {
				cache
					.with_archive(&slow, |_| {
						entered_tx.send(()).expect("signal entered");
						release_rx.recv().expect("wait for release");
						Ok(())
					})
					.expect("held read");
			})
		};
		entered_rx.recv().expect("holder entered");
		let (done_tx, done_rx) = std::sync::mpsc::channel();
		let reader = {
			let cache = cache.clone();
			std::thread::spawn(move || done_tx.send(cache.read_entry(&fast, "p.png").expect("read fast")))
		};
		let read = done_rx.recv_timeout(std::time::Duration::from_secs(5));
		release_tx.send(()).expect("release holder");
		holder.join().expect("holder thread");
		reader.join().expect("reader thread").expect("report read");
		assert_eq!(read.expect("another archive must not wait for the held one"), b"f");
	}
}
