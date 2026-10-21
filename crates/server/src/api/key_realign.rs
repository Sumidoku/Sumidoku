//! Per-source key realignment for Suwayomi backup imports: a Mihon extension and the corresponding
//! Aidoku source can disagree on the in-source key shape for the same site, and rows imported with
//! the backup's shape make the target source build broken URLs (Suwayomi's copymanga extension
//! stores site paths, Aidoku's zh.copymanga builds them from the bare segments). Every rewrite is a
//! pure segment transform, so an already-native key comes back unchanged — re-running an import is
//! safe, and sources without an entry are left alone entirely.

/// Rewrite counts of one import group, reported as `realignedKeys` when non-zero
#[derive(Debug, Default, Clone, Copy)]
pub struct RealignCounts {
	pub manga: i64,
	pub chapter: i64,
}

impl RealignCounts {
	pub fn is_zero(&self) -> bool {
		self.manga == 0 && self.chapter == 0
	}
}

pub fn realign_manga_key(source_key: &str, key: &str) -> (String, bool) {
	match source_key {
		"zh.copymanga" => copymanga_manga(key),
		_ => (key.to_string(), false),
	}
}

pub fn realign_chapter_key(source_key: &str, key: &str) -> (String, bool) {
	match source_key {
		"zh.copymanga" => copymanga_chapter(key),
		_ => (key.to_string(), false),
	}
}

/// "/comic/<slug>" -> "<slug>"
fn copymanga_manga(key: &str) -> (String, bool) {
	match key.strip_prefix("/comic/") {
		Some(rest) if !rest.is_empty() => (rest.to_string(), true),
		_ => (key.to_string(), false),
	}
}

/// both wire shapes ("<slug>/chapter/<uuid>" and "/comic/<slug>/chapter/<uuid>") reduce to the bare
/// uuid, which is the only segment the Aidoku source needs
fn copymanga_chapter(key: &str) -> (String, bool) {
	match key.rsplit_once('/') {
		Some((_, tail)) if !tail.is_empty() => (tail.to_string(), true),
		_ => (key.to_string(), false),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn copymanga_manga_strips_comic_prefix() {
		assert_eq!(realign_manga_key("zh.copymanga", "/comic/pure001mm"), ("pure001mm".to_string(), true));
		// idempotent: a native key is returned as-is
		assert_eq!(realign_manga_key("zh.copymanga", "pure001mm"), ("pure001mm".to_string(), false));
		assert_eq!(realign_manga_key("zh.copymanga", "/comic/"), ("/comic/".to_string(), false));
	}

	#[test]
	fn copymanga_chapter_takes_last_segment() {
		let uuid = "d32acc22-aaaa-4000-8000-000000000001";
		for path in ["pure001mm/chapter/", "/comic/pure001mm/chapter/"] {
			let (realigned, changed) = realign_chapter_key("zh.copymanga", &format!("{path}{uuid}"));
			assert_eq!(realigned, uuid, "{path}");
			assert!(changed, "{path}");
		}
		// idempotent: a bare uuid has no separator and stays put
		assert_eq!(realign_chapter_key("zh.copymanga", uuid), (uuid.to_string(), false));
		assert_eq!(realign_chapter_key("zh.copymanga", "uuid/"), ("uuid/".to_string(), false));
	}

	#[test]
	fn unregistered_source_keeps_keys() {
		assert_eq!(realign_manga_key("en.example-source", "/comic/x"), ("/comic/x".to_string(), false));
		assert_eq!(realign_chapter_key("en.example-source", "a/b"), ("a/b".to_string(), false));
	}
}
