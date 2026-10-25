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
		crate::sync::compact::rebaseline(&pool)
			.await
			.map_err(|e| Error::Sync(e.message().to_string()))?;
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

pub async fn remove_category_exclusions(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	category_id: i64,
) -> Result<(), sqlx::Error> {
	let update_key = format!("user.{user_id}.library.excluded_update_categories");
	let download_key = format!("user.{user_id}.library.excluded_download_categories");
	sqlx::query(
		"UPDATE setting
		 SET value = (
		  SELECT json_group_array(item.value) FROM json_each(setting.value) AS item
		  WHERE item.value <> ?
		 )
		 WHERE key IN (?, ?)
		  AND EXISTS (
		   SELECT 1 FROM json_each(setting.value) AS item WHERE item.value = ?
		  )",
	)
	.bind(category_id)
	.bind(update_key)
	.bind(download_key)
	.bind(category_id)
	.execute(&mut **tx)
	.await?;
	Ok(())
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
			(19, "E7FBBD708D51ABE490FC7258E694CE7F479548E5ED0E9544563CF7A990F1F7C8F30B4C28A565AD66F58F607AD88CE8B5"),
			(20, "D07AFDEDADF92B2245A9C534B3A1F850C33C19BF5402EE4488559DCF6A671C4476501D3645EF3B7C6586714D5631E278"),
			(21, "4948B2920F5805B91B5BA635C67C55BCA2EE7F6B67E6F9C99C9062A8D5C142185BDD8942FBDBF3678F65631DEB98C7EB"),
			(22, "355E27DBABAA8F72A28E96069416BD5705F87974D6B52EB3C74CFB989595C28A5DBAEC14EA6DD437D79E58E6D8A376BC"),
			(23, "E0786EC097D86F201751F36198D1D8070E4B1DFC94966618BEAB41F5C0143B5D41FE1C664DBE8110D083154D2896C394"),
			(24, "07A2159B0996857C1D3AC78A5722CBAAA5E625D7D097B7428F02A808AD929B5A8B6E64034A4EB158D5627280C0D02065"),
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
	async fn migration_20_converts_admin_exclusions_and_preserves_existing_setting() {
		use sqlx::migrate::Migrate;
		use std::str::FromStr;

		for preserve_existing in [false, true] {
			let tmp = tempfile::tempdir().expect("tempdir");
			let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
			let options = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
				.expect("database url")
				.create_if_missing(true)
				.foreign_keys(true);
			let pool = sqlx::sqlite::SqlitePoolOptions::new()
				.max_connections(1)
				.connect_with(options)
				.await
				.expect("connect pre-migration database");
			let migrator = sqlx::migrate!("./migrations");
			let mut conn = pool.acquire().await.expect("migration connection");
			conn.ensure_migrations_table().await.expect("migration table");
			for migration in migrator.iter().filter(|migration| migration.version < 20) {
				conn.apply(migration).await.expect("apply pre-0020 migration");
			}
			drop(conn);

			let first: i64 = sqlx::query_scalar(
				"INSERT INTO category (user_id, name, include_in_download) VALUES (1, 'first', 2) RETURNING id",
			)
			.fetch_one(&pool)
			.await
			.expect("insert first legacy category");
			let second: i64 = sqlx::query_scalar(
				"INSERT INTO category (user_id, name, include_in_download) VALUES (1, 'second', 2) RETURNING id",
			)
			.fetch_one(&pool)
			.await
			.expect("insert second legacy category");
			let included: i64 = sqlx::query_scalar(
				"INSERT INTO category (user_id, name, include_in_download) VALUES (1, 'included', 1) RETURNING id",
			)
			.fetch_one(&pool)
			.await
			.expect("insert include-only category");
			let preserved: i64 = sqlx::query_scalar(
				"INSERT INTO category (user_id, name) VALUES (1, 'preserved') RETURNING id",
			)
			.fetch_one(&pool)
			.await
			.expect("insert preserved category");
			sqlx::query("INSERT INTO user (id, username, password_hash, role) VALUES (2, 'bob', '', 'reader')")
				.execute(&pool)
				.await
				.expect("insert non-admin user");
			sqlx::query("INSERT INTO category (user_id, name, include_in_download) VALUES (2, 'bob', 2)")
				.execute(&pool)
				.await
				.expect("insert non-admin legacy category");
			if preserve_existing {
				sqlx::query("INSERT INTO setting (key, value) VALUES ('user.1.library.excluded_download_categories', ?)")
					.bind(format!("[{preserved}]"))
					.execute(&pool)
					.await
					.expect("seed existing download exclusions");
			}
			sqlx::query("INSERT INTO setting (key, value) VALUES ('auto_download_new_chapters', 'true')")
				.execute(&pool)
				.await
				.expect("seed enabled auto download");
			let mut conn = pool.acquire().await.expect("migration connection");
			conn.apply(migrator.iter().find(|migration| migration.version == 20).expect("migration 20"))
				.await
				.expect("apply 0020 migration");
			drop(conn);

			let auto_download: String = sqlx::query_scalar(
				"SELECT value FROM setting WHERE key = 'auto_download_new_chapters'",
			)
			.fetch_one(&pool)
			.await
			.expect("migrated auto download setting");
			assert!(!serde_json::from_str::<bool>(&auto_download).expect("auto download flag"));
			let raw: String = sqlx::query_scalar(
				"SELECT value FROM setting WHERE key = 'user.1.library.excluded_download_categories'",
			)
			.fetch_one(&pool)
			.await
			.expect("migrated admin exclusions");
			let mut ids: Vec<i64> = serde_json::from_str(&raw).expect("exclusion ids");
			ids.sort_unstable();
			let mut expected = if preserve_existing { vec![preserved] } else { vec![first, second] };
			expected.sort_unstable();
			assert_eq!(ids, expected, "preserve_existing={preserve_existing}");
			let non_admin: Option<String> = sqlx::query_scalar(
				"SELECT value FROM setting WHERE key = 'user.2.library.excluded_download_categories'",
			)
			.fetch_optional(&pool)
			.await
			.expect("non-admin exclusion setting");
			assert!(non_admin.is_none());
			let legacy: i64 = sqlx::query_scalar("SELECT include_in_download FROM category WHERE id = ?")
				.bind(first)
				.fetch_one(&pool)
				.await
				.expect("legacy column remains");
			assert_eq!(legacy, 2);
			let included_flag: i64 = sqlx::query_scalar("SELECT include_in_download FROM category WHERE id = ?")
				.bind(included)
				.fetch_one(&pool)
				.await
				.expect("include-only column remains");
			assert_eq!(included_flag, 1);

			let post: i64 = sqlx::query_scalar(
				"INSERT INTO category (user_id, name, include_in_download) VALUES (1, 'post', 2) RETURNING id",
			)
			.fetch_one(&pool)
			.await
			.expect("insert post-migration category");
			sqlx::query("UPDATE category SET name = 'renamed' WHERE id = ?")
				.bind(post)
				.execute(&pool)
				.await
				.expect("update post-migration category");
			sqlx::query("DELETE FROM category WHERE id = ?")
				.bind(post)
				.execute(&pool)
				.await
				.expect("delete post-migration category");
			let snapshots: Vec<String> = sqlx::query_scalar(
				"SELECT snapshot FROM sync_change WHERE entity_kind = 'category' AND entity_key = ? ORDER BY sequence",
			)
			.bind(post.to_string())
			.fetch_all(&pool)
			.await
			.expect("post-migration category events");
			assert_eq!(snapshots.len(), 3);
			for raw in snapshots {
				let snapshot: serde_json::Value = serde_json::from_str(&raw).expect("category snapshot");
				assert_eq!(snapshot["includeInDownload"], 0, "{snapshot}");
			}
			pool.close().await;
		}
	}

	#[tokio::test]
	async fn migration_21_invalidates_only_local_epub_page_cache() {
		use sqlx::migrate::Migrate;
		use std::str::FromStr;

		let tmp = tempfile::tempdir().expect("tempdir");
		let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
		let options = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
			.expect("database url")
			.create_if_missing(true)
			.foreign_keys(true);
		let pool = sqlx::sqlite::SqlitePoolOptions::new()
			.max_connections(1)
			.connect_with(options)
			.await
			.expect("connect pre-migration database");
		let migrator = sqlx::migrate!("./migrations");
		let mut conn = pool.acquire().await.expect("migration connection");
		conn.ensure_migrations_table().await.expect("migration table");
		for migration in migrator.iter().filter(|migration| migration.version < 21) {
			conn.apply(migration).await.expect("apply pre-0021 migration");
		}
		drop(conn);

		let source_id: i64 = sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
			.fetch_one(&pool)
			.await
			.expect("local source");
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'Book', 'Book') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("insert manga");
		let epub_id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key, page_count) VALUES (?, 'ch 1.epub', 2) RETURNING id")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("insert EPUB chapter");
		let cbz_id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key, page_count) VALUES (?, 'ch 2.cbz', 1) RETURNING id")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("insert CBZ chapter");
		for (chapter_id, idx) in [(epub_id, 0), (epub_id, 1), (cbz_id, 0)] {
			sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, ?, '{}')")
				.bind(chapter_id)
				.bind(idx)
				.execute(&pool)
				.await
				.expect("insert cached page");
		}
		sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, last_page_read) VALUES (?, 1, 1)")
			.bind(epub_id)
			.execute(&pool)
			.await
			.expect("insert reading progress");

		let mut conn = pool.acquire().await.expect("migration connection");
		for migration in migrator.iter().filter(|migration| migration.version == 21) {
			conn.apply(migration).await.expect("apply 0021 migration");
		}
		drop(conn);

		let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM page WHERE chapter_id = ?")
			.bind(epub_id)
			.fetch_one(&pool)
			.await
			.expect("EPUB cache count");
		assert_eq!(cached, 0, "local EPUB page cache must be invalidated");
		let page_count: i64 = sqlx::query_scalar("SELECT page_count FROM chapter WHERE id = ?")
			.bind(epub_id)
			.fetch_one(&pool)
			.await
			.expect("EPUB page count");
		assert_eq!(page_count, 0);
		let last_page_read: i64 = sqlx::query_scalar("SELECT last_page_read FROM chapter_user WHERE chapter_id = ? AND user_id = 1")
			.bind(epub_id)
			.fetch_one(&pool)
			.await
			.expect("reading progress");
		assert_eq!(last_page_read, 1, "derived page invalidation must preserve reading progress");
		let cbz_cached: i64 = sqlx::query_scalar("SELECT count(*) FROM page WHERE chapter_id = ?")
			.bind(cbz_id)
			.fetch_one(&pool)
			.await
			.expect("CBZ cache count");
		assert_eq!(cbz_cached, 1, "non-EPUB cache must be preserved");
		let cbz_page_count: i64 = sqlx::query_scalar("SELECT page_count FROM chapter WHERE id = ?")
			.bind(cbz_id)
			.fetch_one(&pool)
			.await
			.expect("CBZ page count");
		assert_eq!(cbz_page_count, 1);
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
		let original_sequence: i64 = sqlx::query_scalar(
			"SELECT sequence FROM sync_change WHERE entity_kind = 'manga' AND entity_key = ? ORDER BY sequence DESC LIMIT 1",
		)
		.bind(manga_id.to_string())
		.fetch_one(&pool)
		.await
		.expect("original shared sequence");
		sqlx::query("UPDATE manga SET title = 'Reader updated', last_modified_at = 5000 WHERE id = ?")
			.bind(manga_id)
			.execute(&pool)
			.await
			.expect("update shared manga");
		let (shared_identity, shared_snapshot): (String, String) =
			sqlx::query_as("SELECT identity, snapshot FROM sync_manga_state WHERE manga_id = ?")
				.bind(manga_id)
				.fetch_one(&pool)
				.await
				.expect("shared manga state");
		sqlx::query("DELETE FROM sync_change WHERE entity_kind = 'manga' AND entity_key = ?")
			.bind(manga_id.to_string())
			.execute(&pool)
			.await
			.expect("remove old shared events");
		sqlx::query(
			"INSERT INTO sync_change
			 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
			 VALUES (1, 'manga', ?, 0, 5000, ?, ?, 'reader-peer', 2, ?, 9000)",
		)
		.bind(manga_id.to_string())
		.bind(shared_identity)
		.bind(shared_snapshot)
		.bind(manga_id.to_string())
		.execute(&pool)
		.await
		.expect("insert reader-shared event");
		let deleted_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'deleted-shared', 'Delete me') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("insert deleted manga");
		let (deleted_identity, deleted_snapshot): (String, String) =
			sqlx::query_as("SELECT identity, snapshot FROM sync_manga_state WHERE manga_id = ?")
				.bind(deleted_id)
				.fetch_one(&pool)
				.await
				.expect("deleted manga state");
		sqlx::query("DELETE FROM manga WHERE id = ?")
			.bind(deleted_id)
			.execute(&pool)
			.await
			.expect("delete shared manga");
		sqlx::query("DELETE FROM sync_change WHERE entity_kind = 'manga' AND entity_key = ?")
			.bind(deleted_id.to_string())
			.execute(&pool)
			.await
			.expect("remove deleted manga events");
		let deleted_modified_at = serde_json::from_str::<serde_json::Value>(&deleted_snapshot)
			.expect("deleted snapshot json")["lastModifiedAt"]
			.as_i64()
			.expect("deleted snapshot time")
			+ 1;
		sqlx::query(
			"INSERT INTO sync_change
			 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
			 VALUES (1, 'manga', ?, 1, ?, ?, ?, 'reader-peer', 2, ?, 9001)",
		)
		.bind(deleted_id.to_string())
		.bind(deleted_modified_at)
		.bind(deleted_identity)
		.bind(deleted_snapshot)
		.bind(deleted_id.to_string())
		.execute(&pool)
		.await
		.expect("insert reader tombstone");
		let owner_deleted_id: i64 = sqlx::query_scalar(
			"INSERT INTO manga (id, source_id, key, title) VALUES (?, ?, 'removed-library', 'Removed library') RETURNING id",
		)
		.bind(deleted_id + 100)
		.bind(source_id)
		.fetch_one(&pool)
		.await
		.expect("insert owner-state manga");
		sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, 1, 1)")
			.bind(owner_deleted_id)
			.execute(&pool)
			.await
			.expect("insert old admin library state");
		sqlx::query("DELETE FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(owner_deleted_id)
			.execute(&pool)
			.await
			.expect("delete admin library state");
		let owner_deleted_key = format!("{owner_deleted_id}:1");
		let (owner_modified_at, owner_identity, owner_snapshot): (i64, String, String) = sqlx::query_as(
			"SELECT modified_at, identity, snapshot FROM sync_change
			 WHERE user_id = 1 AND entity_kind = 'manga_user' AND entity_key = ? AND deleted = 1
			 ORDER BY sequence DESC LIMIT 1",
		)
		.bind(&owner_deleted_key)
		.fetch_one(&pool)
		.await
		.expect("owner tombstone state");
		sqlx::query("DELETE FROM sync_change WHERE user_id = 1 AND entity_kind = 'manga_user' AND entity_key = ? AND deleted = 1")
			.bind(&owner_deleted_key)
			.execute(&pool)
			.await
			.expect("replace owner tombstone origin");
		sqlx::query(
			"INSERT INTO sync_change
			 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
			 VALUES (1, 'manga_user', ?, 1, ?, json_set(?, '$.userId', 2), json_set(?, '$.userId', 2), 'reader-peer', 2, ?, 9002)",
		)
		.bind(&owner_deleted_key)
		.bind(owner_modified_at)
		.bind(owner_identity)
		.bind(owner_snapshot)
		.bind(format!("{owner_deleted_id}:2"))
		.execute(&pool)
		.await
		.expect("insert reader owner-state tombstone");
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
		assert!(shared.modified_at > 5000);
		assert!(shared.origin_sequence > 0);
		let marker: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = 'sync.single_owner_rebaseline'")
			.fetch_one(&pool)
			.await
			.expect("rebaseline marker");
		assert_eq!(marker, 0);
		let tombstone = baseline
			.changes
			.iter()
			.rev()
			.find(|change| change.entity_kind == "manga" && change.entity_key == deleted_id.to_string())
			.expect("shared tombstone");
		assert!(tombstone.deleted);
		assert_eq!(tombstone.origin_user_id, 1);
		assert!(tombstone.modified_at > deleted_modified_at);
		let owner_tombstone = baseline
			.changes
			.iter()
			.rev()
			.find(|change| change.entity_kind == "manga_user" && change.entity_key == owner_deleted_key)
			.expect("owner-state tombstone");
		assert!(owner_tombstone.deleted);
		assert_eq!(owner_tombstone.origin, current_instance);
		assert_eq!(owner_tombstone.origin_user_id, 1);
		assert_eq!(owner_tombstone.origin_entity_key, owner_deleted_key);
		assert_eq!(owner_tombstone.identity["userId"], 1);
		assert_eq!(owner_tombstone.snapshot["userId"], 1);
		assert!(owner_tombstone.modified_at > owner_modified_at);

		let peer_url = format!("sqlite://{}/peer.db?mode=rwc", tmp.path().display());
		let peer_pool = super::create_pool(&peer_url).await.expect("peer pool");
		let peer_source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, version, wasm_path) VALUES ('en.test', 'Test', 1, 'unused') RETURNING id",
		)
		.fetch_one(&peer_pool)
		.await
		.expect("insert peer source");
		for (key, title) in [("shared", "Shared"), ("deleted-shared", "Delete me"), ("removed-library", "Removed library")] {
			sqlx::query("INSERT INTO manga (source_id, key, title) VALUES (?, ?, ?)")
				.bind(peer_source_id)
				.bind(key)
				.bind(title)
				.execute(&peer_pool)
				.await
				.expect("insert peer manga");
		}
		sqlx::query("UPDATE manga SET last_modified_at = 1 WHERE key IN ('deleted-shared', 'removed-library')")
			.execute(&peer_pool)
			.await
			.expect("set peer original versions");
		sqlx::query("UPDATE manga SET last_modified_at = 5000 WHERE key = 'shared'")
			.execute(&peer_pool)
			.await
			.expect("set peer conflict version");
		sqlx::query(
			"UPDATE sync_change SET modified_at = 1, snapshot = json_set(snapshot, '$.lastModifiedAt', 1)
			 WHERE entity_kind = 'manga' AND entity_key IN (SELECT CAST(id AS TEXT) FROM manga WHERE key IN ('deleted-shared', 'removed-library'))",
		)
		.execute(&peer_pool)
		.await
		.expect("set peer event versions");
		sqlx::query(
			"UPDATE sync_change
			 SET modified_at = 5000, snapshot = json_set(snapshot, '$.lastModifiedAt', 5000),
			  origin = 'reader-peer', origin_user_id = 2, origin_sequence = 9000
			 WHERE entity_kind = 'manga' AND entity_key = (SELECT CAST(id AS TEXT) FROM manga WHERE key = 'shared')",
		)
		.execute(&peer_pool)
		.await
		.expect("set peer equal-time conflict");
		sqlx::query(
			"INSERT INTO sync_origin_state (origin, origin_user_id, user_id, entity_kind, origin_entity_key, origin_sequence)
			 VALUES (?, 1, 1, 'manga', ?, ?)",
		)
		.bind(&current_instance)
		.bind(manga_id.to_string())
		.bind(original_sequence)
		.execute(&peer_pool)
		.await
		.expect("seed existing peer origin");
		let wire: Vec<crate::sync::WireChange> = baseline.changes.iter().cloned().map(crate::sync::WireChange::from).collect();
		crate::sync::apply::push(&peer_pool, &current_instance, 1, wire).await.expect("apply baseline");
		let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE key = 'shared'")
			.fetch_one(&peer_pool)
			.await
			.expect("updated shared title");
		assert_eq!(title, "Reader updated");
		let deleted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key = 'deleted-shared'")
			.fetch_one(&peer_pool)
			.await
			.expect("deleted manga count");
		assert_eq!(deleted_count, 0);
		let owner_count: i64 = sqlx::query_scalar(
			"SELECT COUNT(*) FROM manga_user mu JOIN manga m ON m.id = mu.manga_id WHERE m.key = 'removed-library'",
		)
		.fetch_one(&peer_pool)
		.await
		.expect("deleted owner-state count");
		assert_eq!(owner_count, 0);
		peer_pool.close().await;
		pool.close().await;
	}
}
