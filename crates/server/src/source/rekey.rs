//! Breaking-change key migration: when a source update crosses the manifest's `config.breakingChangeVersion`,
//! a self-migrating source rekeys in place via `handle_key_migration` key by key (row ids untouched,
//! progress/categories/download flags all preserved); an incapable source's rows are not touched, only
//! `migrationRequired` is marked so the UI can guide the user through same-source flow A (mirroring Aidoku's
//! AppDelegate.handleSourceMigration and SourceManager's installedVersion < breakingChangeVersion check)

use crate::error::ApiResult;
use crate::state::AppState;
use aidoku_wasm::SourceHandle;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct RekeyReport {
	/// Crossed a breaking change and the source has no self-migration capability: the user needs flow A same-source migration
	pub required: bool,
	pub warnings: Vec<String>,
}

/// Called after install_from_repo succeeds; old_version is the version installed before the update.
/// Migration guidance is judged by the triggering user's library: entries only in someone else's library are not offered flow A to the current user
pub async fn migrate_after_update(
	state: &Arc<AppState>,
	source_id: i64,
	old_version: i64,
	user_id: i64,
) -> ApiResult<RekeyReport> {
	let row: Option<(i64, i64)> =
		sqlx::query_as("SELECT version, breaking_change_version FROM source WHERE id = ?")
			.bind(source_id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((new_version, breaking)) = row else {
		return Ok(RekeyReport::default());
	};
	// Aidoku only checks old < breaking; breaking <= new_version defends against manifests with scrambled versions
	if !(old_version < breaking && breaking <= new_version) {
		return Ok(RekeyReport::default());
	}
	let (manga_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_one(&state.pool)
		.await?;
	if manga_count == 0 {
		return Ok(RekeyReport::default());
	}
	let handle = state.get_handle(source_id).await?;
	let features = handle.features();
	if !features.migration {
		// Mirroring Aidoku forceMigrate: no guidance when the library has no entries (AppDelegate
		// shows no migration page when sourceManga.isEmpty)
		let (in_library,): (i64,) = sqlx::query_as(
			"SELECT COUNT(*) FROM manga m
			 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ? AND mu.in_library = 1
			 WHERE m.source_id = ?",
		)
		.bind(user_id)
		.bind(source_id)
		.fetch_one(&state.pool)
		.await?;
		return Ok(RekeyReport {
			required: in_library > 0,
			warnings: Vec::new(),
		});
	}
	rekey_in_place(state, source_id, &handle, features.notification).await
}

/// Collects every key mapping first (wasm calls may hit the network and must stay out of the transaction), then
/// rewrites in a single transaction; chapter migration passes the old manga key, consistent with Aidoku
async fn rekey_in_place(
	state: &Arc<AppState>,
	source_id: i64,
	handle: &SourceHandle,
	notification: bool,
) -> ApiResult<RekeyReport> {
	let manga_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_all(&state.pool)
		.await?;
	let chapter_rows: Vec<(i64, String, String)> = sqlx::query_as(
		"SELECT c.id, c.key, m.key FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.source_id = ?",
	)
	.bind(source_id)
	.fetch_all(&state.pool)
	.await?;

	if notification
		&& let Err(e) = handle.handle_notification("system.startMigration".into()).await
	{
		tracing::warn!(source_id, "startMigration notification failed: {e}");
	}

	let mut warnings = Vec::new();
	let mut manga_map: Vec<(i64, String, String)> = Vec::new();
	for (id, key) in &manga_rows {
		match handle.handle_key_migration(0, key.clone(), None).await {
			Ok(new_key) if new_key.is_empty() => {
				warnings.push(format!("manga {key}: source returned an empty key, left unchanged"));
			}
			Ok(new_key) if new_key != *key => manga_map.push((*id, key.clone(), new_key)),
			Ok(_) => {}
			Err(e) => warnings.push(format!("manga {key}: migration call failed: {e}")),
		}
	}
	let mut chapter_map: Vec<(i64, String, String)> = Vec::new();
	for (id, key, manga_key) in &chapter_rows {
		match handle.handle_key_migration(1, manga_key.clone(), Some(key.clone())).await {
			Ok(new_key) if new_key.is_empty() => {
				warnings.push(format!("chapter {key}: source returned an empty key, left unchanged"));
			}
			Ok(new_key) if new_key != *key => chapter_map.push((*id, key.clone(), new_key)),
			Ok(_) => {}
			Err(e) => warnings.push(format!("chapter {key}: migration call failed: {e}")),
		}
	}

	if notification
		&& let Err(e) = handle.handle_notification("system.endMigration".into()).await
	{
		tracing::warn!(source_id, "endMigration notification failed: {e}");
	}

	let mut tx = crate::db::begin_write(&state.pool).await?;
	apply_rekeys(&mut tx, "manga", manga_map, &mut warnings).await?;
	apply_rekeys(&mut tx, "chapter", chapter_map, &mut warnings).await?;
	tx.commit().await?;
	Ok(RekeyReport {
		required: false,
		warnings,
	})
}

/// Rows hitting UNIQUE are deferred and retried: the occupying row may itself be getting rekeyed, and after the
/// fixpoint converges, still-blocked ones keep their old key and go to warnings. A single SQLite statement conflict
/// rolls back only that statement; the transaction stays valid
async fn apply_rekeys(
	tx: &mut sqlx::SqliteConnection,
	table: &str,
	pending: Vec<(i64, String, String)>,
	warnings: &mut Vec<String>,
) -> ApiResult<()> {
	let sql = format!("UPDATE {table} SET key = ? WHERE id = ?");
	let mut pending = pending;
	loop {
		let mut progressed = false;
		let mut blocked = Vec::new();
		for (id, old_key, new_key) in pending {
			match sqlx::query(&sql).bind(&new_key).bind(id).execute(&mut *tx).await {
				Ok(_) => progressed = true,
				Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
					blocked.push((id, old_key, new_key));
				}
				Err(e) => return Err(e.into()),
			}
		}
		if blocked.is_empty() {
			return Ok(());
		}
		if !progressed {
			for (_, old_key, new_key) in blocked {
				warnings.push(format!(
					"{table} {old_key}: target key {new_key} already exists, left unchanged"
				));
			}
			return Ok(());
		}
		pending = blocked;
	}
}
