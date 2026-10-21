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
			(18, "EA9A99FAC68C0CDB990DEB47CAD24DC8DC0BEC613BA00FA18DBDC1395308891BD85BD2BDE234F0B2A0E15B510086521A"),
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
}
