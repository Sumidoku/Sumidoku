//! Bibliographic metadata fetched from a tracker or another source, used to
//! write a library entry's metadata (#...). The wire model is shared by every
//! provider: each provider fills the fields it exposes and leaves the rest at
//! their defaults, and the metadata API marks empty fields unselectable.
//!
//! A tracker's title has one default variant plus labelled alternatives
//! (`native`, `romaji`, `english`, ...). The default is what the provider
//! considers canonical for this library: AniList prefers the native title,
//! Bangumi the Chinese name, MangaBaka the configured title language.

use aidoku::{ContentRating, MangaStatus};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleAlternative {
	pub label: String,
	pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TitleVariants {
	/// The provider's preferred title, used when the user does not pick another
	pub default: String,
	pub alternatives: Vec<TitleAlternative>,
}

impl TitleVariants {
	pub fn is_empty(&self) -> bool {
		self.default.is_empty()
	}

	/// The default followed by every alternative that differs from it
	pub fn values(&self) -> Vec<String> {
		let mut values = Vec::new();
		if !self.default.is_empty() {
			values.push(self.default.clone());
		}
		for alternative in &self.alternatives {
			if !alternative.value.is_empty() && !values.contains(&alternative.value) {
				values.push(alternative.value.clone());
			}
		}
		values
	}
}

/// One credited person under two spellings: the one an infobox credit list
/// uses, and the tracker's other spelling of the same person. Bangumi's
/// infobox lists credits under their original names only, so the structured
/// staff list is where the Chinese spelling comes from; AniList's staff
/// records carry both a native and a latin (`full`) name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonName {
	/// The spelling an infobox credit list uses
	pub name: String,
	/// The other spelling, absent when the tracker has none or both agree
	pub name_cn: Option<String>,
}

/// `names` rewritten in the alternate spellings `people` knows, or `None` when
/// not one of them has one. A credit the records do not cover keeps its
/// original spelling, so a partly translated list stays complete.
pub fn localized_names(people: &[PersonName], names: &[String]) -> Option<Vec<String>> {
	if people.is_empty() {
		return None;
	}
	let mut changed = false;
	let localized: Vec<String> = names
		.iter()
		.map(|name| match localized_name(people, name) {
			Some(localized) => {
				changed = true;
				localized.to_string()
			}
			None => name.clone(),
		})
		.collect();
	changed.then(|| dedupe(localized))
}

fn localized_name<'a>(people: &'a [PersonName], name: &str) -> Option<&'a str> {
	people
		.iter()
		.find(|person| person.name == name || person.name_cn.as_deref() == Some(name))
		.and_then(|person| person.name_cn.as_deref())
		.filter(|localized| !localized.trim().is_empty() && *localized != name)
}

/// One alternative spelling of a record's staff credits, under a label the
/// webui shows (`Original`, `Full`, ...). The two people fields share the label
/// because the records behind them do; a field the provider cannot spell
/// differently stays empty and offers no alternative.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffNames {
	pub label: String,
	pub authors: Vec<String>,
	pub artists: Vec<String>,
}

impl StaffNames {
	/// The list this field would take, if it differs from what it already has
	pub fn for_field(&self, field: StaffField, incoming: &[String]) -> Option<Vec<String>> {
		let names = match field {
			StaffField::Authors => &self.authors,
			StaffField::Artists => &self.artists,
		};
		(!names.is_empty() && names != incoming).then(|| names.clone())
	}
}

/// The two metadata fields that name people
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaffField {
	Authors,
	Artists,
}

/// One tracker entry's or source entry's metadata. Every field is optional
/// because capabilities differ; absent means "this provider cannot supply it".
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackerMetadata {
	pub id: String,
	pub title: TitleVariants,
	pub authors: Vec<String>,
	pub artists: Vec<String>,
	/// Other spellings of the authors and artists above; empty for providers
	/// whose records carry only one spelling
	pub staff: Vec<StaffNames>,
	pub description: Option<String>,
	pub tags: Vec<String>,
	pub status: Option<MangaStatus>,
	pub content_rating: Option<ContentRating>,
	pub cover_url: Option<String>,
	pub url: Option<String>,
	/// One line of publication facts (year, authors, format) for the preview candidate
	pub info: Option<String>,
}

impl TrackerMetadata {
	/// The value a field would receive before the user picks an alternative.
	pub fn title_default(&self) -> Option<&str> {
		(!self.title.default.is_empty()).then_some(self.title.default.as_str())
	}
}

/// `MangaStatus` <-> DB integer, mirroring `model::status_from_int`.
pub fn status_to_int(status: MangaStatus) -> i64 {
	match status {
		MangaStatus::Ongoing => 1,
		MangaStatus::Completed => 2,
		MangaStatus::Cancelled => 3,
		MangaStatus::Hiatus => 4,
		MangaStatus::Unknown => 0,
	}
}

/// `ContentRating` <-> DB integer, mirroring `model::content_rating_from_int`.
pub fn content_rating_to_int(rating: ContentRating) -> i64 {
	match rating {
		ContentRating::Safe => 1,
		ContentRating::Suggestive => 2,
		ContentRating::NSFW => 3,
		ContentRating::Unknown => 0,
	}
}

/// The AniList/MangaBaka publication statuses drawn from one vocabulary.
pub fn anilist_status(status: &str) -> Option<MangaStatus> {
	match status {
		"RELEASING" => Some(MangaStatus::Ongoing),
		"FINISHED" => Some(MangaStatus::Completed),
		"CANCELLED" => Some(MangaStatus::Cancelled),
		"HIATUS" => Some(MangaStatus::Hiatus),
		_ => None,
	}
}

pub fn mal_status(status: &str) -> Option<MangaStatus> {
	match status {
		"currently_publishing" => Some(MangaStatus::Ongoing),
		"finished" => Some(MangaStatus::Completed),
		"discontinued" => Some(MangaStatus::Cancelled),
		"on_hiatus" => Some(MangaStatus::Hiatus),
		_ => None,
	}
}

pub fn mangabaka_status(status: &str) -> Option<MangaStatus> {
	match status {
		"releasing" => Some(MangaStatus::Ongoing),
		"completed" => Some(MangaStatus::Completed),
		"cancelled" => Some(MangaStatus::Cancelled),
		"hiatus" => Some(MangaStatus::Hiatus),
		_ => None,
	}
}

/// Case-insensitive duplicate removal that keeps the first spelling.
pub fn dedupe(values: impl IntoIterator<Item = String>) -> Vec<String> {
	let mut seen = std::collections::HashSet::new();
	values
		.into_iter()
		.filter(|value| !value.trim().is_empty())
		.filter(|value| seen.insert(value.trim().to_lowercase()))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn title_alternatives_are_labelled_and_deduped() {
		let title = TitleVariants {
			default: "葬送のフリーレン".into(),
			alternatives: vec![
				TitleAlternative { label: "Romaji".into(), value: "Sousou no Frieren".into() },
				TitleAlternative { label: "English".into(), value: "Frieren: Beyond Journey's End".into() },
				TitleAlternative { label: "Native".into(), value: "葬送のフリーレン".into() },
			],
		};
		assert_eq!(
			title.values(),
			vec!["葬送のフリーレン".to_string(), "Sousou no Frieren".to_string(), "Frieren: Beyond Journey's End".to_string()]
		);
	}

	#[test]
	fn capability_statuses_map_to_library_integers() {
		assert_eq!(status_to_int(MangaStatus::Ongoing), 1);
		assert_eq!(status_to_int(MangaStatus::Completed), 2);
		assert_eq!(status_to_int(MangaStatus::Cancelled), 3);
		assert_eq!(status_to_int(MangaStatus::Hiatus), 4);
		assert_eq!(status_to_int(MangaStatus::Unknown), 0);
		assert_eq!(content_rating_to_int(ContentRating::NSFW), 3);
		assert_eq!(content_rating_to_int(ContentRating::Safe), 1);
	}

	#[test]
	fn provider_status_strings_stay_separate() {
		assert_eq!(anilist_status("FINISHED"), Some(MangaStatus::Completed));
		assert_eq!(mal_status("currently_publishing"), Some(MangaStatus::Ongoing));
		assert_eq!(mangabaka_status("hiatus"), Some(MangaStatus::Hiatus));
		assert_eq!(mangabaka_status("finished"), None);
	}

	#[test]
	fn metadata_wire_model_is_camel_case_with_defaults() {
		let metadata = TrackerMetadata {
			id: "42".into(),
			title: TitleVariants { default: "Title".into(), alternatives: Vec::new() },
			authors: vec!["Author".into()],
			status: Some(MangaStatus::Ongoing),
			cover_url: Some("https://img.example/cover.jpg".into()),
			..TrackerMetadata::default()
		};
		let wire = serde_json::to_value(&metadata).unwrap();
		assert_eq!(wire["coverUrl"], "https://img.example/cover.jpg");
		assert_eq!(wire["status"], "Ongoing");
		assert_eq!(wire["title"]["default"], "Title");
		assert_eq!(serde_json::from_str::<TrackerMetadata>("{}").unwrap(), TrackerMetadata::default());
		assert_eq!(dedupe(vec!["A".into(), "a".into(), " B ".into(), "B".into()]), vec!["A".to_string(), " B ".to_string()]);
	}

	#[test]
	fn localized_names_rewrite_only_known_people() {
		let people = vec![
			PersonName { name: "尾田栄一郎".into(), name_cn: Some("尾田荣一郎".into()) },
			PersonName { name: "Studio Gaga".into(), name_cn: Some("Studio Gaga".into()) },
		];
		assert_eq!(
			localized_names(&people, &["尾田栄一郎".into(), "Studio Gaga".into()]),
			Some(vec!["尾田荣一郎".to_string(), "Studio Gaga".to_string()])
		);
		// nothing to translate is reported as such, so the preview offers no choice
		assert_eq!(localized_names(&people, &["Studio Gaga".into()]), None);
		assert_eq!(localized_names(&[], &["尾田栄一郎".into()]), None);
	}

	#[test]
	fn a_staff_alternative_is_offered_only_when_it_changes_the_field() {
		let staff = StaffNames {
			label: "NameCN".into(),
			authors: vec!["尾田荣一郎".into()],
			artists: vec!["A".into()],
		};
		let incoming = ["尾田栄一郎".to_string()];
		assert_eq!(staff.for_field(StaffField::Authors, &incoming), Some(vec!["尾田荣一郎".to_string()]));
		// an empty list for a field is no alternative for that field
		assert_eq!(staff.for_field(StaffField::Artists, &incoming), Some(vec!["A".to_string()]));
		assert_eq!(staff.for_field(StaffField::Authors, &["尾田荣一郎".to_string()]), None);
	}
}
