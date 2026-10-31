use crate::error::Error;
use sqlx::SqlitePool;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::str::FromStr;

/// Only reviewed historical artifacts belong here; deriving aliases from migration text would also accept
/// unreviewed edits.
const LEGACY_MIGRATION_CHECKSUMS: &[(i64, &[&str])] = &[
	(1, &[
		"0A32F7A388114C455681CDB0ED294244A840677D32F58CB18A0AFB5DFA0377288B89A35A7CE7375D4ACBAD57CE472D0B",
		"F9D6E68BEC408DCC687E65E4D97D4F9987CE9CB0EEEBBA048995344CFBA3AEF6FCE700B44C119ECDE1D586C97CCC4710",
		"6333085A0FC9C6B58386799D8C93BAE9AB8A2CB329E7C2A5A838AA2DBE3A6330E11CFCB659FA201967D29E71D446C2F8",
	]),
	(2, &[
		"F90D878C9B462832EF37AFDD96D0AA7FF7A90CF7905C93CEBDAEA164B8303DAD26B8231C2D47495A973FF0B0CF680226",
		"83179819E4B49211075933A1D2FCDB005A6EF760DDBEBD7C4DEF9F864B1BD72671F05AD2A54E8E20A4A724E93B5E7138",
		"9033269236C2674E1FEB565BF6A2C41373FA46E1802F79C0650EA04A0867FB3C39E1412907B5405E5C08EE099D2129FE",
	]),
	(3, &["1B5F3CB55FA7C81A0D2A6A5D1A698F944974F1FA1238745323C5F416EE26C8E57687A377A994F985C7EBE3375E89F029"]),
	(4, &[
		"2A2410B566CD5B92D818FD1C144D4CB0B904CD36C0872FEA8F4DBC2105C6DE0FE111894309A5368D03C8DF04AE9CB05F",
		"6ECB84D05B30D1557770FD49AF0BE8F00EEB7AAB41AFF80152DEB7FC3634FCABAE28F51BAC42CA1D17BE279C241E4954",
		"00638E47DD627EF5FD251D3A28498DD00E534848FA7912F00EA7AA93D7AE96C50388DB16B9CA088D8A57ACFE249426F8",
	]),
	(5, &[
		"2D3B48A60599AF7DC2D1BD3036EA1A423547C34E16E36D27D1E5C33A0954D8ECF260D4336B8A475B19B05B823C50F5EA",
		"2FC1AAAAC879F39295A3D74BE7FC12585251EA41B59C30B9FF5146728A71197C21F8981782D7EA5DA20ADB39FAF0149C",
		"87E63A176FE781A5FFDC8F581A8E71BC36B86634110DE2428AC9066F69F355B1AD20F0F41F745A84ED7D97ECFBE09FE2",
	]),
	(6, &[
		"DD02CC7EF46C7FEDF95DA0D295F225CAA78DB2FF0A5F66B5905262030C347B9F167EAC20E3F11D04AACA9BEE39ADA69D",
		"9D10E54D48F0C32EA964FDC255AE0EB170D63A2AB00E1B7E7BCCCFFF57E7B12C724656BF1777E74CD564B8812ABBF82A",
		"677E9A17D09AE74B294E9DF066C03859A7B861E24381BF3367494033A01BD75B3C37C814EED742BF12A87491DAE143BD",
	]),
	(7, &[
		"4E26367A7F96C839BDA030B87BE724476BE9A6F68C3D9B07A86D157A451C0C88E21D46ADBE86D99DF3668E97E5C39293",
		"C3AEDB481F0A00C668AE114691B62BEE7536C7AE3D66B26B21870E9FD821F1AF18DBCB083B92572492F1D5EF47866168",
		"4C9DF6A9621FA45A9E61CA7293D9E69CAA83EEA694558DEECA0D9D4AE7B0766B14530B14D81ED4D3EA60D99B5CB32488",
	]),
	(8, &["05D3A55F1D9E7498823BE25BAAEA64EE8E899C519572564EEAF46867713215433F20674401D0C95B3BDF8E1D2F868CCD"]),
	(9, &[
		"5919FC62FCD1C0C234CAD0EA2A033DDF869F1EBF7F6FF4D77F18D4E2136BCB4FB22E5F675F94E0A7639320D9EF7CDF30",
		"98C07EE10B63893B626C80280D11DD2370497FA241EF805ED206886D748392E9B87FAB6528CC01DEF45D5BA75E128BA2",
		"A0543EAC2AD7565B8D3F588B46298D7AB631C0296B256651591EFEAFC8224B23E9185B2C904738A13133659A4CE1DFD3",
	]),
	(10, &[
		"0F04A6323ABDD6D7C27FEB05F7603C3E7DFFC47F955E498FD6991EFE9F4B9B5E2993CEE2D39AE4E31046E0FE340DBCCC",
		"7577484F65644F20BAEC3FA268D60B000772FE243EFACBF21748D543ECCE6905506E3FD4359ACAC52560D94422D82EDA",
		"71C038C9BC3F70A5FF1965EE7BE1CF811CEAB47D578F7F8396F8E1FD7BFDE379A3000C9D15ADD84EE7B7645A6BEBFFC9",
	]),
	(11, &[
		"9C5CD7FF3766B4F91B7FDFBBA814B95D0C9773E0461EB2C81D1ACC630CA949E024BD26DF786C22EC0BF9C079ABB3B370",
		"981CA8FC5758999EF715AC2A4C7594D7FE7DD696C411E7BC7C8A28AD68C1095D6D20CD3B1E191D8A8829AE4CCCCBA11E",
		"0D116C7C8E6E1D8B189F2DBC713DBD7CA6B7F3D29B03A319710B82F8A4873E2EEC6A30AC5BAACAA831C1295102D1239C",
	]),
	(12, &[
		"CB4F4889F693EBC5CF83387C447C1B2B229A65A2C0D739399946295C3AE1F44F4B47A520CC831F7057B2F62936AA4EE3",
		"1785F25EA121AF6B352B1A71527DD78FCAF331B56C162A48C240D81349229CB592802C4CF94F43B8A89BFCFF7E7003A5",
		"FE7A143F9AABAE0F56578587AF335877DF939500916CDC48909D01820470A2A3391B457F7837AF59AB51CE00A9904F10",
	]),
	(13, &["AFEA83F78159738F14881DABA227D5D27DD578FBFBC09E6CCBEAE16A54532AF36FE6D4EF74CA42B93E3EFC8F22AAB755"]),
	(14, &["992C56FF5FB96E5E17C5C926AE71E0B8033F4713D6C479FC78F0D23C6C9E2F5B6051B1FEF0862F710D9482D6F6B759A4"]),
	(15, &["C55D2D59D5B7C5AF9EDC524A6EC153FD82D51BFC1A52F3A3F1BFC24E162C54F35775F83CB53BDD9D37B80F2577DEC301"]),
	(16, &["82BD50096CC6EB5EA5A7C2DAA2F4C47AD12840B848D2ED74437593A8D2ECEAB8498F65B49152C9D649647B9581D41084"]),
	(17, &["766DC769B584DEFBBFAE914C9FFAD310596DF43FAD84684BBE2337030F318C0A9066F7E7A68EDFCBC7E9A7CB66C8C301"]),
	(18, &["77CC68C849A72257FBA5D613A6B211EAC966A788BEFF2EADC3F6456A6F2EB2A0F33D194461E350F9A8A4F83D0431F3D0"]),
	(19, &["D0314C5C5C85A566A93BED6782FE2486D77063D954ED11507D16CAE414B8CE902F6AAB2E0FA41F8D5336D0D2DDB5B8B7"]),
	(20, &["FD6A230C00AAE64A6FFA6672BEDC5B6E05904BE26FEFDABAADF0A340CE22B3AE4BE6112CF2CBF89564F6430E756919BA"]),
	(21, &["7A81A9CCE76CB09E1033455BD1635CE4ED0969DAA16C30837C73E73D74C6213D873303DC7B332FA07FB65425F4E37DF0"]),
	(22, &["F6C4BFF54C9271013C46AADEB63E2584A94BF7FDAFAB8E70D22A0E0D2148CE2D95FE44DEEE6D61DD2D6BB663ADC6FF59"]),
	(23, &["74E8B3E3AD9397A714D9576892269BB464785956B17FF0B23857C523B3D520144A55B0CD874D4304CA9F5B04F549AF31"]),
	(24, &[
		"07A2159B0996857C1D3AC78A5722CBAAA5E625D7D097B7428F02A808AD929B5A8B6E64034A4EB158D5627280C0D02065",
		"60728293ED53593660887F681F4CA1F4BE91B77697C64A435BDB00218D12D2D64D29E83AB4D9560254CD2B4DC0204C3A",
	]),
];

async fn normalize_migration_checksums(pool: &SqlitePool, migrator: &Migrator) -> Result<bool, sqlx::Error> {
	let mut tx = begin_write(pool).await?;
	let applied: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
		.fetch_all(&mut *tx)
		.await?;
	let mut updates = Vec::new();
	for (version, stored) in applied {
		let Some(migration) = migrator.iter().find(|migration| migration.version == version) else {
			return Ok(false);
		};
		if stored.as_slice() == migration.checksum.as_ref() {
			continue;
		}
		let stored_hex: String = stored.iter().map(|byte| format!("{byte:02X}")).collect();
		if !LEGACY_MIGRATION_CHECKSUMS
			.iter()
			.any(|(candidate, checksums)| *candidate == version && checksums.contains(&stored_hex.as_str()))
		{
			return Ok(false);
		}
		updates.push((version, stored, migration.checksum.to_vec()));
	}
	for (version, stored, checksum) in updates {
		let changed = sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ? AND checksum = ?")
			.bind(checksum)
			.bind(version)
			.bind(stored)
			.execute(&mut *tx)
			.await?
			.rows_affected();
		if changed != 1 {
			return Ok(false);
		}
	}
	tx.commit().await?;
	Ok(true)
}

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
	let migrator = sqlx::migrate!("./migrations");
	match migrator.run(&pool).await {
		Ok(()) => {}
		Err(error @ MigrateError::VersionMismatch(_)) => {
			if !normalize_migration_checksums(&pool, &migrator).await? {
				return Err(error.into());
			}
			migrator.run(&pool).await?;
		}
		Err(error) => return Err(error.into()),
	}
	if crate::server_state::get(&pool, crate::server_state::SINGLE_OWNER_REBASELINE).await?.is_some() {
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

#[cfg(test)]
mod tests {
	use super::SqlitePool;

	/// Existing databases reject changed migration bytes unless their exact old checksum is registered.
	/// Pin the canonical SHA-384 for every shipped file; new migrations append a pin.
	#[tokio::test]
	async fn migration_32_moves_server_state_and_the_origin_trigger() {
		use sqlx::migrate::Migrate;
		use std::str::FromStr;
		let tmp = tempfile::tempdir().expect("tmp");
		let url = format!("sqlite://{}/state.db?mode=rwc", tmp.path().display());
		let options = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
			.expect("database url")
			.create_if_missing(true)
			.foreign_keys(true);
		let pool = sqlx::sqlite::SqlitePoolOptions::new()
			.max_connections(1)
			.connect_with(options)
			.await
			.expect("connect");
		let migrator = sqlx::migrate!("./migrations");
		let mut conn = pool.acquire().await.expect("migration connection");
		conn.ensure_migrations_table().await.expect("migration table");
		for migration in migrator.iter().filter(|migration| migration.version < 32) {
			conn.apply(migration).await.expect("apply pre-0032 migration");
		}
		drop(conn);
		let instance_id: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
			.fetch_one(&pool)
			.await
			.expect("instance id");
		for (key, value) in [
			("library.last_scheduled_run_at", "11"),
			("library.last_update_finished_at", "22"),
			("local.last_scan_at", "33"),
			("backup.last_run", "null"),
			("sync.single_owner_rebaseline", "true"),
			("custom.option", "1"),
		] {
			sqlx::query("INSERT OR REPLACE INTO setting (key, value) VALUES (?, ?)")
				.bind(key)
				.bind(value)
				.execute(&pool)
				.await
				.expect("seed setting");
		}

		let mut conn = pool.acquire().await.expect("migration connection");
		conn.apply(migrator.iter().find(|migration| migration.version == 32).expect("migration 32"))
			.await
			.expect("apply 0032 migration");
		drop(conn);

		let moved: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM server_state ORDER BY key")
			.fetch_all(&pool)
			.await
			.expect("server state");
		assert_eq!(
			moved,
			[
				("library.last_scheduled_run_at", "11"),
				("library.last_update_finished_at", "22"),
				("local.last_scan_at", "33"),
				("sync.instance_id", instance_id.as_str()),
				("sync.single_owner_rebaseline", "true"),
			]
			.map(|(key, value)| (key.to_string(), value.to_string()))
		);
		let mut left: Vec<&str> = crate::server_state::KEYS.to_vec();
		left.push("custom.option");
		let mut remaining = 0;
		for key in left {
			let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = ?")
				.bind(key)
				.fetch_one(&pool)
				.await
				.expect("setting rows");
			remaining += count;
		}
		assert_eq!(remaining, 1, "only custom.option stays");

		sqlx::query("DELETE FROM server_state WHERE key = 'sync.single_owner_rebaseline'")
			.execute(&pool)
			.await
			.expect("clear marker");
		sqlx::query(
			"INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
			 VALUES (1, 'manga', '7', 0, 1, '{}', '{}')",
		)
		.execute(&pool)
		.await
		.expect("insert sync change");
		let origin: String = sqlx::query_scalar("SELECT origin FROM sync_change WHERE entity_key = '7'")
			.fetch_one(&pool)
			.await
			.expect("origin");
		assert_eq!(origin, instance_id);
	}

		#[test]
	fn shipped_migrations_are_immutable() {
		static PINS: &[(i64, &str)] = &[
			(1, "2BBFC0F9B10375B680176FBC7CE075876B0D43D90130E42FDD1EC1D2A560A91F4476D87DBB9C0B5A308B3C48890C0BFE"),
			(2, "D1FE7428542831CAAF01356004ACE36008C9E2604C4774F1CD548E2EE8B8EC6386EC360EA9DDAA494D9C305D85EF5E1F"),
			(3, "80C6884AA4E7DED4D12CD811053401A14D69E367A1198E9627A77398F591E4DC76789978B79D1FDC9EF075AB30867740"),
			(4, "2A36EBD887C8D99AA6E839C0BDCC6B382EDC39A5900F607BBCB6CE8F464DB7EE605865F56F30F139E09D799E5BAFBDAD"),
			(5, "1C5C49BCE9352908302CD5598446EF326F8605B64DBDB54BC58E6295E79F6FBEB0340A6F4840C9400D8C7314F90889B9"),
			(6, "271EC3A8E5CA791B9DCD976106FE2DE3C9C9442E5B4DA0FCE6822AD7AF58E60A50184933772285E96911C0B58E3B4BDF"),
			(7, "B55448C4795B655F3800548D19DFF7ACA45B5EA7A20D8E23EC77896BCCD5283C91A94C6A88683FF94B8CDAAFF4B6C7EC"),
			(8, "016A8DDD216337B4EF752DD381F936B3D6CC8AE0FB1A3BF10E61AB89046EC53D5B945DE5233B8841C43292E628BEAACE"),
			(9, "2A38E3CD5A3159F3C12593CA2A8775C01C7C32DB9611521B797A14F4A2E07AEA9D46FD1ECED496AC24128767BDC2C6ED"),
			(10, "AB5F1422B6793F9042AE2859B25E1F18F6B174E20D1623EAFD2DE9EBB0EA61CFBEE7125A3AE13DD09D9EC09E0BB16A5F"),
			(11, "020926C55040999A906FC446FEBBB07C0E3DFA136D0D1500774535F2A01E3720A349E98037052D10B45CEAD41EF804FD"),
			(12, "33E3D54667BB06025A355ED95F24E528D63AB861A72A4ABB7654E86845A34BD2498AD3A2D0DAD497EF1284ABA543345E"),
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
			(24, "B91C6E8048633C33A9AD09AC1DBDDA4D9E4EA95A436F547EAC5F8D4FBD6C4153A43420A9AF4860DA36CDACE750124C64"),
			(25, "711639E0BDBB1082011512CC4C73170D35D3FA7DBC06A1C190ACCBA702EEA2E7390E9CFB346159C320864D2B4FBCC8C3"),
			(26, "0D573C0D3050EF3EF0CA3B1E9490A4B9FB488272E17846785E6437A620B72814FDE0A25839EE90B6BB723EC88B87CD8B"),
			(27, "55E8BD49FC573A526C3A10B997171C3BC0F6C1BEEC6A046F2BB0091594389845B36749585EAA6AA65CFF7181F6C398A8"),
			(28, "B3CDC27CEF5E45DF5CADEE6E68A2AF85E7A230D081AEE9F2BF39FE5334F463552A9BB7C299686281F4C2834009A80C8D"),
			(29, "8C2D4542C2632F75059B8E195AE8426B4B42CDABAA090F3BF1109D454848FFD179352E51749B100FF74135775E0932D4"),
			(30, "C50F50303BE725E33BE955FE38768D8D948311487F450D372EFFFADB5E17B58E2BE0D08DA9112DBA66A904351E8647F6"),
			(31, "F149F163796123A8F0A99F27943E7A895E76CB07FD73C9B937AFFE523801C83D5A851F14A6CE4506D76A46FFCBB0A120"),
			(32, "CB18CACFE9A9F3A939FF7A7C3294BD7449B16FF45DEA56355FCFDC688016A33DD6F76552894B0742B0527CAE1F54FFF9"),
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

	#[test]
	fn shipped_migrations_use_ascii_comments_and_lf() {
		for migration in sqlx::migrate!("./migrations").iter() {
			assert!(!migration.sql.contains('\r'), "migration {} must use LF line endings", migration.version);
			for comment in migration.sql.lines().filter(|line| line.trim_start().starts_with("--")) {
				assert!(comment.is_ascii(), "migration {} comments must be English", migration.version);
			}
		}
	}

	fn decode_checksum(hex: &str) -> Vec<u8> {
		assert_eq!(hex.len(), 96);
		(0..hex.len())
			.step_by(2)
			.map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex checksum"))
			.collect()
	}

	async fn write_checksum(pool: &SqlitePool, version: i64, checksum: &str) {
		let changed = sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
			.bind(decode_checksum(checksum))
			.bind(version)
			.execute(pool)
			.await
			.expect("write migration checksum")
			.rows_affected();
		assert_eq!(changed, 1);
	}

	#[tokio::test]
	async fn legacy_checksum_history_normalizes_and_continues_migrations() {
		for (variant, legacy_checksum) in [
			("Chinese LF", "0A32F7A388114C455681CDB0ED294244A840677D32F58CB18A0AFB5DFA0377288B89A35A7CE7375D4ACBAD57CE472D0B"),
			("Chinese CRLF", "F9D6E68BEC408DCC687E65E4D97D4F9987CE9CB0EEEBBA048995344CFBA3AEF6FCE700B44C119ECDE1D586C97CCC4710"),
			("English CRLF", "6333085A0FC9C6B58386799D8C93BAE9AB8A2CB329E7C2A5A838AA2DBE3A6330E11CFCB659FA201967D29E71D446C2F8"),
		] {
			let tmp = tempfile::tempdir().expect("tempdir");
			let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
			let pool = SqlitePool::connect(&url).await.expect("blank database");
			sqlx::raw_sql(
				"CREATE TABLE IF NOT EXISTS _sqlx_migrations (
					version BIGINT PRIMARY KEY,
					description TEXT NOT NULL,
					installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
					success BOOLEAN NOT NULL,
					checksum BLOB NOT NULL,
					execution_time BIGINT NOT NULL
				)",
			)
			.execute(&pool)
			.await
			.expect("migration bookkeeping");
			let first = sqlx::migrate!("./migrations").iter().next().expect("initial migration");
			sqlx::raw_sql(first.sql.as_ref()).execute(&pool).await.expect("apply initial schema");
			sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, TRUE, ?, 0)")
				.bind(first.version)
				.bind(first.description.as_ref())
				.bind(decode_checksum(legacy_checksum))
				.execute(&pool)
				.await
				.expect("record legacy initial migration");
			pool.close().await;

			let pool = super::create_pool(&url).await.unwrap_or_else(|error| panic!("open {variant} database: {error}"));
			for migration in sqlx::migrate!("./migrations").iter() {
				let stored: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
					.bind(migration.version)
					.fetch_one(&pool)
					.await
					.expect("normalized migration record");
				assert_eq!(stored.as_slice(), migration.checksum.as_ref(), "{variant}: migration {} was not normalized", migration.version);
			}
			let api_key_table: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'api_key'")
				.fetch_one(&pool)
				.await
				.expect("api_key table");
			assert_eq!(api_key_table, 1, "{variant}: the pending migration must run after normalization");
			pool.close().await;
		}
	}

	#[tokio::test]
	async fn api_key_chinese_history_normalizes() {
		for legacy_checksum in [
			"07A2159B0996857C1D3AC78A5722CBAAA5E625D7D097B7428F02A808AD929B5A8B6E64034A4EB158D5627280C0D02065",
			"60728293ED53593660887F681F4CA1F4BE91B77697C64A435BDB00218D12D2D64D29E83AB4D9560254CD2B4DC0204C3A",
		] {
			let tmp = tempfile::tempdir().expect("tempdir");
			let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
			let pool = super::create_pool(&url).await.expect("seed current history");
			write_checksum(&pool, 24, legacy_checksum).await;
			pool.close().await;

			let pool = super::create_pool(&url).await.expect("open Chinese API key history");
			let stored: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 24")
				.fetch_one(&pool)
				.await
				.expect("normalized API key checksum");
			assert_eq!(stored, decode_checksum("B91C6E8048633C33A9AD09AC1DBDDA4D9E4EA95A436F547EAC5F8D4FBD6C4153A43420A9AF4860DA36CDACE750124C64"));
			pool.close().await;
		}
	}

	#[tokio::test]
	async fn unknown_checksum_does_not_normalize_any_history() {
		const CHINESE_CRLF: &str = "F9D6E68BEC408DCC687E65E4D97D4F9987CE9CB0EEEBBA048995344CFBA3AEF6FCE700B44C119ECDE1D586C97CCC4710";
		const ENGLISH_CRLF: &str = "9033269236C2674E1FEB565BF6A2C41373FA46E1802F79C0650EA04A0867FB3C39E1412907B5405E5C08EE099D2129FE";

		let tmp = tempfile::tempdir().expect("tempdir");
		let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
		let pool = super::create_pool(&url).await.expect("seed current history");
		write_checksum(&pool, 1, CHINESE_CRLF).await;
		write_checksum(&pool, 2, ENGLISH_CRLF).await;
		sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = 3")
			.bind(vec![0u8; 48])
			.execute(&pool)
			.await
			.expect("write unknown checksum");
		pool.close().await;

		let error = super::create_pool(&url).await.expect_err("unknown checksum must abort startup");
		assert!(matches!(error, crate::error::Error::Migrate(sqlx::migrate::MigrateError::VersionMismatch(1))), "unexpected startup error: {error:?}");
		let pool = SqlitePool::connect(&url).await.expect("inspect unchanged history");
		for (version, expected) in [(1, CHINESE_CRLF), (2, ENGLISH_CRLF)] {
			let stored: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
				.bind(version)
				.fetch_one(&pool)
				.await
				.expect("legacy checksum");
			assert_eq!(stored, decode_checksum(expected), "migration {version} changed before all rows validated");
		}
		let unknown: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 3")
			.fetch_one(&pool)
			.await
			.expect("unknown checksum");
		assert_eq!(unknown, vec![0u8; 48], "the unknown checksum must remain untouched");
		pool.close().await;
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
		let instance_id: String = sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'sync.instance_id'")
			.fetch_one(&pool)
			.await
			.expect("instance id");
		let deleted = sqlx::query("DELETE FROM _sqlx_migrations WHERE version IN (19, 32)")
			.execute(&pool)
			.await
			.expect("remove migration 19 and 32 bookkeeping");
		assert_eq!(deleted.rows_affected(), 2);
		// Back to the shape 0019 runs against, so 0019 and then 0032 replay in shipping order. One transaction
		// keeps these on one connection: another pooled connection can parse with a stale schema.
		let mut tx = pool.begin().await.expect("undo transaction");
		for statement in [
			"INSERT INTO setting (key, value) SELECT key, value FROM server_state",
			"DROP TRIGGER sync_change_origin_insert",
			"DROP TABLE server_state",
			"CREATE TRIGGER sync_change_origin_insert AFTER INSERT ON sync_change WHEN 0 BEGIN SELECT 1; END",
		] {
			sqlx::query(statement).execute(&mut *tx).await.expect("undo server_state");
		}
		tx.commit().await.expect("commit undo");
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
		let current_instance: String = sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'sync.instance_id'")
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
		let marker: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_state WHERE key = 'sync.single_owner_rebaseline'")
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
