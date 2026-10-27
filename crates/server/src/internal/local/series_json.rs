//! series.json sidecar parsing (the flat schema and Mylar's nested metadata object), mapped into
//! the shared ComicInfo shape so the metadata chain treats it as one more source. Unknown fields
//! are ignored; an unparseable file warns and yields None, so a broken sidecar never fails the
//! scan — the chain falls through to the next source.

use aidoku::{ContentRating, Viewer};
use serde::Deserialize;

use super::comic_info::{ComicInfo, CreatorNames, normalized_names};

pub const SERIES_JSON_FILE: &str = "series.json";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SeriesJson {
	title: Option<String>,
	author: Option<Names>,
	authors: Option<Names>,
	artist: Option<Names>,
	artists: Option<Names>,
	description: Option<String>,
	genres: Option<Names>,
	tags: Option<Names>,
	status: Option<String>,
	metadata: Option<MylarMetadata>,
}

#[derive(Debug, Deserialize)]
struct MylarMetadata {
	name: String,
	year: i32,
	volume: Option<i32>,
	description_text: Option<String>,
	description_formatted: Option<String>,
	status: Option<String>,
	#[serde(rename = "readingDirection")]
	reading_direction: Option<String>,
	genres: Option<Names>,
	tags: Option<Names>,
	age_rating: Option<String>,
	#[serde(default)]
	authors: Vec<MylarAuthor>,
}

#[derive(Debug, Deserialize)]
struct MylarAuthor {
	name: String,
	role: Option<String>,
}

/// Hand-written sidecars mix the singular/plural spellings with the scalar/list shapes, so every
/// combination parses; the plural spelling wins when both are present
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Names {
	One(String),
	Many(Vec<String>),
}

impl Names {
	fn into_vec(self) -> Vec<String> {
		match self {
			Names::One(one) => vec![one],
			Names::Many(many) => many,
		}
	}

	fn into_flat_vec(self) -> Vec<String> {
		match self {
			Names::One(one) => one.split(',').map(str::to_string).collect(),
			Names::Many(many) => many,
		}
	}
}

fn cleaned_flat(names: Option<Names>) -> Option<Vec<String>> {
	normalized_names(names?.into_flat_vec())
}

/// Parse failure returns None (the caller falls through to the next metadata source)
pub fn parse(bytes: &[u8]) -> Option<ComicInfo> {
	let json: SeriesJson = match serde_json::from_slice(bytes) {
		Ok(json) => json,
		Err(e) => {
			tracing::warn!("unparsable {SERIES_JSON_FILE}: {e}");
			return None;
		}
	};
	Some(json.into_comic_info())
}

impl SeriesJson {
	fn into_comic_info(self) -> ComicInfo {
		if let Some(metadata) = self.metadata {
			return metadata.into_comic_info();
		}
		ComicInfo {
			series: self.title,
			author_names: cleaned_flat(self.authors.or(self.author)),
			artist_names: cleaned_flat(self.artists.or(self.artist)),
			summary: self.description,
			tag_names: {
				let mut combined = Vec::new();
				if let Some(g) = self.genres {
					combined.extend(g.into_flat_vec());
				}
				if let Some(t) = self.tags {
					combined.extend(t.into_flat_vec());
				}
				cleaned_flat(Some(Names::Many(combined)))
			},
			// mapped through the Tachiyomi spellings so ComicInfo::status() yields the aidoku number
			status: self
				.status
				.as_deref()
				.and_then(|status| match status.trim().to_ascii_uppercase().as_str() {
					"ONGOING" => Some("Ongoing"),
					"COMPLETED" => Some("Completed"),
					"CANCELLED" => Some("Cancelled"),
					"HIATUS" => Some("On hiatus"),
					_ => None,
				})
				.map(str::to_string),
			..ComicInfo::default()
		}
	}
}

impl MylarMetadata {
	fn into_comic_info(self) -> ComicInfo {
		let series = if self.volume.is_none_or(|volume| volume == 1) {
			self.name
		} else {
			format!("{} ({})", self.name, self.year)
		};
		let status = self.status.as_deref().and_then(|status| match status {
			"Ended" => Some("Completed"),
			"Continuing" => Some("Ongoing"),
			_ => None,
		});
		let content_rating = match self.age_rating.as_deref() {
			Some("All" | "9+" | "12+") => ContentRating::Safe,
			Some("15+" | "17+") => ContentRating::Suggestive,
			Some("Adult") => ContentRating::NSFW,
			_ => ContentRating::Unknown,
		};
		let viewer = match self.reading_direction.as_deref() {
			Some("LEFT_TO_RIGHT") => Viewer::LeftToRight,
			Some("RIGHT_TO_LEFT") => Viewer::RightToLeft,
			Some("VERTICAL") => Viewer::Vertical,
			Some("WEBTOON") => Viewer::Webtoon,
			_ => Viewer::Unknown,
		};
		let mut creators = CreatorNames::default();
		for author in self.authors {
			let name = author.name.trim();
			if name.is_empty() {
				continue;
			}
			match author.role.as_deref().map(str::trim).unwrap_or("writer") {
				"writer" => creators.writer.push(name.to_string()),
				"penciller" => creators.penciller.push(name.to_string()),
				"inker" => creators.inker.push(name.to_string()),
				"colorist" => creators.colorist.push(name.to_string()),
				"letterer" => creators.letterer.push(name.to_string()),
				"cover" | "cover_artist" => creators.cover_artist.push(name.to_string()),
				_ => {}
			}
		}
		let (author_names, artist_names) = creators.into_lists();
		ComicInfo {
			series: Some(series),
			summary: self.description_formatted.or(self.description_text),
			tag_names: normalized_names(self.genres.into_iter().chain(self.tags).flat_map(Names::into_vec).collect()),
			status: status.map(str::to_string),
			author_names,
			artist_names,
			content_rating,
			viewer,
			..ComicInfo::default()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn full_mapping_plural_spellings() {
		let json = r#"{
  "title": "Theta Series",
  "authors": ["Author One", "Author Two"],
  "artists": ["Artist One", " Artist Two "],
  "description": "desc",
  "genres": ["Action", "Drama"],
  "status": "COMPLETED",
  "publisher": "unknown fields are ignored"
}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.title(), Some("Theta Series"));
		assert_eq!(info.authors().expect("authors"), vec!["Author One", "Author Two"]);
		assert_eq!(info.artists().expect("artists"), vec!["Artist One", "Artist Two"]);
		assert_eq!(info.description().as_deref(), Some("desc"));
		assert_eq!(info.tags().expect("tags"), vec!["Action", "Drama"]);
		assert_eq!(info.status(), 2);
	}

	#[test]
	fn singular_spellings_and_case_insensitive_status() {
		let json = r#"{"title": "Iota", "author": "Solo Author", "artist": "Solo Artist", "status": "hiatus"}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.authors().expect("authors"), vec!["Solo Author"]);
		assert_eq!(info.artists().expect("artists"), vec!["Solo Artist"]);
		assert_eq!(info.status(), 4);
	}

	#[test]
	fn mixed_shapes_and_plural_wins() {
		// a scalar plural and a list singular both parse; both spellings present → the plural wins
		let json = r#"{"authors": "Only Author", "author": ["Ignored", "Also Ignored"], "genres": "Action", "status": "Ongoing"}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.authors().expect("authors"), vec!["Only Author"]);
		assert_eq!(info.tags().expect("tags"), vec!["Action"]);
		assert_eq!(info.status(), 1);
	}

	#[test]
	fn unknown_status_maps_to_unknown() {
		let json = r#"{"status": "DROPPED"}"#;
		assert_eq!(parse(json.as_bytes()).expect("parse").status(), 0);
	}

	#[test]
	fn mylar_nested_metadata_maps_into_shared_shape() {
		let json = r#"{
  "version": "1.0.1",
  "metadata": {
    "name": "Berserk",
    "comicid": 9527,
    "year": 1989,
    "volume": 2,
    "total_issues": "41",
    "description_text": "plain",
    "description_formatted": "<p>formatted</p>",
    "status": "Ended",
    "readingDirection": "RIGHT_TO_LEFT",
    "tags": ["Action, Drama", "Seinen"],
    "age_rating": "17+",
    "authors": [
      {"name": "Last, First"},
      {"name": "Writer Explicit", "role": "writer"},
      {"name": "Penciller, One", "role": "penciller"},
      {"name": "Inker", "role": "inker"},
      {"name": "Colorist", "role": "colorist"},
      {"name": "Letterer", "role": "letterer"},
      {"name": "Cover Artist", "role": "cover_artist"}
    ],
    "publisher": "unknown fields are ignored"
  }
}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.title(), Some("Berserk (1989)"));
		assert_eq!(info.description().as_deref(), Some("<p>formatted</p>"));
		assert_eq!(info.authors().expect("authors"), vec!["Last, First", "Writer Explicit"]);
		assert_eq!(
			info.artists().expect("artists"),
			vec!["Penciller, One", "Inker", "Colorist", "Letterer", "Cover Artist"]
		);
		assert_eq!(info.tags().expect("tags"), vec!["Action, Drama", "Seinen"]);
		assert_eq!(info.status(), 2);
	}

	#[test]
	fn mylar_volume_and_description_fallbacks() {
		let no_volume = r#"{"metadata":{"name":"Berserk","year":1989,"description_text":"plain","status":"Continuing"}}"#;
		let info = parse(no_volume.as_bytes()).expect("parse without volume");
		assert_eq!(info.title(), Some("Berserk"));
		assert_eq!(info.description().as_deref(), Some("plain"));
		assert_eq!(info.status(), 1);

		let volume_one = r#"{"metadata":{"name":"Berserk","comicid":"9527","year":1989,"volume":1,"total_issues":41,"description_text":"plain","description_formatted":null}}"#;
		let info = parse(volume_one.as_bytes()).expect("parse volume one");
		assert_eq!(info.title(), Some("Berserk"));
		assert_eq!(info.description().as_deref(), Some("plain"));
	}

	#[test]
	fn mylar_genres_and_tags_combine_and_dedupe() {
		let json = r#"{"metadata":{"name":"Berserk","year":1989,"genres":[" Action ","","Seinen"],"tags":["Action, Drama","Seinen","Action"," "]}}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.tags().expect("tags"), vec!["Action", "Seinen", "Action, Drama"]);
	}

	#[test]
	fn flat_schema_remains_compatible() {
		let json = r#"{"title":"Flat Series","name":"Ignored","author":"A, B","artists":["Artist, One"],"description":"flat desc","genres":"Action, Drama","status":"ONGOING"}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.title(), Some("Flat Series"));
		assert_eq!(info.authors().expect("authors"), vec!["A", "B"]);
		assert_eq!(info.artists().expect("artists"), vec!["Artist, One"]);
		assert_eq!(info.description().as_deref(), Some("flat desc"));
		assert_eq!(info.tags().expect("tags"), vec!["Action", "Drama"]);
		assert_eq!(info.status(), 1);
	}

	#[test]
	fn flat_tags_combine_with_genres() {
		let json = r#"{"title":"Flat Tags","genres":["Action"],"tags":["Drama","Adventure"]}"#;
		let info = parse(json.as_bytes()).expect("parse");
		assert_eq!(info.tags().expect("tags"), vec!["Action", "Drama", "Adventure"]);
	}

	#[test]
	fn garbage_yields_none() {
		assert!(parse(b"{ not json").is_none());
	}
}
