//! Tracker progress primitives for reading-status sync (kmrs analogue, see #35).
//! Pure and side-effect free: given the tracker-side state and the local read
//! set, decide what to push. Unlike kmrs there is no filename parsing layer —
//! Sumidoku chapters carry structured `number`/`volume` from their source.
//!
//! Progress only ever moves forward, and the completed check compares against
//! the new (about-to-be-pushed) value, so finishing the final chapter marks
//! completed on that very event.

use chrono::NaiveDate;

/// How a series' read progress maps onto tracker fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackMode {
	/// Decide from the series' chapter numbers, chapter-first.
	#[default]
	Auto,
	/// Push `last_read_chapter`.
	Chapter,
	/// Push `last_read_volume`.
	Volume,
}

impl TrackMode {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Auto => "auto",
			Self::Chapter => "chapter",
			Self::Volume => "volume",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"auto" => Some(Self::Auto),
			"chapter" => Some(Self::Chapter),
			"volume" => Some(Self::Volume),
			_ => None,
		}
	}
}

/// Unified reading status; lowercase on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackStatus {
	Reading,
	Planning,
	Completed,
	Paused,
	Dropped,
	Rereading,
}

impl TrackStatus {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Reading => "reading",
			Self::Planning => "planning",
			Self::Completed => "completed",
			Self::Paused => "paused",
			Self::Dropped => "dropped",
			Self::Rereading => "rereading",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"reading" => Some(Self::Reading),
			"planning" => Some(Self::Planning),
			"completed" => Some(Self::Completed),
			"paused" => Some(Self::Paused),
			"dropped" => Some(Self::Dropped),
			"rereading" => Some(Self::Rereading),
			_ => None,
		}
	}
}

/// Tracker-side current tracking state (camelCase).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteState {
	pub score: Option<i32>,
	pub status: Option<TrackStatus>,
	pub last_read_chapter: Option<f32>,
	pub last_read_volume: Option<i32>,
	pub total_chapters: Option<i32>,
	pub total_volumes: Option<i32>,
	pub start_read_date: Option<String>,
	pub finish_read_date: Option<String>,
}

/// The push payload (camelCase on the wire).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackUpdate {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub score: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub status: Option<TrackStatus>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub last_read_chapter: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub last_read_volume: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub start_read_date: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub finish_read_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackSearchItem {
	pub id: String,
	pub title: String,
	pub cover_url: Option<String>,
	pub description: Option<String>,
	pub tracked: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub media_type: Option<TrackMediaType>,
	pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackMediaType {
	Manga,
	Novel,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackLedgerItem {
	pub provider: String,
	pub track_id: String,
	pub title: Option<String>,
	pub url: Option<String>,
	pub cover_url: Option<String>,
	pub updated_at: i64,
}

/// One chapter's contribution to the series aggregate. Sources may leave
/// either number NULL (chapter-only numbering, volume-only numbering, or no
/// numbering at all), so each mode falls back to the other signal.
#[derive(Clone, Copy)]
pub struct ChapterTrackInput {
	pub number: Option<f64>,
	pub volume: Option<f64>,
	pub read: bool,
}

impl ChapterTrackInput {
	fn chapter_value(self) -> Option<f64> {
		self.number.or(self.volume)
	}

	fn volume_value(self) -> Option<f64> {
		self.volume.or_else(|| self.number.map(f64::floor))
	}
}

/// Resolve `Auto` against the whole series: any chapter-number signal makes it
/// a chapter series, else any volume signal makes it a volume series, else the
/// chapter fallback for series with no numbering at all.
pub fn resolve_series_mode(inputs: &[ChapterTrackInput]) -> TrackMode {
	if inputs.iter().any(|c| c.number.is_some()) {
		TrackMode::Chapter
	} else if inputs.iter().any(|c| c.volume.is_some()) {
		TrackMode::Volume
	} else {
		TrackMode::Chapter
	}
}

/// The effective mode of one binding: `Auto` resolves against the series.
pub fn effective_mode(mode: TrackMode, inputs: &[ChapterTrackInput]) -> TrackMode {
	match mode {
		TrackMode::Auto => resolve_series_mode(inputs),
		explicit => explicit,
	}
}

/// Aggregated read progress of a series: both signals are computed, the caller
/// pushes only the one matching the effective mode.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProgressValue {
	pub chapter: Option<f64>,
	pub volume: Option<i32>,
}

pub fn aggregate(inputs: &[ChapterTrackInput]) -> ProgressValue {
	let mut value = ProgressValue::default();
	for chapter in inputs.iter().filter(|c| c.read) {
		if let Some(c) = chapter.chapter_value() {
			value.chapter = Some(value.chapter.map_or(c, |acc: f64| acc.max(c)));
		}
		if let Some(v) = chapter.volume_value() {
			value.volume = Some(value.volume.map_or(v as i32, |acc: i32| acc.max(v as i32)));
		}
	}
	value
}

/// Decide what to push for one binding, or `None` when there is nothing worth
/// pushing (no read chapter, or local progress does not advance the remote
/// one). `today` is the local date used for start/finish dates. `mode` must
/// already be resolved (`effective_mode`) — `Auto` panics.
pub fn decide(
	mode: TrackMode,
	chapter_offset: i32,
	remote: &RemoteState,
	inputs: &[ChapterTrackInput],
	today: NaiveDate,
) -> Option<TrackUpdate> {
	let progress = aggregate(inputs);
	let today = today.format("%Y-%m-%d").to_string();

	// only push when local progress strictly advances the remote value
	let (new_chapter, new_volume) = match mode {
		TrackMode::Chapter => {
			let new = progress.chapter?;
			let adjusted = apply_chapter_offset(new, chapter_offset, remote.total_chapters);
			if adjusted <= f64::from(remote.last_read_chapter.unwrap_or(0.0)) {
				return None;
			}
			(Some(adjusted as f32), None)
		}
		TrackMode::Volume => {
			let new = progress.volume?;
			if Some(new) <= remote.last_read_volume {
				return None;
			}
			(None, Some(new))
		}
		TrackMode::Auto => unreachable!("callers resolve auto first"),
	};

	let completed = match mode {
		TrackMode::Chapter => new_chapter
			.zip(remote.total_chapters)
			.is_some_and(|(c, total)| f64::from(c).floor() == f64::from(total)),
		TrackMode::Volume => new_volume.is_some() && new_volume == remote.total_volumes,
		TrackMode::Auto => false,
	};

	let mut update = TrackUpdate {
		last_read_chapter: new_chapter,
		last_read_volume: new_volume,
		..TrackUpdate::default()
	};
	if completed {
		if remote.finish_read_date.is_none() {
			update.finish_read_date = Some(today.clone());
		}
		update.status = Some(TrackStatus::Completed);
	} else if remote.status != Some(TrackStatus::Reading) && remote.status != Some(TrackStatus::Rereading) {
		// not currently reading: opening a chapter starts the entry, and
		// re-reading a completed entry switches to rereading
		if remote.start_read_date.is_none() && matches!(remote.status, None | Some(TrackStatus::Planning)) {
			update.start_read_date = Some(today);
		}
		update.status = Some(if remote.status == Some(TrackStatus::Completed) {
			TrackStatus::Rereading
		} else {
			TrackStatus::Reading
		});
	}
	Some(update)
}

/// Observed chapters may exceed a stale tracker total; only overshoot
/// introduced by the offset is clamped.
fn apply_chapter_offset(chapter: f64, offset: i32, total: Option<i32>) -> f64 {
	let adjusted = (chapter + f64::from(offset)).max(0.0);
	match total {
		Some(total) if chapter <= f64::from(total) => adjusted.min(f64::from(total)),
		_ => adjusted,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn chapter(number: Option<f64>, volume: Option<f64>, read: bool) -> ChapterTrackInput {
		ChapterTrackInput { number, volume, read }
	}

	fn day() -> NaiveDate {
		NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
	}

	#[test]
	fn wire_models_match_tracker_contracts() {
		let update: TrackUpdate = serde_json::from_value(serde_json::json!({
			"score": 80,
			"status": "rereading",
			"title": "Title",
			"coverUrl": "https://img.example/cover.jpg"
		}))
		.unwrap();
		assert_eq!(serde_json::to_value(update).unwrap(), serde_json::json!({"score": 80, "status": "rereading"}));
		assert_eq!(serde_json::to_value(TrackUpdate::default()).unwrap(), serde_json::json!({}));

		let state = RemoteState {
			score: Some(8),
			status: Some(TrackStatus::Reading),
			last_read_chapter: Some(12.5),
			..RemoteState::default()
		};
		let wire = serde_json::to_value(&state).unwrap();
		assert_eq!(wire["score"], 8);
		assert_eq!(wire["status"], "reading");
		assert_eq!(wire["lastReadChapter"], 12.5);
		assert!(wire.get("totalChapters").is_some_and(|v| v.is_null()));
		assert_eq!(serde_json::from_str::<RemoteState>("{}").unwrap(), RemoteState::default());

		let item = TrackSearchItem {
			id: "42".into(),
			title: "Title".into(),
			cover_url: None,
			description: None,
			tracked: true,
			media_type: Some(TrackMediaType::Novel),
			url: Some("https://tracker.example/42".into()),
		};
		let wire = serde_json::to_value(&item).unwrap();
		assert_eq!(wire["mediaType"], "novel");
		assert!(wire.get("coverUrl").is_some_and(|v| v.is_null()));
		assert_eq!(wire["id"], "42");
		let mut without_type = serde_json::to_value(TrackSearchItem { media_type: None, ..item }).unwrap();
		assert!(without_type.as_object_mut().unwrap().remove("mediaType").is_none());

		let ledger = TrackLedgerItem {
			provider: "mal".into(),
			track_id: "42".into(),
			title: None,
			url: None,
			cover_url: None,
			updated_at: 1_700_000_000,
		};
		let wire = serde_json::to_value(ledger).unwrap();
		assert_eq!(wire["trackId"], "42");
		assert_eq!(wire["updatedAt"], 1_700_000_000);
	}

	#[test]
	fn automatic_update_omits_unset_wire_fields() {
		let remote = RemoteState { status: Some(TrackStatus::Reading), last_read_chapter: Some(10.0), ..RemoteState::default() };
		let chapters = vec![chapter(Some(11.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(serde_json::to_value(update).unwrap(), serde_json::json!({"lastReadChapter": 11.0}));
	}

	#[test]
	fn auto_mode_resolves_chapter_first_then_volume_then_chapter_fallback() {
		let volumes = vec![chapter(None, Some(1.0), false), chapter(None, Some(2.0), false)];
		assert_eq!(resolve_series_mode(&volumes), TrackMode::Volume);
		let mixed = vec![chapter(None, Some(3.0), false), chapter(Some(12.0), None, false)];
		assert_eq!(resolve_series_mode(&mixed), TrackMode::Chapter);
		let unknown = vec![chapter(None, None, false)];
		assert_eq!(resolve_series_mode(&unknown), TrackMode::Chapter);
		assert_eq!(effective_mode(TrackMode::Auto, &unknown), TrackMode::Chapter);
		assert_eq!(effective_mode(TrackMode::Volume, &mixed), TrackMode::Volume);
	}

	#[test]
	fn aggregate_uses_fallbacks_per_chapter() {
		let chapters = vec![
			chapter(None, Some(3.0), true),  // chapter fallback → 3
			chapter(Some(12.0), None, true), // chapter 12, volume floor 12
			chapter(None, None, true),       // no signal contributes nothing
			chapter(Some(99.0), None, false), // unread contributes nothing
		];
		let value = aggregate(&chapters);
		assert_eq!(value.chapter, Some(12.0));
		assert_eq!(value.volume, Some(12));
	}

	fn empty_remote() -> RemoteState {
		RemoteState::default()
	}

	#[test]
	fn no_push_when_nothing_read_or_no_advance() {
		let chapters = vec![chapter(Some(10.0), None, false)];
		assert_eq!(decide(TrackMode::Chapter, 0, &empty_remote(), &chapters, day()), None);

		let read = vec![chapter(Some(10.0), None, true)];
		let ahead = RemoteState { last_read_chapter: Some(10.0), ..empty_remote() };
		assert_eq!(decide(TrackMode::Chapter, 0, &ahead, &read, day()), None);
	}

	#[test]
	fn planning_entry_starts_reading_with_start_date() {
		let remote = RemoteState { status: Some(TrackStatus::Planning), total_chapters: Some(20), ..empty_remote() };
		let chapters = vec![chapter(Some(10.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_chapter, Some(10.0));
		assert_eq!(update.status, Some(TrackStatus::Reading));
		assert_eq!(update.start_read_date.as_deref(), Some("2026-10-26"));
		assert_eq!(update.finish_read_date, None);
	}

	#[test]
	fn finishing_last_marks_completed_immediately_with_finish_date() {
		// comparing the pre-push value would lag completion by one event; the new
		// value lands completion on this very event.
		let remote = RemoteState {
			status: Some(TrackStatus::Reading),
			last_read_chapter: Some(19.0),
			start_read_date: Some("2026-10-01".into()),
			total_chapters: Some(20),
			..empty_remote()
		};
		let chapters = vec![chapter(Some(20.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_chapter, Some(20.0));
		assert_eq!(update.status, Some(TrackStatus::Completed));
		assert_eq!(update.finish_read_date.as_deref(), Some("2026-10-26"));
		// existing start date is kept (never overwritten)
		assert_eq!(update.start_read_date, None);
	}

	#[test]
	fn completed_entry_reopened_becomes_rereading() {
		let remote = RemoteState {
			status: Some(TrackStatus::Completed),
			last_read_chapter: Some(20.0),
			total_chapters: Some(30),
			start_read_date: Some("2026-09-01".into()),
			finish_read_date: Some("2026-09-30".into()),
			..empty_remote()
		};
		let chapters = vec![chapter(Some(21.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.status, Some(TrackStatus::Rereading));
		// dates of the original run are left untouched
		assert_eq!(update.start_read_date, None);
		assert_eq!(update.finish_read_date, None);
	}

	#[test]
	fn unknown_totals_never_auto_complete() {
		let remote = RemoteState { status: Some(TrackStatus::Reading), last_read_chapter: Some(99.0), ..empty_remote() };
		let chapters = vec![chapter(Some(100.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.status, None);
		assert_eq!(update.finish_read_date, None);
	}

	#[test]
	fn chapter_progress_can_advance_past_stale_remote_total() {
		let remote =
			RemoteState { status: Some(TrackStatus::Reading), last_read_chapter: Some(76.0), total_chapters: Some(76), ..empty_remote() };
		let chapters = vec![chapter(Some(77.0), None, true)];
		let update = decide(TrackMode::Chapter, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_chapter, Some(77.0));
		assert_eq!(update.status, None);
	}

	#[test]
	fn chapter_offset_applies_and_clamps_to_total() {
		let remote = RemoteState { status: Some(TrackStatus::Reading), total_chapters: Some(100), ..empty_remote() };
		let chapters = vec![chapter(Some(10.0), None, true)];
		let update = decide(TrackMode::Chapter, 5, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_chapter, Some(15.0));

		// overshoot clamps to the tracker total instead of exceeding it
		let update = decide(TrackMode::Chapter, 500, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_chapter, Some(100.0));
		assert_eq!(update.status, Some(TrackStatus::Completed));

		// a negative offset floors at 0 and may then not advance the remote
		let remote = RemoteState { status: Some(TrackStatus::Reading), ..empty_remote() };
		let chapters = vec![chapter(Some(3.0), None, true)];
		assert_eq!(decide(TrackMode::Chapter, -5, &remote, &chapters, day()), None);
	}

	#[test]
	fn volume_mode_ignores_offset_and_completes_against_total_volumes() {
		let remote = RemoteState { status: Some(TrackStatus::Reading), total_volumes: Some(3), ..empty_remote() };
		// chapter-numbered series falls back to floor(chapter) as volume
		let chapters = vec![chapter(Some(3.0), None, true)];
		let update = decide(TrackMode::Volume, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_volume, Some(3));
		assert_eq!(update.last_read_chapter, None);
		assert_eq!(update.status, Some(TrackStatus::Completed));

		// volume values never receive the chapter offset
		let chapters = vec![chapter(None, Some(2.0), true)];
		let update = decide(TrackMode::Volume, 100, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_volume, Some(2));
	}

	#[test]
	fn volume_mode_waits_for_a_volume_signal_in_volume_numbered_series() {
		let remote = RemoteState { status: Some(TrackStatus::Reading), total_volumes: Some(5), ..empty_remote() };
		let chapters = vec![chapter(None, Some(3.0), true)];
		let update = decide(TrackMode::Volume, 0, &remote, &chapters, day()).unwrap();
		assert_eq!(update.last_read_volume, Some(3));
		assert_eq!(update.status, None); // 3 != 5, still reading
	}
}
