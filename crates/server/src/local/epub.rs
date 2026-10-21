//! Divina-profile EPUB reading for the local source: an EPUB is an OCF zip, so entries come through
//! the same ZipCache as CBZ. The page list is the OPF spine order restricted to image manifest
//! items; hrefs resolve against the OPF location into zip entry paths. Only the divina profile
//! (image-based books) is supported — a reflowable text document parses fine but yields no pages.

use quick_xml::de::from_reader;
use serde::Deserialize;
use std::path::Path;

use super::zip_cache::ZipCache;

/// The image media types a divina page may declare; anything else in the spine (nav documents, stylesheets) is not a page
const IMAGE_MEDIA_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];

/// One page resolved from the OPF: the zip entry path plus the manifest-declared media type
pub struct Page {
	pub entry: String,
	pub media_type: String,
}

/// The OPF package document, located through META-INF/container.xml. The metadata fields are not
/// read by the page pipeline; they are parsed so OPF-based metadata mapping reuses this same read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Package {
	pub metadata: PackageMetadata,
	manifest: Manifest,
	spine: Spine,
	/// Directory of the OPF inside the archive: manifest hrefs are relative to it
	#[serde(skip)]
	opf_dir: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageMetadata {
	// quick-xml's serde matches elements by local name (the prefix is stripped), so dc:title is "title" here
	#[serde(rename = "title")]
	pub title: Option<String>,
	#[serde(rename = "creator")]
	pub creators: Vec<String>,
	#[serde(rename = "description")]
	pub description: Option<String>,
	#[serde(rename = "subject")]
	pub subjects: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Manifest {
	#[serde(rename = "item")]
	items: Vec<ManifestItem>,
}

#[derive(Debug, Deserialize)]
struct ManifestItem {
	#[serde(rename = "@id")]
	id: String,
	#[serde(rename = "@href")]
	href: String,
	#[serde(rename = "@media-type")]
	media_type: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Spine {
	#[serde(rename = "itemref")]
	itemrefs: Vec<ItemRef>,
}

#[derive(Debug, Deserialize)]
struct ItemRef {
	#[serde(rename = "@idref")]
	idref: String,
}

#[derive(Debug, Deserialize)]
struct Container {
	rootfiles: Rootfiles,
}

#[derive(Debug, Deserialize)]
struct Rootfiles {
	#[serde(rename = "rootfile")]
	files: Vec<Rootfile>,
}

#[derive(Debug, Deserialize)]
struct Rootfile {
	#[serde(rename = "@full-path")]
	full_path: String,
	#[serde(rename = "@media-type")]
	media_type: Option<String>,
}

impl Package {
	/// Page list in spine reading order, restricted to manifest items declaring an image media
	/// type; entries not referenced by the spine and non-image spine items never appear
	pub fn pages(&self) -> Vec<Page> {
		let mut out = Vec::new();
		for itemref in &self.spine.itemrefs {
			let Some(item) = self.manifest.items.iter().find(|item| item.id == itemref.idref) else {
				continue;
			};
			if !IMAGE_MEDIA_TYPES.contains(&item.media_type.as_str()) {
				continue;
			}
			out.push(Page {
				entry: join_zip_path(&self.opf_dir, &percent_decode(&item.href)),
				media_type: item.media_type.clone(),
			});
		}
		out
	}
}

/// Reads the OPF package document of an EPUB archive. Any missing or unparseable step (container,
/// rootfile, OPF) yields None — the caller then falls back to the CBZ rule, so a broken EPUB still
/// reads as a plain image archive instead of erroring the chapter.
pub fn read_package(zip: &ZipCache, path: &Path) -> Option<Package> {
	let container = zip.read_entry(path, "META-INF/container.xml").ok()?;
	let container: Container = match from_reader(&container[..]) {
		Ok(container) => container,
		Err(e) => {
			tracing::warn!(path = %path.display(), "unparsable EPUB container.xml: {e}");
			return None;
		}
	};
	let rootfile = container
		.rootfiles
		.files
		.iter()
		.find(|f| f.media_type.as_deref() == Some("application/oebps-package+xml"))
		.or_else(|| container.rootfiles.files.first())?;
	let opf = zip.read_entry(path, &rootfile.full_path).ok()?;
	let mut package: Package = match from_reader(&opf[..]) {
		Ok(package) => package,
		Err(e) => {
			tracing::warn!(path = %path.display(), "unparsable EPUB package document: {e}");
			return None;
		}
	};
	package.opf_dir = rootfile
		.full_path
		.rsplit_once('/')
		.map(|(dir, _)| dir.to_string())
		.unwrap_or_default();
	Some(package)
}

/// OPF hrefs are URIs: percent-encoded octets decode back to the raw zip entry name
fn percent_decode(href: &str) -> String {
	fn hex(b: u8) -> Option<u8> {
		match b {
			b'0'..=b'9' => Some(b - b'0'),
			b'a'..=b'f' => Some(b - b'a' + 10),
			b'A'..=b'F' => Some(b - b'A' + 10),
			_ => None,
		}
	}
	let bytes = href.as_bytes();
	let mut out = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%'
			&& i + 2 < bytes.len()
			&& let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
		{
			out.push(hi << 4 | lo);
			i += 3;
			continue;
		}
		out.push(bytes[i]);
		i += 1;
	}
	String::from_utf8_lossy(&out).into_owned()
}

/// Joins an OPF-relative href onto the OPF directory, resolving dot segments so the result names a
/// real zip entry
fn join_zip_path(base: &str, href: &str) -> String {
	let mut segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
	for segment in href.split('/') {
		match segment {
			"" | "." => {}
			".." => {
				segments.pop();
			}
			_ => segments.push(segment),
		}
	}
	segments.join("/")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn make_epub(path: &Path, container: Option<&str>, opf_path: &str, opf: Option<&str>, entries: &[(&str, &[u8])]) {
		let file = std::fs::File::create(path).expect("create epub");
		let mut writer = zip::ZipWriter::new(file);
		if let Some(container) = container {
			writer
				.start_file("META-INF/container.xml", zip::write::SimpleFileOptions::default())
				.expect("start container");
			std::io::Write::write_all(&mut writer, container.as_bytes()).expect("write container");
		}
		if let Some(opf) = opf {
			writer
				.start_file(opf_path, zip::write::SimpleFileOptions::default())
				.expect("start opf");
			std::io::Write::write_all(&mut writer, opf.as_bytes()).expect("write opf");
		}
		for (name, bytes) in entries {
			writer
				.start_file(*name, zip::write::SimpleFileOptions::default())
				.expect("start entry");
			std::io::Write::write_all(&mut writer, bytes).expect("write entry");
		}
		writer.finish().expect("finish epub");
	}

	const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

	const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">x</dc:identifier>
    <dc:title>Spine Book</dc:title>
    <dc:creator>Author One</dc:creator>
    <dc:creator>Author Two</dc:creator>
    <dc:description>desc</dc:description>
    <dc:subject>manga</dc:subject>
    <dc:subject>action</dc:subject>
  </metadata>
  <manifest>
    <item id="p1" href="images/a.png" media-type="image/png"/>
    <item id="p2" href="images/b.jpg" media-type="image/jpeg"/>
    <item id="p3" href="images/c%20d.png" media-type="image/png"/>
    <item id="nav" href="text/nav.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
    <itemref idref="nav"/>
    <itemref idref="p2"/>
    <itemref idref="missing"/>
    <itemref idref="p3"/>
    <itemref idref="p1"/>
  </spine>
</package>"#;

	#[test]
	fn spine_pages_filter_resolve_and_keep_order() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some(OPF), &[]);
		let package = read_package(&ZipCache::new(), &path).expect("package");
		let pages = package.pages();
		let entries: Vec<&str> = pages.iter().map(|p| p.entry.as_str()).collect();
		assert_eq!(entries, ["OEBPS/images/b.jpg", "OEBPS/images/c d.png", "OEBPS/images/a.png"]);
		let media_types: Vec<&str> = pages.iter().map(|p| p.media_type.as_str()).collect();
		assert_eq!(media_types, ["image/jpeg", "image/png", "image/png"]);
	}

	#[test]
	fn metadata_is_exposed() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some(OPF), &[]);
		let metadata = &read_package(&ZipCache::new(), &path).expect("package").metadata;
		assert_eq!(metadata.title.as_deref(), Some("Spine Book"));
		assert_eq!(metadata.creators, ["Author One", "Author Two"]);
		assert_eq!(metadata.description.as_deref(), Some("desc"));
		assert_eq!(metadata.subjects, ["manga", "action"]);
	}

	#[test]
	fn opf_at_archive_root_resolves_bare_hrefs() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		let container = CONTAINER.replace("OEBPS/content.opf", "content.opf");
		make_epub(&path, Some(&container), "content.opf", Some(OPF), &[]);
		let package = read_package(&ZipCache::new(), &path).expect("package");
		let pages = package.pages();
		let entries: Vec<&str> = pages.iter().map(|p| p.entry.as_str()).collect();
		assert_eq!(entries, ["images/b.jpg", "images/c d.png", "images/a.png"]);
	}

	#[test]
	fn missing_or_broken_steps_yield_none() {
		let tmp = tempfile::tempdir().expect("tempdir");
		// no container.xml at all
		let path = tmp.path().join("a.epub");
		make_epub(&path, None, "OEBPS/content.opf", Some(OPF), &[]);
		assert!(read_package(&ZipCache::new(), &path).is_none());
		// container.xml is not xml
		let path = tmp.path().join("b.epub");
		make_epub(&path, Some("not xml <"), "OEBPS/content.opf", Some(OPF), &[]);
		assert!(read_package(&ZipCache::new(), &path).is_none());
		// the rootfile target does not exist
		let path = tmp.path().join("c.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/other.opf", Some(OPF), &[]);
		assert!(read_package(&ZipCache::new(), &path).is_none());
		// the OPF is not xml
		let path = tmp.path().join("d.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some("not xml <"), &[]);
		assert!(read_package(&ZipCache::new(), &path).is_none());
	}

	#[test]
	fn dot_segments_and_percent_decoding() {
		assert_eq!(join_zip_path("OEBPS", "./images/a.png"), "OEBPS/images/a.png");
		assert_eq!(join_zip_path("OEBPS/x", "../images/a.png"), "OEBPS/images/a.png");
		assert_eq!(join_zip_path("", "images/a.png"), "images/a.png");
		assert_eq!(percent_decode("c%20d.png"), "c d.png");
		assert_eq!(percent_decode("100%.png"), "100%.png");
		assert_eq!(percent_decode("%E4%B8%AD.png"), "中.png");
	}
}
