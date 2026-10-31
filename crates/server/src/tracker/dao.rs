//! SQLite rows for the tracker tables (migration 0027). All timestamps are
//! unix seconds.

use super::Provider;
use super::core::TrackLedgerItem;
use sqlx::{Sqlite, SqlitePool, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerOAuthToken {
	pub access_token: String,
	pub refresh_token: Option<String>,
	pub token_type: Option<String>,
	pub expires_at: Option<i64>,
	pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerLink {
	pub manga_id: i64,
	pub user_id: i64,
	pub provider: String,
	pub track_id: String,
	pub title: Option<String>,
	pub track_mode: super::core::TrackMode,
	pub chapter_offset: i32,
	pub created_at: i64,
	pub updated_at: i64,
}

pub struct NewTrackerLink<'a> {
	pub manga_id: i64,
	pub user_id: i64,
	pub provider: &'a str,
	pub track_id: &'a str,
	pub title: Option<&'a str>,
	pub track_mode: super::core::TrackMode,
	pub chapter_offset: i32,
}

fn now() -> i64 {
	std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

pub async fn upsert_token(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
	token: &TrackerOAuthToken,
) -> Result<(), sqlx::Error> {
	let now = now();
	sqlx::query(
		"INSERT INTO tracker_oauth_token (provider, user_id, access_token, refresh_token, token_type, expires_at,
		 username, created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (provider, user_id) DO UPDATE SET
		 access_token = excluded.access_token, refresh_token = excluded.refresh_token,
		 token_type = excluded.token_type, expires_at = excluded.expires_at,
		 username = excluded.username, updated_at = excluded.updated_at",
	)
	.bind(provider.as_str())
	.bind(user_id)
	.bind(&token.access_token)
	.bind(&token.refresh_token)
	.bind(&token.token_type)
	.bind(token.expires_at)
	.bind(&token.username)
	.bind(now)
	.bind(now)
	.execute(pool)
	.await?;
	Ok(())
}

pub async fn get_token(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
) -> Result<Option<TrackerOAuthToken>, sqlx::Error> {
	sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<i64>, Option<String>)>(
		"SELECT access_token, refresh_token, token_type, expires_at, username
		 FROM tracker_oauth_token WHERE provider = ? AND user_id = ?",
	)
	.bind(provider.as_str())
	.bind(user_id)
	.fetch_optional(pool)
	.await
	.map(|row| {
		row.map(|(access_token, refresh_token, token_type, expires_at, username)| TrackerOAuthToken {
			access_token,
			refresh_token,
			token_type,
			expires_at,
			username,
		})
	})
}

/// Username backfill must not clobber a concurrent refresh: only the exact row
/// still holding the old token is updated.
pub async fn set_username_if_token_matches(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
	access_token: &str,
	username: &str,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		"UPDATE tracker_oauth_token SET username = ?, updated_at = ?
		 WHERE provider = ? AND user_id = ? AND access_token = ?",
	)
	.bind(username)
	.bind(now())
	.bind(provider.as_str())
	.bind(user_id)
	.bind(access_token)
	.execute(pool)
	.await?;
	Ok(())
}

pub async fn delete_token(pool: &SqlitePool, provider: Provider, user_id: i64) -> Result<(), sqlx::Error> {
	sqlx::query("DELETE FROM tracker_oauth_token WHERE provider = ? AND user_id = ?")
		.bind(provider.as_str())
		.bind(user_id)
		.execute(pool)
		.await?;
	Ok(())
}

/// A logout racing an in-flight callback must not delete the row the callback
/// just created: only the pre-logout token is removed.
pub async fn delete_token_if_matches(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
	access_token: &str,
) -> Result<bool, sqlx::Error> {
	let result = sqlx::query("DELETE FROM tracker_oauth_token WHERE provider = ? AND user_id = ? AND access_token = ?")
		.bind(provider.as_str())
		.bind(user_id)
		.bind(access_token)
		.execute(pool)
		.await?;
	Ok(result.rows_affected() > 0)
}

pub async fn insert_pending(
	pool: &SqlitePool,
	provider: Provider,
	nonce: &str,
	user_id: i64,
	verifier: &str,
) -> Result<(), sqlx::Error> {
	sqlx::query("INSERT INTO tracker_oauth_pending (provider, nonce, user_id, verifier, created_at) VALUES (?, ?, ?, ?, ?)")
		.bind(provider.as_str())
		.bind(nonce)
		.bind(user_id)
		.bind(verifier)
		.bind(now())
		.execute(pool)
		.await?;
	Ok(())
}

/// Handshakes older than ten minutes are abandoned sign-ins, not valid state.
const PENDING_TTL_SECS: i64 = 600;

pub async fn get_pending(
	pool: &SqlitePool,
	provider: Provider,
	nonce: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
	sqlx::query_as::<_, (i64, String)>(
		"SELECT user_id, verifier FROM tracker_oauth_pending
		 WHERE provider = ? AND nonce = ? AND created_at > ?",
	)
	.bind(provider.as_str())
	.bind(nonce)
	.bind(now() - PENDING_TTL_SECS)
	.fetch_optional(pool)
	.await
}

pub async fn delete_pending(pool: &SqlitePool, provider: Provider, nonce: &str) -> Result<(), sqlx::Error> {
	sqlx::query("DELETE FROM tracker_oauth_pending WHERE provider = ? AND nonce = ?")
		.bind(provider.as_str())
		.bind(nonce)
		.execute(pool)
		.await?;
	Ok(())
}

/// One-shot consume for the callback: a nonce replay finds nothing.
pub async fn consume_pending(
	pool: &SqlitePool,
	provider: Provider,
	nonce: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
	sqlx::query_as::<_, (i64, String)>(
		"DELETE FROM tracker_oauth_pending WHERE provider = ? AND nonce = ? AND created_at > ?
		 RETURNING user_id, verifier",
	)
	.bind(provider.as_str())
	.bind(nonce)
	.bind(now() - PENDING_TTL_SECS)
	.fetch_optional(pool)
	.await
}

pub async fn delete_pending_for_user(pool: &SqlitePool, provider: Provider, user_id: i64) -> Result<(), sqlx::Error> {
	sqlx::query("DELETE FROM tracker_oauth_pending WHERE provider = ? AND user_id = ?")
		.bind(provider.as_str())
		.bind(user_id)
		.execute(pool)
		.await?;
	Ok(())
}

/// A refresh racing a logout/login must not resurrect the old row: the fresh
/// token is only written while the row still holds the token the refresh saw.
pub async fn update_token_if_matches(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
	expected_access_token: &str,
	fresh: &TrackerOAuthToken,
) -> Result<bool, sqlx::Error> {
	let result = sqlx::query(
		"UPDATE tracker_oauth_token SET
		 access_token = ?, refresh_token = ?, token_type = ?, expires_at = ?, username = ?, updated_at = ?
		 WHERE provider = ? AND user_id = ? AND access_token = ?",
	)
	.bind(&fresh.access_token)
	.bind(&fresh.refresh_token)
	.bind(&fresh.token_type)
	.bind(fresh.expires_at)
	.bind(&fresh.username)
	.bind(now())
	.bind(provider.as_str())
	.bind(user_id)
	.bind(expected_access_token)
	.execute(pool)
	.await?;
	Ok(result.rows_affected() > 0)
}

pub async fn upsert_link(pool: &SqlitePool, link: &NewTrackerLink<'_>) -> Result<(), sqlx::Error> {
	let now = now();
	sqlx::query(
		"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, title, track_mode, chapter_offset,
		 created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, user_id, provider) DO UPDATE SET
		 track_id = excluded.track_id, title = excluded.title, track_mode = excluded.track_mode,
		 chapter_offset = excluded.chapter_offset, updated_at = excluded.updated_at",
	)
	.bind(link.manga_id)
	.bind(link.user_id)
	.bind(link.provider)
	.bind(link.track_id)
	.bind(link.title)
	.bind(link.track_mode.as_str())
	.bind(link.chapter_offset)
	.bind(now)
	.bind(now)
	.execute(pool)
	.await?;
	Ok(())
}

/// Same binding upsert as [`upsert_link`], for a caller that already holds a transaction (metadata apply binds
/// and writes metadata atomically).
pub async fn upsert_link_tx(
	tx: &mut Transaction<'_, Sqlite>,
	link: &NewTrackerLink<'_>,
) -> Result<(), sqlx::Error> {
	let now = now();
	sqlx::query(
		"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, title, track_mode, chapter_offset,
		 created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, user_id, provider) DO UPDATE SET
		 track_id = excluded.track_id, title = excluded.title, track_mode = excluded.track_mode,
		 chapter_offset = excluded.chapter_offset, updated_at = excluded.updated_at",
	)
	.bind(link.manga_id)
	.bind(link.user_id)
	.bind(link.provider)
	.bind(link.track_id)
	.bind(link.title)
	.bind(link.track_mode.as_str())
	.bind(link.chapter_offset)
	.bind(now)
	.bind(now)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

/// Additive merge used by the external-app imports: a binding that already
/// exists for (manga, user, provider) wins, so importing a backup never
/// rewrites a link the user made here; returns whether a row was inserted, so
/// a repeated import of the same file writes nothing at all.
pub async fn insert_link_if_absent(
	tx: &mut Transaction<'_, Sqlite>,
	link: &NewTrackerLink<'_>,
) -> Result<bool, sqlx::Error> {
	let now = now();
	let result = sqlx::query(
		"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, title, track_mode, chapter_offset,
		 created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, user_id, provider) DO NOTHING",
	)
	.bind(link.manga_id)
	.bind(link.user_id)
	.bind(link.provider)
	.bind(link.track_id)
	.bind(link.title)
	.bind(link.track_mode.as_str())
	.bind(link.chapter_offset)
	.bind(now)
	.bind(now)
	.execute(&mut **tx)
	.await?;
	Ok(result.rows_affected() == 1)
}

fn row_to_link(row: (i64, i64, String, String, Option<String>, String, i32, i64, i64)) -> TrackerLink {
	let (manga_id, user_id, provider, track_id, title, track_mode, chapter_offset, created_at, updated_at) = row;
	TrackerLink {
		manga_id,
		user_id,
		provider,
		track_id,
		title,
		track_mode: super::core::TrackMode::parse(&track_mode).unwrap_or_default(),
		chapter_offset,
		created_at,
		updated_at,
	}
}

const LINK_COLUMNS: &str = "manga_id, user_id, provider, track_id, title, track_mode, chapter_offset, created_at, updated_at";

pub async fn list_links_by_manga_and_user(
	pool: &SqlitePool,
	manga_id: i64,
	user_id: i64,
) -> Result<Vec<TrackerLink>, sqlx::Error> {
	sqlx::query_as::<_, (i64, i64, String, String, Option<String>, String, i32, i64, i64)>(&format!(
		"SELECT {LINK_COLUMNS} FROM tracker_link WHERE manga_id = ? AND user_id = ? ORDER BY provider"
	))
	.bind(manga_id)
	.bind(user_id)
	.fetch_all(pool)
	.await
	.map(|rows| rows.into_iter().map(row_to_link).collect())
}

/// A link with the local manga fields the bindings list displays, so the list needs no per-row manga request
#[derive(Debug, Clone)]
pub struct TrackerBinding {
	pub link: TrackerLink,
	pub manga_title: Option<String>,
	pub manga_cover: Option<String>,
}

/// Pages by manga rather than by link, so a manga's providers never split across two pages
pub async fn list_bindings_page(
	pool: &SqlitePool,
	user_id: i64,
	provider: Option<Provider>,
	limit: i64,
	offset: i64,
) -> Result<Vec<TrackerBinding>, sqlx::Error> {
	type Row = (i64, i64, String, String, Option<String>, String, i32, i64, i64, Option<String>, Option<String>);
	let provider = provider.map(Provider::as_str);
	let rows = sqlx::query_as::<_, Row>(
		"SELECT tl.manga_id, tl.user_id, tl.provider, tl.track_id, tl.title, tl.track_mode, tl.chapter_offset,
		        tl.created_at, tl.updated_at, m.title, m.cover_url
		 FROM tracker_link tl
		 LEFT JOIN manga m ON m.id = tl.manga_id
		 WHERE tl.user_id = ?1 AND (?2 IS NULL OR tl.provider = ?2)
		   AND tl.manga_id IN (
		     SELECT manga_id FROM tracker_link
		     WHERE user_id = ?1 AND (?2 IS NULL OR provider = ?2)
		     GROUP BY manga_id ORDER BY manga_id LIMIT ?3 OFFSET ?4
		   )
		 ORDER BY tl.manga_id, tl.provider",
	)
	.bind(user_id)
	.bind(provider)
	.bind(limit)
	.bind(offset)
	.fetch_all(pool)
	.await?;
	Ok(rows
		.into_iter()
		.map(|(manga_id, user_id, provider, track_id, title, track_mode, chapter_offset, created_at, updated_at, manga_title, manga_cover)| {
			TrackerBinding {
				link: row_to_link((manga_id, user_id, provider, track_id, title, track_mode, chapter_offset, created_at, updated_at)),
				manga_title,
				manga_cover,
			}
		})
		.collect())
}

/// Bound manga in total and per provider; a manga has at most one link per provider, so rows count manga
pub async fn binding_counts(pool: &SqlitePool, user_id: i64) -> Result<(i64, Vec<(String, i64)>), sqlx::Error> {
	let manga = sqlx::query_scalar("SELECT COUNT(DISTINCT manga_id) FROM tracker_link WHERE user_id = ?").bind(user_id).fetch_one(pool).await?;
	let providers = sqlx::query_as("SELECT provider, COUNT(*) FROM tracker_link WHERE user_id = ? GROUP BY provider ORDER BY provider")
		.bind(user_id)
		.fetch_all(pool)
		.await?;
	Ok((manga, providers))
}

pub async fn delete_link(pool: &SqlitePool, manga_id: i64, user_id: i64, provider: Provider) -> Result<(), sqlx::Error> {
	sqlx::query("DELETE FROM tracker_link WHERE manga_id = ? AND user_id = ? AND provider = ?")
		.bind(manga_id)
		.bind(user_id)
		.bind(provider.as_str())
		.execute(pool)
		.await?;
	Ok(())
}

pub async fn upsert_ledger(
	pool: &SqlitePool,
	provider: Provider,
	user_id: i64,
	track_id: &str,
	title: Option<&str>,
	url: Option<&str>,
	cover_url: Option<&str>,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		"INSERT INTO tracker_ledger (provider, user_id, track_id, title, url, cover_url, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (provider, user_id, track_id) DO UPDATE SET
		 title = COALESCE(excluded.title, tracker_ledger.title),
		 url = COALESCE(excluded.url, tracker_ledger.url),
		 cover_url = COALESCE(excluded.cover_url, tracker_ledger.cover_url),
		 updated_at = excluded.updated_at",
	)
	.bind(provider.as_str())
	.bind(user_id)
	.bind(track_id)
	.bind(title)
	.bind(url)
	.bind(cover_url)
	.bind(now())
	.execute(pool)
	.await?;
	Ok(())
}

pub async fn list_ledger(pool: &SqlitePool, user_id: i64) -> Result<Vec<TrackLedgerItem>, sqlx::Error> {
	sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, i64)>(
		"SELECT provider, track_id, title, url, cover_url, updated_at
		 FROM tracker_ledger WHERE user_id = ? ORDER BY updated_at DESC LIMIT 100",
	)
	.bind(user_id)
	.fetch_all(pool)
	.await
	.map(|rows| {
		rows.into_iter()
			.map(|(provider, track_id, title, url, cover_url, updated_at)| TrackLedgerItem {
				provider,
				track_id,
				title,
				url,
				cover_url,
				updated_at,
			})
			.collect()
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tracker::core::TrackMode;

	async fn pool() -> (tempfile::TempDir, SqlitePool) {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		(tmp, pool)
	}

	async fn seed_user_and_manga(pool: &SqlitePool) -> (i64, i64) {
		let user_id =
			sqlx::query_scalar::<_, i64>("INSERT INTO user (username, password_hash) VALUES ('reader', 'x') RETURNING id")
				.fetch_one(pool)
				.await
				.unwrap();
		let source_id = sqlx::query_scalar::<_, i64>(
			"INSERT INTO source (key, name, wasm_path) VALUES ('en.test', 'Test', '') RETURNING id",
		)
		.fetch_one(pool)
		.await
		.unwrap();
		let manga_id = sqlx::query_scalar::<_, i64>(
			"INSERT INTO manga (source_id, key, title) VALUES (?, 'm1', 'Series') RETURNING id",
		)
		.bind(source_id)
		.fetch_one(pool)
		.await
		.unwrap();
		(user_id, manga_id)
	}

	#[tokio::test]
	async fn token_roundtrip_and_conditional_delete() {
		let (_tmp, pool) = pool().await;
		let (user_id, _) = seed_user_and_manga(&pool).await;
		assert_eq!(get_token(&pool, Provider::Anilist, user_id).await.unwrap(), None);
		let token = TrackerOAuthToken {
			access_token: "a1".into(),
			refresh_token: Some("r1".into()),
			token_type: Some("Bearer".into()),
			expires_at: Some(1_800_000_000),
			username: None,
		};
		upsert_token(&pool, Provider::Anilist, user_id, &token).await.unwrap();
		assert_eq!(get_token(&pool, Provider::Anilist, user_id).await.unwrap(), Some(token.clone()));

		set_username_if_token_matches(&pool, Provider::Anilist, user_id, "other-token", "alice").await.unwrap();
		assert_eq!(get_token(&pool, Provider::Anilist, user_id).await.unwrap().unwrap().username, None);
		set_username_if_token_matches(&pool, Provider::Anilist, user_id, "a1", "alice").await.unwrap();
		assert_eq!(get_token(&pool, Provider::Anilist, user_id).await.unwrap().unwrap().username.as_deref(), Some("alice"));

		delete_token_if_matches(&pool, Provider::Anilist, user_id, "other-token").await.unwrap();
		assert!(get_token(&pool, Provider::Anilist, user_id).await.unwrap().is_some());
		delete_token_if_matches(&pool, Provider::Anilist, user_id, "a1").await.unwrap();
		assert!(get_token(&pool, Provider::Anilist, user_id).await.unwrap().is_none());
	}

	#[tokio::test]
	async fn pending_expires_after_ttl() {
		let (_tmp, pool) = pool().await;
		let (user_id, _) = seed_user_and_manga(&pool).await;
		insert_pending(&pool, Provider::Mal, "n1", user_id, "v1").await.unwrap();
		assert_eq!(get_pending(&pool, Provider::Mal, "n1").await.unwrap(), Some((user_id, "v1".to_string())));
		sqlx::query("UPDATE tracker_oauth_pending SET created_at = created_at - 601")
			.execute(&pool)
			.await
			.unwrap();
		assert_eq!(get_pending(&pool, Provider::Mal, "n1").await.unwrap(), None);
		delete_pending(&pool, Provider::Mal, "n1").await.unwrap();
	}

	#[tokio::test]
	async fn link_upsert_and_scoping() {
		let (_tmp, pool) = pool().await;
		let (user_id, manga_id) = seed_user_and_manga(&pool).await;
		upsert_link(
			&pool,
			&NewTrackerLink {
				manga_id,
				user_id,
				provider: "anilist",
				track_id: "42",
				title: Some("Series"),
				track_mode: TrackMode::Auto,
				chapter_offset: 2,
			},
		)
		.await
		.unwrap();
		upsert_link(
			&pool,
			&NewTrackerLink {
				manga_id,
				user_id,
				provider: "anilist",
				track_id: "43",
				title: None,
				track_mode: TrackMode::Volume,
				chapter_offset: 0,
			},
		)
		.await
		.unwrap();
		let links = list_links_by_manga_and_user(&pool, manga_id, user_id).await.unwrap();
		assert_eq!(links.len(), 1);
		assert_eq!(links[0].track_id, "43");
		assert_eq!(links[0].track_mode, TrackMode::Volume);
		assert_eq!(list_links_by_manga_and_user(&pool, manga_id, user_id + 1).await.unwrap().len(), 0);

		delete_link(&pool, manga_id, user_id, Provider::Anilist).await.unwrap();
		assert!(list_bindings_page(&pool, user_id, None, 50, 0).await.unwrap().is_empty());
	}

	#[tokio::test]
	async fn ledger_keeps_existing_metadata_on_null_upsert() {
		let (_tmp, pool) = pool().await;
		let (user_id, _) = seed_user_and_manga(&pool).await;
		upsert_ledger(&pool, Provider::Mal, user_id, "42", Some("Series"), Some("https://x"), None).await.unwrap();
		upsert_ledger(&pool, Provider::Mal, user_id, "42", None, None, Some("https://cover")).await.unwrap();
		let items = list_ledger(&pool, user_id).await.unwrap();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].title.as_deref(), Some("Series"));
		assert_eq!(items[0].cover_url.as_deref(), Some("https://cover"));
	}
}
