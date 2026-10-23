//! Divina-profile EPUB reading for the local source: an EPUB is an OCF zip, so entries come through
//! the same ZipCache as CBZ. The page list is the OPF spine order restricted to image manifest
//! items; hrefs resolve against the OPF location into zip entry paths. Only the divina profile
//! (image-based books) is supported — a reflowable text document parses fine but yields no pages.

use aidoku::Viewer;
use quick_xml::de::from_reader;
use serde::Deserialize;
use std::path::Path;

use super::comic_info::{ComicInfo, CreatorNames, normalized_names};
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
	pub title: Vec<String>,
	#[serde(rename = "creator")]
	pub creators: Vec<Creator>,
	#[serde(rename = "description")]
	pub description: Vec<String>,
	#[serde(rename = "subject")]
	pub subjects: Vec<String>,
	#[serde(rename = "meta")]
	metas: Vec<Meta>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Creator {
	#[serde(rename = "@id")]
	id: Option<String>,
	#[serde(rename = "@role")]
	role: Option<String>,
	#[serde(rename = "$text")]
	name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Meta {
	#[serde(rename = "@property")]
	property: Option<String>,
	#[serde(rename = "@name")]
	name: Option<String>,
	#[serde(rename = "@content")]
	content: Option<String>,
	#[serde(rename = "@refines")]
	refines: Option<String>,
	#[serde(rename = "@scheme")]
	scheme: Option<String>,
	#[serde(rename = "$text")]
	text: String,
}

fn normalize_text(raw: &str) -> String {
	raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty(raw: &str) -> Option<String> {
	let text = normalize_text(raw);
	(!text.is_empty()).then_some(text)
}

fn clean_html(html: &str) -> String {
	aidoku_wasm::clean_html_text(html)
}

fn refined_role(metas: &[Meta], creator: &Creator) -> Option<String> {
	let id = creator.id.as_deref().filter(|id| !id.is_empty())?;
	metas
		.iter()
		.find(|meta| {
			meta.property.as_deref() == Some("role")
				&& meta.scheme.as_deref() == Some("marc:relators")
				&& meta.refines.as_deref().is_some_and(|refines| refines.trim_start_matches('#') == id)
		})
		.and_then(|meta| non_empty(&meta.text))
}

impl PackageMetadata {
	pub fn into_comic_info(self) -> ComicInfo {
		let series = self
			.metas
			.iter()
			.find(|meta| meta.property.as_deref() == Some("belongs-to-collection"))
			.and_then(|meta| non_empty(&meta.text))
			.or_else(|| {
				["calibre:series", "series"].iter().find_map(|name| {
					self.metas
						.iter()
						.find(|meta| meta.name.as_deref() == Some(*name))
						.and_then(|meta| meta.content.as_deref())
						.and_then(non_empty)
				})
			})
			.or_else(|| self.title.first().and_then(|title| non_empty(title)));
		let summary = self
			.description
			.first()
			.map(|description| clean_html(description))
			.filter(|description| !description.is_empty());
		let tag_names = normalized_names(self.subjects.iter().map(|subject| normalize_text(subject)).collect());
		let mut creators = CreatorNames::default();
		for creator in &self.creators {
			let Some(name) = non_empty(&creator.name) else {
				continue;
			};
			let role = creator
				.role
				.as_deref()
				.map(str::trim)
				.filter(|role| !role.is_empty())
				.map(str::to_string)
				.or_else(|| refined_role(&self.metas, creator));
			match role.as_deref() {
				Some("edt" | "trl") => {}
				Some("art" | "ill") => creators.penciller.push(name),
				Some("clr") => creators.colorist.push(name),
				Some("cov") => creators.cover_artist.push(name),
				_ => creators.writer.push(name),
			}
		}
		let (author_names, artist_names) = creators.into_lists();
		ComicInfo {
			series,
			summary,
			tag_names,
			author_names,
			artist_names,
			..ComicInfo::default()
		}
	}
}

impl Package {
	pub fn into_comic_info(self) -> ComicInfo {
		let viewer = match self.spine.page_progression_direction.as_deref() {
			Some("ltr") => Viewer::LeftToRight,
			Some("rtl") => Viewer::RightToLeft,
			_ => Viewer::Unknown,
		};
		ComicInfo {
			viewer,
			..self.metadata.into_comic_info()
		}
	}
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
	#[serde(rename = "@page-progression-direction")]
	page_progression_direction: Option<String>,
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
		assert_eq!(metadata.title.first().map(String::as_str), Some("Spine Book"));
		let creators: Vec<&str> = metadata.creators.iter().map(|creator| creator.name.as_str()).collect();
		assert_eq!(creators, ["Author One", "Author Two"]);
		assert_eq!(metadata.description.first().map(String::as_str), Some("desc"));
		assert_eq!(metadata.subjects, ["manga", "action"]);
	}

	#[test]
	fn metadata_maps_into_comic_info() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some(OPF), &[]);
		let info = read_package(&ZipCache::new(), &path)
			.expect("package")
			.metadata
			.into_comic_info();
		assert_eq!(info.title(), Some("Spine Book"));
		assert_eq!(info.authors().expect("authors"), vec!["Author One", "Author Two"]);
		assert_eq!(info.description().as_deref(), Some("desc"));
		assert_eq!(info.tags().expect("tags"), vec!["manga", "action"]);
		assert_eq!(info.artists(), None);
		assert_eq!(info.status(), 0);
	}

	#[test]
	fn repeated_title_and_description_use_first_and_keep_pages() {
		let opf = OPF
			.replace(
				"<dc:title>Spine Book</dc:title>",
				"<dc:title>First Title</dc:title><dc:title>Second Title</dc:title>",
			)
			.replace(
				"<dc:description>desc</dc:description>",
				"<dc:description>&lt;p&gt;First &lt;b&gt;description&lt;/b&gt;&lt;/p&gt;</dc:description><dc:description>Second description</dc:description>",
			);
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some(&opf), &[]);
		let package = read_package(&ZipCache::new(), &path).expect("package");
		let pages = package.pages();
		let entries: Vec<&str> = pages.iter().map(|page| page.entry.as_str()).collect();
		assert_eq!(entries, ["OEBPS/images/b.jpg", "OEBPS/images/c d.png", "OEBPS/images/a.png"]);
		let info = package.into_comic_info();
		assert_eq!(info.title(), Some("First Title"));
		assert_eq!(info.description().as_deref(), Some("First description"));
	}

	fn comic_info_from_opf(opf: &str) -> ComicInfo {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("x.epub");
		make_epub(&path, Some(CONTAINER), "OEBPS/content.opf", Some(opf), &[]);
		read_package(&ZipCache::new(), &path)
			.expect("package")
			.metadata
			.into_comic_info()
	}

	#[test]
	fn epub3_collection_wins_over_other_titles() {
		let info = comic_info_from_opf(
			r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Book Title</dc:title>
    <opf:meta property="belongs-to-collection">  EPUB3
      Series  </opf:meta>
    <opf:meta name="series" content="EPUB2 Series"/>
    <opf:meta name="calibre:series" content="Calibre Series"/>
  </metadata>
  <manifest/><spine/>
</package>"#,
		);
		assert_eq!(info.title(), Some("EPUB3 Series"));
	}

	#[test]
	fn epub2_series_fallbacks_and_dc_title() {
		let calibre = comic_info_from_opf(
			r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Book Title</dc:title>
    <meta name="series" content="EPUB2 Series"/>
    <meta name="calibre:series" content="Calibre Series"/>
  </metadata>
  <manifest/><spine/>
</package>"#,
		);
		assert_eq!(calibre.title(), Some("Calibre Series"));

		let bare = comic_info_from_opf(
			r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Book Title</dc:title>
    <meta name="series" content=" EPUB2 Series "/>
  </metadata>
  <manifest/><spine/>
</package>"#,
		);
		assert_eq!(bare.title(), Some("EPUB2 Series"));

		let title_only = comic_info_from_opf(OPF);
		assert_eq!(title_only.title(), Some("Spine Book"));
	}

	#[test]
	fn creator_roles_and_refines_map_to_authors_and_artists() {
		let info = comic_info_from_opf(
			r##"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:creator id="pen" opf:role="art"> Penciller,
      One </dc:creator>
    <dc:creator id="default">Last, First</dc:creator>
    <dc:creator id="editor" opf:role="edt">Ignored Editor</dc:creator>
    <dc:creator id="writer" opf:role="aut">OPF Writer</dc:creator>
    <opf:meta refines="#writer" property="role" scheme="marc:relators">art</opf:meta>
    <dc:creator id="color">Refined Colorist</dc:creator>
    <opf:meta refines="#color" property="role" scheme="marc:relators">clr</opf:meta>
    <dc:creator id="cover">Refined Cover</dc:creator>
    <opf:meta refines="#cover" property="role" scheme="marc:relators">cov</opf:meta>
    <dc:creator id="ill" opf:role="ill">Illustrator</dc:creator>
  </metadata>
  <manifest/><spine/>
</package>"##,
		);
		assert_eq!(info.authors().expect("authors"), vec!["Last, First", "OPF Writer"]);
		assert_eq!(
			info.artists().expect("artists"),
			vec!["Penciller, One", "Illustrator", "Refined Colorist", "Refined Cover"]
		);
	}

	#[test]
	fn description_is_cleaned_and_subjects_are_normalized() {
		let info = comic_info_from_opf(
			r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:description>&lt;p&gt; A &lt;b&gt;great&lt;/b&gt; book. &lt;/p&gt;&lt;p&gt;Second &amp;amp; final.&lt;/p&gt;</dc:description>
    <dc:subject> Action,
      Drama </dc:subject>
    <dc:subject>Action, Drama</dc:subject>
    <dc:subject> Mystery </dc:subject>
  </metadata>
  <manifest/><spine/>
</package>"#,
		);
		assert_eq!(info.description().as_deref(), Some("A great book. Second & final."));
		assert_eq!(info.tags().expect("tags"), vec!["Action, Drama", "Mystery"]);
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
