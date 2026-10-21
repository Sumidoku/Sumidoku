//! series.json sidecar parsing (flat schema: title, author/authors, artist/artists, description,
//! genres, status enum), mapped into the shared ComicInfo shape so the metadata chain treats it as
//! one more source. Unknown fields are ignored; an unparseable file warns and yields None, so a
//! broken sidecar never fails the scan — the chain falls through to the next source.

use serde::Deserialize;

use super::comic_info::ComicInfo;

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
	status: Option<String>,
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
}

fn cleaned(names: Option<Names>) -> Option<Vec<String>> {
	let out: Vec<String> = names?
		.into_vec()
		.into_iter()
		.map(|name| name.trim().to_string())
		.filter(|name| !name.is_empty())
		.collect();
	(!out.is_empty()).then_some(out)
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
		ComicInfo {
			series: self.title,
			writer: cleaned(self.authors.or(self.author)).map(|names| names.join(", ")),
			penciller: cleaned(self.artists.or(self.artist)).map(|names| names.join(", ")),
			summary: self.description,
			genre: cleaned(self.genres).map(|names| names.join(", ")),
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
	fn garbage_yields_none() {
		assert!(parse(b"{ not json").is_none());
	}
}
