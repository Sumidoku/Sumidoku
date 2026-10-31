//! Entity stores: the only production code that writes these tables, so a change to how a row is
//! written happens in one place.

pub mod category;

#[cfg(test)]
mod tests {
	use std::fs;
	use std::path::{Path, PathBuf};

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
			if relative == "store/category.rs" {
				continue;
			}
			let text = fs::read_to_string(&file).expect("read source");
			let production = text.split("#[cfg(test)]\nmod tests").next().expect("split yields one part");
			for table in ["category", "category_manga"] {
				for verb in ["INSERT INTO", "INSERT OR IGNORE INTO", "INSERT OR REPLACE INTO", "UPDATE", "DELETE FROM"] {
					for end in [" ", "(", "\n"] {
						if production.contains(&format!("{verb} {table}{end}")) {
							offenders.push(format!("{relative}: {verb} {table}"));
						}
					}
				}
			}
		}
		assert!(offenders.is_empty(), "{offenders:#?}");
	}
}
