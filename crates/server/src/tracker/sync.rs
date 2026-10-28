//! Tracker sync: pushes reading progress to the user's tracker accounts when
//! chapters are marked read. Every decision rule lives in `core` (pure,
//! unit-tested); this module only wires the read-progress tap, database reads
//! and provider calls together.
//!
//! Progress only ever moves forward: unread/delete events never regress the
//! tracker entry. Failures are logged and never propagated — reading must
//! never fail because a tracker is down.

use super::core::{self, ChapterTrackInput};
use super::{Provider, TrackerServices, dao};
use sqlx::SqlitePool;
use std::sync::Arc;

/// A read path marked chapters read: re-evaluate the manga's bindings.
/// Coalesced per manga, so the payload is just the pair plus an optional
/// provider to skip: a pull sync marks chapters from tracker A and the
/// resulting push must not echo back to A (Aidoku's skipTracker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerSyncEvent {
	Progress { manga_id: i64, user_id: i64, skip: Option<Provider> },
}

pub type TrackerSyncSender = tokio::sync::mpsc::UnboundedSender<TrackerSyncEvent>;

/// Serializes concurrent syncs of the same (manga, user) pair. Pushing is a
/// GET → decide → POST sequence against the tracker, and overlapping sequences
/// reorder: two chapters completing in quick succession would both read the
/// same remote progress and the earlier chapter could POST last, regressing
/// the remote value. Holding one lock per key for the whole sync keeps every
/// sequence atomic; distinct pairs still run concurrently.
type SyncKey = (i64, i64);

fn sync_locks() -> &'static std::sync::Mutex<std::collections::HashMap<SyncKey, Arc<tokio::sync::Mutex<()>>>> {
	static LOCKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<SyncKey, Arc<tokio::sync::Mutex<()>>>>> =
		std::sync::OnceLock::new();
	LOCKS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

async fn run_serialized<R>(key: &SyncKey, fut: impl std::future::Future<Output = R>) -> R {
	let slot = {
		let mut locks = sync_locks().lock().unwrap_or_else(|e| e.into_inner());
		locks.entry(*key).or_default().clone()
	};
	let _guard = slot.lock().await;
	let result = fut.await;
	drop(_guard);
	// drop the slot when nobody else holds it so the registry can't grow with
	// one entry per synced manga
	let mut locks = sync_locks().lock().unwrap_or_else(|e| e.into_inner());
	if Arc::strong_count(&slot) == 2 {
		locks.remove(key);
	}
	result
}

/// Feeds on the lossless tap of the read paths and syncs manga with tracker
/// bindings when their read progress changes. A drained burst is coalesced to
/// one sync per (manga, user): a sync re-reads the current progress from the
/// database anyway, so intermediate events of the same pair would only repeat
/// the same tracker round trip.
pub fn consume_events(
	pool: SqlitePool,
	services: TrackerServices,
	mut events: tokio::sync::mpsc::UnboundedReceiver<TrackerSyncEvent>,
) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		loop {
			// all senders are gone (shutdown)
			let Some(event) = events.recv().await else {
				break;
			};
			// `None` means "push to every bound tracker"; merging two events
			// keeps a skip only when both agree on it, since each event's
			// intent is "push everywhere but here"
			let mut pairs = std::collections::BTreeMap::new();
			let TrackerSyncEvent::Progress { manga_id, user_id, skip } = event;
			pairs.insert((manga_id, user_id), skip);
			while let Ok(TrackerSyncEvent::Progress { manga_id, user_id, skip }) = events.try_recv() {
				pairs
					.entry((manga_id, user_id))
					.and_modify(|existing| {
						if *existing != skip {
							*existing = None;
						}
					})
					.or_insert(skip);
			}
			for ((manga_id, user_id), skip) in pairs {
				let pool = pool.clone();
				let services = services.clone();
				tokio::spawn(async move {
					run_serialized(&(manga_id, user_id), sync_manga(&pool, &services, manga_id, user_id, skip)).await;
				});
			}
		}
	})
}

/// Chapters with neither number nor volume contribute nothing (see core), so
/// no sort-order fallback is needed here either.
async fn gather(pool: &SqlitePool, manga_id: i64, user_id: i64) -> Result<Option<(Vec<dao::TrackerLink>, Vec<ChapterTrackInput>)>, sqlx::Error> {
	let links = dao::list_links_by_manga_and_user(pool, manga_id, user_id).await?;
	if links.is_empty() {
		return Ok(None);
	}
	let rows: Vec<(Option<f64>, Option<f64>, bool)> = sqlx::query_as(
		"SELECT c.number, c.volume, COALESCE(cu.is_read, 0) FROM chapter c
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = ? AND c.trashed_at IS NULL",
	)
	.bind(user_id)
	.bind(manga_id)
	.fetch_all(pool)
	.await?;
	if rows.is_empty() {
		return Ok(None);
	}
	let chapters = rows
		.into_iter()
		.map(|(number, volume, read)| ChapterTrackInput { number, volume, read })
		.collect();
	Ok(Some((links, chapters)))
}

/// Syncs every tracker binding of one (manga, user) pair, except the provider
/// a pull sync just read from: pushing its own progress back could advance the
/// remote entry (floored local numbers round up past the tracker value).
async fn sync_manga(pool: &SqlitePool, services: &TrackerServices, manga_id: i64, user_id: i64, skip: Option<Provider>) {
	let (links, chapters) = match gather(pool, manga_id, user_id).await {
		Ok(Some(snapshot)) => snapshot,
		Ok(None) => return,
		Err(e) => {
			tracing::warn!("tracker sync: failed to gather manga {manga_id}: {e}");
			return;
		}
	};
	let today = chrono::Local::now().date_naive();
	for link in &links {
		if skip.is_some_and(|provider| provider.as_str() == link.provider) {
			continue;
		}
		if let Err(e) = push_link(services, link, &chapters, today).await {
			tracing::warn!("tracker sync: {}:{} for user {} failed: {e}", link.provider, link.track_id, link.user_id);
		}
	}
}

/// Fetches the tracker state, decides, and pushes when there is something to
/// push. "Nothing to push" is `Ok(())`.
async fn push_link(
	services: &TrackerServices,
	link: &dao::TrackerLink,
	chapters: &[ChapterTrackInput],
	today: chrono::NaiveDate,
) -> Result<(), super::TrackerError> {
	let Some(provider) = Provider::parse(&link.provider) else {
		// the CHECK constraint makes this unreachable unless the row was
		// written by something bypassing it; skipping beats pushing blind
		tracing::warn!("tracker sync: unsupported provider {} on manga {}", link.provider, link.manga_id);
		return Ok(());
	};
	let service = services.get(provider).ok_or_else(|| super::TrackerError::Provider {
		provider,
		message: "tracker provider is unavailable".into(),
	})?;
	let remote = service.get_state(link.user_id, &link.track_id).await?;
	let mode = core::effective_mode(link.track_mode, chapters);
	let Some(update) = core::decide(mode, link.chapter_offset, &remote, chapters, today) else {
		return Ok(());
	};
	service.update(link.user_id, &link.track_id, &update).await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tracker::core::{RemoteState, TrackMode, TrackSearchItem, TrackStatus, TrackUpdate};
	use crate::tracker::{TrackerError, TrackerService};
	use std::sync::Mutex as StdMutex;

	/// Two pushes racing on the same (manga, user) must stay ordered: each
	/// task's GET happens strictly before its POST, otherwise the earlier
	/// chapter could POST last and regress the remote value. The sleep stands
	/// in for the provider round trip and forces the interleaving.
	#[tokio::test]
	async fn same_key_sequences_never_interleave() {
		let events: Arc<StdMutex<Vec<(&'static str, &'static str)>>> = Arc::new(StdMutex::new(Vec::new()));
		let key = (1, 1);
		let run = |tag: &'static str| {
			let events = events.clone();
			async move {
				run_serialized(&key, async move {
					events.lock().unwrap().push(("get", tag));
					tokio::time::sleep(std::time::Duration::from_millis(20)).await;
					events.lock().unwrap().push(("post", tag));
				})
				.await
			}
		};
		tokio::join!(run("one"), run("two"));
		let order: Vec<_> = events.lock().unwrap().iter().map(|(op, _)| *op).collect();
		assert_eq!(order, vec!["get", "post", "get", "post"]);
	}

	/// Distinct (manga, user) pairs must not block each other. The barrier
	/// forces both GETs before either POST; a global lock would deadlock them
	/// into get,post,get,post instead.
	#[tokio::test]
	async fn distinct_keys_run_concurrently() {
		let barrier = Arc::new(tokio::sync::Barrier::new(2));
		let events: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));
		let run = |key: SyncKey, barrier: Arc<tokio::sync::Barrier>| {
			let events = events.clone();
			async move {
				run_serialized(&key, async move {
					events.lock().unwrap().push("get");
					barrier.wait().await;
					tokio::time::sleep(std::time::Duration::from_millis(10)).await;
					events.lock().unwrap().push("post");
				})
				.await
			}
		};
		tokio::join!(run((1, 1), barrier.clone()), run((2, 1), barrier.clone()));
		assert_eq!(*events.lock().unwrap(), vec!["get", "get", "post", "post"]);
	}

	/// Finished slots are evicted so the registry can't grow with one entry
	/// per synced manga.
	#[tokio::test]
	async fn finished_slots_are_evicted() {
		let key = (42, 1);
		run_serialized(&key, async {}).await;
		let locks = sync_locks().lock().unwrap_or_else(|e| e.into_inner());
		assert!(!locks.contains_key(&key));
	}

	type UpdateLog = Arc<StdMutex<Vec<(i64, String, TrackUpdate)>>>;

	struct StubTracker {
		provider: Provider,
		remote: RemoteState,
		updates: UpdateLog,
	}

	#[async_trait::async_trait]
	impl TrackerService for StubTracker {
		fn provider(&self) -> Provider {
			self.provider
		}

		async fn is_logged_in(&self, _user_id: i64) -> Result<bool, TrackerError> {
			Ok(true)
		}

		async fn search(&self, _user_id: i64, _query: &str, _nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
			Ok(vec![])
		}

		async fn get_state(&self, _user_id: i64, _track_id: &str) -> Result<RemoteState, TrackerError> {
			Ok(self.remote.clone())
		}

		async fn update(&self, user_id: i64, track_id: &str, update: &TrackUpdate) -> Result<(), TrackerError> {
			self.updates.lock().unwrap().push((user_id, track_id.to_string(), update.clone()));
			Ok(())
		}
	}

	#[tokio::test]
	async fn push_link_uses_provider_and_respects_mode() {
		let updates = Arc::new(StdMutex::new(Vec::new()));
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let services = TrackerServices::new(
			crate::tracker::oauth::OAuthManager::new(pool, reqwest::Client::new()),
			vec![Arc::new(StubTracker {
				provider: Provider::Anilist,
				remote: RemoteState {
					status: Some(TrackStatus::Reading),
					last_read_chapter: Some(10.0),
					total_chapters: Some(12),
					..RemoteState::default()
				},
				updates: updates.clone(),
			})],
		);
		let link = dao::TrackerLink {
			manga_id: 1,
			user_id: 7,
			provider: "anilist".into(),
			track_id: "42".into(),
			title: None,
			track_mode: TrackMode::Chapter,
			chapter_offset: 0,
			created_at: 0,
			updated_at: 0,
		};
		let chapters = [ChapterTrackInput { number: Some(11.0), volume: None, read: true }];
		push_link(&services, &link, &chapters, chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()).await.unwrap();

		let updates = updates.lock().unwrap();
		assert_eq!(updates.len(), 1);
		assert_eq!(updates[0].0, 7);
		assert_eq!(updates[0].1, "42");
		assert_eq!(updates[0].2.last_read_chapter, Some(11.0));
		assert_eq!(updates[0].2.status, None);
	}

	/// A pull sync marks chapters read from one tracker and then pushes the new
	/// progress to the manga's other bindings: the source provider is skipped
	/// (Aidoku's skipTracker), or the echo would advance the remote entry past
	/// the value it just reported (floored local numbers round up).
	#[tokio::test]
	async fn sync_manga_skip_excludes_the_source_tracker() {
		let updates = Arc::new(StdMutex::new(Vec::new()));
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let make = |provider: Provider, updates: &UpdateLog| {
			Arc::new(StubTracker {
				provider,
				remote: RemoteState { status: Some(TrackStatus::Reading), last_read_chapter: Some(10.0), ..RemoteState::default() },
				updates: updates.clone(),
			})
		};
		let services = TrackerServices::new(
			crate::tracker::oauth::OAuthManager::new(pool.clone(), reqwest::Client::new()),
			vec![make(Provider::Anilist, &updates), make(Provider::Mal, &updates)],
		);
		let source_id: i64 = sqlx::query_scalar("INSERT INTO source (key, name, wasm_path) VALUES ('en.test', 'Test', '') RETURNING id")
			.fetch_one(&pool)
			.await
			.unwrap();
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'm', 'M') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.unwrap();
		let chapter_id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key, number) VALUES (?, 'ch11', 11) RETURNING id")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.unwrap();
		sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, is_read) VALUES (?, 1, 1)")
			.bind(chapter_id)
			.execute(&pool)
			.await
			.unwrap();
		for provider in ["anilist", "mal"] {
			dao::upsert_link(
				&pool,
				&dao::NewTrackerLink {
					manga_id,
					user_id: 1,
					provider,
					track_id: "42",
					title: None,
					track_mode: TrackMode::Auto,
					chapter_offset: 0,
				},
			)
			.await
			.unwrap();
		}

		sync_manga(&pool, &services, manga_id, 1, Some(Provider::Anilist)).await;

		{
			let pushed = updates.lock().unwrap();
			// only the mal binding is pushed; the anilist stub still exists in
			// the registry, so a missing skip would produce two updates
			assert_eq!(pushed.len(), 1);
			// both trackers read progress 10 and local is at 11, so the
			// surviving push carries the new chapter
			assert_eq!(pushed[0].2.last_read_chapter, Some(11.0));
		}

		// without a skip both bindings are pushed (the first push advanced
		// neither remote past 11, so both decide again)
		sync_manga(&pool, &services, manga_id, 1, None).await;
		let pushed = updates.lock().unwrap();
		assert_eq!(pushed.len(), 3);
	}
}
