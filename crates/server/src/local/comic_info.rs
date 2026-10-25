//! ComicInfo.xml reading and mapping (the counterpart of Suwayomi LocalSource's `copyFromComicInfo`):
//! title←Series, author←Writer, artist←Penciller/Inker/Colorist/Letterer/CoverArtist,
//! description←Summary, tags←Genre+Tags+Categories, status←PublishingStatusTachiyomi.
//! Tachiyomi extension fields carry a `ty:` namespace prefix, and quick-xml matches element names literally, so both spellings are accepted.

use aidoku::{ContentRating, Viewer};
use serde::Deserialize;

pub const COMIC_INFO_FILE: &str = "ComicInfo.xml";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ComicInfo {
	#[serde(rename = "Series")]
	pub series: Option<String>,
	#[serde(rename = "Summary")]
	pub summary: Option<String>,
	#[serde(rename = "Writer")]
	pub writer: Option<String>,
	#[serde(rename = "Penciller")]
	pub penciller: Option<String>,
	#[serde(rename = "Inker")]
	pub inker: Option<String>,
	#[serde(rename = "Colorist")]
	pub colorist: Option<String>,
	#[serde(rename = "Letterer")]
	pub letterer: Option<String>,
	#[serde(rename = "CoverArtist")]
	pub cover_artist: Option<String>,
	#[serde(rename = "Genre")]
	pub genre: Option<String>,
	#[serde(rename = "Tags")]
	pub tags: Option<String>,
	#[serde(rename = "Categories")]
	pub categories: Option<String>,
	#[serde(rename = "ty:Categories")]
	pub categories_ty: Option<String>,
	#[serde(rename = "PublishingStatusTachiyomi")]
	pub status: Option<String>,
	#[serde(rename = "ty:PublishingStatusTachiyomi")]
	pub status_ty: Option<String>,
	#[serde(skip)]
	pub author_names: Option<Vec<String>>,
	#[serde(skip)]
	pub artist_names: Option<Vec<String>>,
	#[serde(skip)]
	pub tag_names: Option<Vec<String>>,
	#[serde(skip)]
	pub content_rating: ContentRating,
	#[serde(skip)]
	pub viewer: Viewer,
}

#[derive(Debug, Default)]
pub(crate) struct CreatorNames {
	pub writer: Vec<String>,
	pub penciller: Vec<String>,
	pub inker: Vec<String>,
	pub colorist: Vec<String>,
	pub letterer: Vec<String>,
	pub cover_artist: Vec<String>,
}

impl CreatorNames {
	pub(crate) fn into_lists(self) -> (Option<Vec<String>>, Option<Vec<String>>) {
		let artists = [self.penciller, self.inker, self.colorist, self.letterer, self.cover_artist].concat();
		(normalized_names(self.writer), normalized_names(artists))
	}
}

/// Parse failure returns None (the caller proceeds with no metadata, using the directory name as title)
pub fn parse(bytes: &[u8]) -> Option<ComicInfo> {
	match quick_xml::de::from_reader::<_, ComicInfo>(bytes) {
		Ok(info) => Some(info),
		Err(e) => {
			tracing::warn!("unparsable {COMIC_INFO_FILE}: {e}");
			None
		}
	}
}

pub(crate) fn normalized_names(names: Vec<String>) -> Option<Vec<String>> {
	let mut out: Vec<String> = Vec::new();
	for name in names {
		let name = name.trim();
		if !name.is_empty() && !out.iter().any(|seen| seen == name) {
			out.push(name.to_string());
		}
	}
	(!out.is_empty()).then_some(out)
}

/// Comma-separated list → blanks removed and deduped (first-seen order kept); None/all-blank → None
fn split_list(parts: &[Option<&str>]) -> Option<Vec<String>> {
	let mut out: Vec<String> = Vec::new();
	for part in parts.iter().flatten() {
		for item in part.split(',') {
			let item = item.trim();
			if !item.is_empty() && !out.iter().any(|seen| seen == item) {
				out.push(item.to_string());
			}
		}
	}
	(!out.is_empty()).then_some(out)
}

impl ComicInfo {
	pub fn title(&self) -> Option<&str> {
		self.series.as_deref().map(str::trim).filter(|s| !s.is_empty())
	}

	pub fn authors(&self) -> Option<Vec<String>> {
		self.author_names.clone().or_else(|| split_list(&[self.writer.as_deref()]))
	}

	pub fn artists(&self) -> Option<Vec<String>> {
		self.artist_names.clone().or_else(|| {
			split_list(&[
				self.penciller.as_deref(),
				self.inker.as_deref(),
				self.colorist.as_deref(),
				self.letterer.as_deref(),
				self.cover_artist.as_deref(),
			])
		})
	}

	pub fn tags(&self) -> Option<Vec<String>> {
		self.tag_names.clone().or_else(|| {
			split_list(&[
				self.genre.as_deref(),
				self.tags.as_deref(),
				self.categories.as_deref(),
				self.categories_ty.as_deref(),
			])
		})
	}

	pub fn description(&self) -> Option<String> {
		self.summary
			.as_deref()
			.map(str::trim)
			.filter(|s| !s.is_empty())
			.map(str::to_string)
	}

	/// aidoku MangaStatus number: Licensed has no aidoku counterpart, mapped to Unknown
	pub fn status(&self) -> i64 {
		match self.status_ty.as_deref().or(self.status.as_deref()) {
			Some("Ongoing") => 1,
			Some("Completed") | Some("Publishing finished") => 2,
			Some("Cancelled") => 3,
			Some("On hiatus") => 4,
			_ => 0,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn full_mapping() {
		let xml = r#"<?xml version="1.0"?>
<ComicInfo xmlns:ty="http://www.w3.org/2001/XMLSchema">
  <Series>Alpha Series</Series>
  <Summary>  desc text  </Summary>
  <Writer>w1, w2</Writer>
  <Penciller>p1</Penciller>
  <Inker>i1, p1</Inker>
  <CoverArtist>ca1</CoverArtist>
  <Genre>Action, Adventure</Genre>
  <Tags>dark</Tags>
  <ty:Categories>seinen</ty:Categories>
  <ty:PublishingStatusTachiyomi>Completed</ty:PublishingStatusTachiyomi>
</ComicInfo>"#;
		let info = parse(xml.as_bytes()).expect("parse");
		assert_eq!(info.title(), Some("Alpha Series"));
		assert_eq!(info.description().as_deref(), Some("desc text"));
		assert_eq!(info.authors().expect("authors"), vec!["w1", "w2"]);
		assert_eq!(info.artists().expect("artists"), vec!["p1", "i1", "ca1"]);
		assert_eq!(info.tags().expect("tags"), vec!["Action", "Adventure", "dark", "seinen"]);
		assert_eq!(info.status(), 2);
	}

	#[test]
	fn plain_status_element_and_blanks() {
		let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series></Series>
  <PublishingStatusTachiyomi>On hiatus</PublishingStatusTachiyomi>
</ComicInfo>"#;
		let info = parse(xml.as_bytes()).expect("parse");
		assert_eq!(info.title(), None);
		assert_eq!(info.status(), 4);
		assert_eq!(info.authors(), None);
		assert_eq!(info.tags(), None);
	}

	#[test]
	fn unknown_status_maps_to_unknown() {
		let xml = r#"<?xml version="1.0"?><ComicInfo><ty:PublishingStatusTachiyomi xmlns:ty="http://www.w3.org/2001/XMLSchema">Licensed</ty:PublishingStatusTachiyomi></ComicInfo>"#;
		assert_eq!(parse(xml.as_bytes()).expect("parse").status(), 0);
	}

	#[test]
	fn garbage_yields_none() {
		assert!(parse(b"not xml").is_none());
	}
}
