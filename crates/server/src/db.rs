use crate::error::Error;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::str::FromStr;

pub async fn create_pool(database_url: &str) -> Result<SqlitePool, Error> {
	// The parent directory of the file path after sqlite:// must exist first, otherwise rwc cannot create the DB
	let path_part = database_url
		.strip_prefix("sqlite://")
		.unwrap_or(database_url)
		.split('?')
		.next()
		.unwrap_or_default();
	if !path_part.is_empty() && path_part != ":memory:"
		&& let Some(parent) = std::path::Path::new(path_part).parent()
		&& !parent.as_os_str().is_empty()
	{
		std::fs::create_dir_all(parent)?;
	}
	let options = SqliteConnectOptions::from_str(database_url)?
		.create_if_missing(true)
		.journal_mode(SqliteJournalMode::Wal)
		.foreign_keys(true);
	let pool = SqlitePoolOptions::new().connect_with(options).await?;
	sqlx::migrate!("./migrations").run(&pool).await?;
	let rebaseline: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'sync.single_owner_rebaseline'")
		.fetch_optional(&pool)
		.await?;
	if rebaseline.is_some() {
		// Deleting reader origins also removes their latest shared state; rebuild it from the remaining admin rows
		crate::sync::compact::compact(&pool)
			.await
			.map_err(|e| Error::Sync(e.message().to_string()))?;
		sqlx::query("DELETE FROM setting WHERE key = 'sync.single_owner_rebaseline'")
			.execute(&pool)
			.await?;
	}
	Ok(pool)
}

/// Write-transaction entry point: when a deferred transaction reads before writing, upgrading to a write lock
/// against a concurrent commit hits SQLITE_BUSY_SNAPSHOT (the snapshot is stale and busy_timeout does not cover it);
/// IMMEDIATE queues for the write lock at BEGIN, so the transaction's read snapshot is necessarily writable
pub async fn begin_write(
	pool: &SqlitePool,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, sqlx::Error> {
	pool.begin_with("BEGIN IMMEDIATE").await
}

#[cfg(test)]
mod tests {
	/// Applied migrations are immutable: at startup sqlx compares the checksum embedded by
	/// migrate! against what _sqlx_migrations recorded when the migration was applied, and any
	/// byte change — a comment edit included — aborts the boot with VersionMismatch for every
	/// existing deployment. Pin the embedded SHA-384 checksums; adding a NEW migration appends a
	/// pin, touching an old file fails here before it can ship.
	#[test]
	fn shipped_migrations_are_immutable() {
		static PINS: &[(i64, &str)] = &[
			(1, "0A32F7A388114C455681CDB0ED294244A840677D32F58CB18A0AFB5DFA0377288B89A35A7CE7375D4ACBAD57CE472D0B"),
			(2, "F90D878C9B462832EF37AFDD96D0AA7FF7A90CF7905C93CEBDAEA164B8303DAD26B8231C2D47495A973FF0B0CF680226"),
			(3, "80C6884AA4E7DED4D12CD811053401A14D69E367A1198E9627A77398F591E4DC76789978B79D1FDC9EF075AB30867740"),
			(4, "2A2410B566CD5B92D818FD1C144D4CB0B904CD36C0872FEA8F4DBC2105C6DE0FE111894309A5368D03C8DF04AE9CB05F"),
			(5, "2D3B48A60599AF7DC2D1BD3036EA1A423547C34E16E36D27D1E5C33A0954D8ECF260D4336B8A475B19B05B823C50F5EA"),
			(6, "DD02CC7EF46C7FEDF95DA0D295F225CAA78DB2FF0A5F66B5905262030C347B9F167EAC20E3F11D04AACA9BEE39ADA69D"),
			(7, "4E26367A7F96C839BDA030B87BE724476BE9A6F68C3D9B07A86D157A451C0C88E21D46ADBE86D99DF3668E97E5C39293"),
			(8, "016A8DDD216337B4EF752DD381F936B3D6CC8AE0FB1A3BF10E61AB89046EC53D5B945DE5233B8841C43292E628BEAACE"),
			(9, "5919FC62FCD1C0C234CAD0EA2A033DDF869F1EBF7F6FF4D77F18D4E2136BCB4FB22E5F675F94E0A7639320D9EF7CDF30"),
			(10, "0F04A6323ABDD6D7C27FEB05F7603C3E7DFFC47F955E498FD6991EFE9F4B9B5E2993CEE2D39AE4E31046E0FE340DBCCC"),
			(11, "9C5CD7FF3766B4F91B7FDFBBA814B95D0C9773E0461EB2C81D1ACC630CA949E024BD26DF786C22EC0BF9C079ABB3B370"),
			(12, "CB4F4889F693EBC5CF83387C447C1B2B229A65A2C0D739399946295C3AE1F44F4B47A520CC831F7057B2F62936AA4EE3"),
		(13, "AF887712FE6A23301CA3699D25BAB83041C55C15FE2AE1CEE7F8DE8B4EA67BE512884251BD5D8920AD1DF95E61F36370"),
			(14, "DDAEC2A8EF7367BF09182F1458A33773A9C1124595E71D3DDB8A9867A5C9D14B6D4FCBE176432E802DEF0218AAB99CF3"),
			(15, "D9FEC20F2FABD842778568556A897C88B343E29055DAD28D5787ECA10C698DB4BDB2F3448C79A4D7610AD8EC56B89A83"),
			(16, "979AD5AC41A4DB74145E953C5DEEEEC8A1A2E58B5DD04176DC09A649C155BD1B91393C48AD38823593203B9018DFD2C4"),
			(17, "25C04C3F4F4643EDAF682FAB6AFB7FCB13D9642E6B75FFF00F8230A7F2D71DD0C5D65CFFBD3E2B4914ADB6766E1B97F7"),
			(18, "A00A6D869B8A591DCF5BDFC20B7909C8F892305FA8967C462954974A132C2D69E1D355E8B61ED670CEEF4E0F40701AEC"),
			(19, "B79357E14D2E3A9F8777EB4BE37F3CBA1C234197BA4884EB8CB0B68640A5688F4147D40757EE8830F9B42BDFE665DD8C"),
		];
		let migrator = sqlx::migrate!("./migrations");
		assert_eq!(
			migrator.migrations.len(),
			PINS.len(),
			"a migration was added or removed: new migrations append a pin here"
		);
		for (migration, (version, pin)) in migrator.migrations.iter().zip(PINS) {
			assert_eq!(migration.version, *version, "migration files must stay in version order");
			let checksum: String = migration.checksum.iter().map(|b| format!("{b:02X}")).collect();
			assert_eq!(
				&checksum, pin,
				"migration {version} changed after it shipped; existing deployments refuse to boot"
			);
		}
	}

	#[tokio::test]
	async fn migration_16_converts_legacy_exclusions_and_preserves_existing_settings() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
		let pool = super::create_pool(&url).await.expect("initial pool");
		sqlx::query("INSERT INTO user (username, password_hash, role) VALUES ('bob', '', 'reader')")
			.execute(&pool)
			.await
			.expect("insert bob");
		let first: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name, include_in_update) VALUES (1, 'first', 2) RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("insert first legacy category");
		let second: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name, include_in_update) VALUES (1, 'second', 2) RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("insert second legacy category");
		let _bob_legacy: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name, include_in_update) VALUES (2, 'bob legacy', 2) RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("insert bob legacy category");
		let bob_existing: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name) VALUES (2, 'bob existing') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("insert bob existing category");

		sqlx::query("DELETE FROM setting WHERE key LIKE 'user.%.library.excluded_update_categories'")
			.execute(&pool)
			.await
			.expect("remove generated settings");
		sqlx::query("INSERT INTO setting (key, value) VALUES ('user.2.library.excluded_update_categories', ?)")
			.bind(format!("[{bob_existing}]"))
			.execute(&pool)
			.await
			.expect("seed existing setting");
		let deleted = sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 16")
			.execute(&pool)
			.await
			.expect("remove migration 16 bookkeeping");
		assert_eq!(deleted.rows_affected(), 1);
		pool.close().await;

		let pool = super::create_pool(&url).await.expect("rerun migrations");
		let mut admin = crate::update::LibraryUpdateSettings::load(&pool, 1, 0)
			.await
			.expect("load admin settings")
			.excluded_update_categories;
		admin.sort_unstable();
		assert_eq!(admin, vec![first, second]);
		let bob = crate::update::LibraryUpdateSettings::load(&pool, 2, 0)
			.await
			.expect("load bob settings")
			.excluded_update_categories;
		assert_eq!(bob, vec![bob_existing]);
		pool.close().await;
	}

	#[tokio::test]
	async fn migration_19_removes_non_admin_state_and_preserves_admin() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
		let pool = super::create_pool(&url).await.expect("initial pool");
		sqlx::query("UPDATE user SET role = 'reader' WHERE id = 1")
			.execute(&pool)
			.await
			.expect("demote built-in user");
		sqlx::query("INSERT INTO user (id, username, password_hash, role) VALUES (2, 'bob', '', 'reader')")
			.execute(&pool)
			.await
			.expect("insert reader");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, version, wasm_path) VALUES ('en.test', 'Test', 1, 'unused') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("insert source");
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'shared', 'Shared') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("insert manga");
		let chapter_id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key) VALUES (?, 'chapter-1') RETURNING id")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("insert chapter");
		for user_id in [1, 2] {
			let category_id: i64 = sqlx::query_scalar("INSERT INTO category (user_id, name) VALUES (?, 'Books') RETURNING id")
				.bind(user_id)
				.fetch_one(&pool)
				.await
				.expect("insert category");
			sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, ?)")
				.bind(category_id)
				.bind(manga_id)
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert category membership");
			sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, ?, 1, 1)")
				.bind(manga_id)
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert library state");
			sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, last_page_read) VALUES (?, ?, 7)")
				.bind(chapter_id)
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert progress");
			sqlx::query("INSERT INTO download (chapter_id, user_id, status) VALUES (?, ?, 'finished')")
				.bind(chapter_id)
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert download");
			sqlx::query("INSERT INTO filter_group (user_id, name) VALUES (?, 'Favorites')")
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert filter group");
			sqlx::query("INSERT INTO auth_token (token_hash, user_id, created_at, expires_at) VALUES (?, ?, 0, 9999999999)")
				.bind(format!("token-{user_id}"))
				.bind(user_id)
				.execute(&pool)
				.await
				.expect("insert session");
			sqlx::query("INSERT INTO setting (key, value) VALUES (?, 'true')")
				.bind(format!("user.{user_id}.incognito.enabled"))
				.execute(&pool)
				.await
				.expect("insert setting");
			sqlx::query(
				"INSERT INTO sync_change
				 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
				 VALUES (?, 'manga_user', 'key', 0, 1, '{}', '{}', 'peer', ?, 'key', 1)",
			)
			.bind(user_id)
			.bind(user_id)
			.execute(&pool)
			.await
			.expect("insert sync change");
			sqlx::query(
				"INSERT INTO sync_alias (origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key)
				 VALUES ('peer', ?, ?, 'manga_user', 'key', 'local')",
			)
			.bind(user_id)
			.bind(user_id)
			.execute(&pool)
			.await
			.expect("insert sync alias");
			sqlx::query(
				"INSERT INTO sync_origin_state (origin, origin_user_id, user_id, entity_kind, origin_entity_key, origin_sequence)
				 VALUES ('peer', ?, ?, 'manga_user', 'key', 1)",
			)
			.bind(user_id)
			.bind(user_id)
			.execute(&pool)
			.await
			.expect("insert sync origin state");
		}
		sqlx::query("UPDATE manga SET title = 'Reader updated', last_modified_at = 5000 WHERE id = ?")
			.bind(manga_id)
			.execute(&pool)
			.await
			.expect("update shared manga");
		sqlx::query("DELETE FROM sync_change WHERE entity_kind = 'manga' AND entity_key = ?")
			.bind(manga_id.to_string())
			.execute(&pool)
			.await
			.expect("remove old shared events");
		sqlx::query(
			"INSERT INTO sync_change
			 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
			 VALUES (1, 'manga', ?, 0, 5000, '{}', '{}', 'reader-peer', 2, ?, 9000)",
		)
		.bind(manga_id.to_string())
		.bind(manga_id.to_string())
		.execute(&pool)
		.await
		.expect("insert reader-shared event");
		let instance_id: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
			.fetch_one(&pool)
			.await
			.expect("instance id");
		let deleted = sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 19")
			.execute(&pool)
			.await
			.expect("remove migration 19 bookkeeping");
		assert_eq!(deleted.rows_affected(), 1);
		pool.close().await;

		let pool = super::create_pool(&url).await.expect("rerun migrations");
		assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user").fetch_one(&pool).await.expect("users"), 1);
		let role: String = sqlx::query_scalar("SELECT role FROM user WHERE id = 1")
			.fetch_one(&pool)
			.await
			.expect("admin role");
		assert_eq!(role, "admin");
		for table in [
			"manga_user",
			"chapter_user",
			"category",
			"category_manga",
			"download",
			"filter_group",
			"auth_token",
			"sync_change",
			"sync_alias",
			"sync_origin_state",
		] {
			let remaining: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE user_id != 1"))
				.fetch_one(&pool)
				.await
				.expect("remaining user rows");
			assert_eq!(remaining, 0, "{table}");
			let admin_rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE user_id = 1"))
				.fetch_one(&pool)
				.await
				.expect("admin rows");
			assert!(admin_rows > 0, "{table}");
		}
		for table in ["sync_change", "sync_alias", "sync_origin_state"] {
			let remaining: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE origin_user_id != 1"))
				.fetch_one(&pool)
				.await
				.expect("remaining origin rows");
			assert_eq!(remaining, 0, "{table}");
		}
		let settings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key LIKE 'user.%' AND key NOT LIKE 'user.1.%'")
			.fetch_one(&pool)
			.await
			.expect("non-admin settings");
		assert_eq!(settings, 0);
		let admin_setting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = 'user.1.incognito.enabled'")
			.fetch_one(&pool)
			.await
			.expect("admin setting");
		assert_eq!(admin_setting, 1);
		let current_instance: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
			.fetch_one(&pool)
			.await
			.expect("current instance id");
		assert_eq!(current_instance, instance_id);
		let baseline = crate::sync::changes_since_for_peer(&pool, 1, "new-peer", 0, 500)
			.await
			.expect("full baseline");
		let shared = baseline
			.changes
			.iter()
			.find(|change| change.entity_kind == "manga" && change.entity_key == manga_id.to_string())
			.expect("shared manga baseline");
		assert_eq!(shared.origin_user_id, 1);
		assert_eq!(shared.origin, current_instance);
		assert_eq!(shared.snapshot["title"], "Reader updated");
		assert!(shared.origin_sequence > 0);
		let marker: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = 'sync.single_owner_rebaseline'")
			.fetch_one(&pool)
			.await
			.expect("rebaseline marker");
		assert_eq!(marker, 0);
		pool.close().await;
	}
}
