//! Entity stores: the only production code that writes these tables, so a change to how a row is
//! written happens in one place.

pub mod category;
pub mod chapter;
pub mod library;
pub mod manga;
pub mod progress;
pub mod source;

#[cfg(test)]
mod journal_contract;

#[cfg(test)]
mod tests {
	use std::fs;
	use std::path::{Path, PathBuf};

	const VERBS: [&str; 5] = ["INSERT INTO", "INSERT OR IGNORE INTO", "INSERT OR REPLACE INTO", "UPDATE", "DELETE FROM"];

	fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
		for entry in fs::read_dir(dir).expect("read src dir") {
			let path = entry.expect("dir entry").path();
			if path.is_dir() {
				collect_sources(&path, files);
			} else if path.extension().is_some_and(|ext| ext == "rs") {
				files.push(path);
			}
		}
	}

	#[test]
	fn only_the_owning_store_writes_its_tables() {
		let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
		let mut files = Vec::new();
		collect_sources(&src, &mut files);
		let mut offenders = Vec::new();
		for file in files {
			let relative = file.strip_prefix(&src).expect("under src").to_string_lossy().replace('\\', "/");
			let text = fs::read_to_string(&file).expect("read source");
			let production = text.split("#[cfg(test)]\nmod tests").next().expect("split yields one part");
			for (owners, table) in [
				(&["store/category.rs"][..], "category"),
				(&["store/category.rs"], "category_manga"),
				(&["store/chapter.rs"], "chapter"),
				(&["store/library.rs"], "manga_user"),
				(&["store/manga.rs"], "manga"),
				(&["store/progress.rs"], "chapter_user"),
				(&["sync.rs", "sync/journal.rs", "sync/apply.rs", "sync/compact.rs", "db.rs"], "sync_change"),
				(&["store/chapter.rs", "store/manga.rs", "store/source.rs"], "sync_chapter_state"),
			] {
				if owners.contains(&relative.as_str()) {
					continue;
				}
				for verb in VERBS {
					for end in [" ", "(", "\n"] {
						if production.contains(&format!("{verb} {table}{end}")) {
							offenders.push(format!("{relative}: {verb} {table}"));
						}
					}
				}
			}
			if relative != "store/source.rs" && production.contains("DELETE FROM source WHERE") {
				offenders.push(format!("{relative}: DELETE FROM source"));
			}
			for verb in VERBS {
				if production.contains(&format!("{verb} {{")) {
					offenders.push(format!("{relative}: {verb} with an interpolated table"));
				}
			}
		}
		assert!(offenders.is_empty(), "{offenders:#?}");
	}
}
