//! DoS semantics of the fuel budget: within budget the result is normal, exhaustion traps
//! as out-of-fuel, the budget resets per call without accumulating, and the instance stays
//! usable after a trap. The guest is WAT compiled on the fly inside the test:
//! get_manga_list's page parameter is the number of burn rounds and it returns a valid
//! result from the data segment; get_search_manga_list calls a host function
//! (std.current_date) repeatedly inside the loop, pinning that fuel accounting still
//! applies across host calls
use aidoku_wasm::{
	DirectPolicy, Error, Limits, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle,
	StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

// Burn is estimated in billed instructions: wasmi charges about 1 unit per IR instruction
// and one loop round is ~5-7. WITHIN is about 3M < BUDGET; FATAL about 25M > BUDGET —
// both sides keep several-fold headroom and do not depend on the exact rate
const FUEL_BUDGET: u64 = 10_000_000;
const BURN_WITHIN: i32 = 500_000;
const BURN_FATAL: i32 = 4_000_000;

fn listing() -> aidoku::Listing {
	aidoku::Listing {
		id: "hot".into(),
		name: "Hot".into(),
		kind: aidoku::ListingKind::List,
	}
}

/// On-the-fly burn guest: the static data segment holds a valid MangaPageResult in ABI
/// layout (len+cap+postcard); get_manga_list returns it after burning page rounds
fn burn_guest() -> PathBuf {
	let payload = postcard::to_allocvec(&aidoku::MangaPageResult {
		entries: vec![aidoku::Manga {
			key: "fuel-fixture".into(),
			title: "Fuel Fixture".into(),
			..Default::default()
		}],
		has_next_page: false,
	})
	.expect("encode result");
	let mut item = ((payload.len() + 8) as i32).to_le_bytes().to_vec();
	// The cap field is not read on the host side; fill 0
	item.extend_from_slice(&[0, 0, 0, 0]);
	item.extend_from_slice(&payload);
	let data: String = item.iter().map(|b| format!("\\{b:02x}")).collect();
	let wat_text = format!(
		r#"
(module
  (import "std" "current_date" (func $now (result f64)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{data}")
  (func (export "start"))
  (func (export "get_manga_list") (param i32 i32) (result i32)
    (local $i i32)
    (block $done
      (loop $l
        (br_if $done (i32.ge_s (local.get $i) (local.get 1)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (i32.const 1024))
  (func (export "get_search_manga_list") (param i32 i32 i32) (result i32)
    (loop $s
      (drop (call $now))
      (br $s))
    unreachable)
)"#
	);
	let bytes = wat::parse_str(wat_text).expect("valid wat");
	let dir = std::env::temp_dir().join(format!("aidoku-wasm-fuel-test-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create fixture dir");
	let path = dir.join("main.wasm");
	std::fs::write(&path, bytes).expect("write fixture wasm");
	path
}

async fn spawn(fuel: u64) -> SourceHandle {
	let config = SourceConfig {
		key: "en.fueltest".to_string(),
		wasm_path: burn_guest(),
		limits: Limits {
			fuel,
			..Limits::default()
		},
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	};
	let (handle, features) = SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	assert!(features.manga_list, "夹具导出 get_manga_list 必须被识别");
	handle
}

fn assert_out_of_fuel(err: Error) {
	match err {
		Error::Trap(msg) => assert_eq!(msg, "out of fuel"),
		other => panic!("expected out-of-fuel trap, got {other}"),
	}
}

#[tokio::test]
async fn within_budget_result_is_unaffected() {
	let handle = spawn(FUEL_BUDGET).await;
	let result = handle
		.get_manga_list(listing(), BURN_WITHIN)
		.await
		.expect("within budget");
	assert_eq!(result.entries.len(), 1);
	assert_eq!(result.entries[0].key, "fuel-fixture");
	assert!(!result.has_next_page);
}

#[tokio::test]
async fn budget_exhaustion_traps_out_of_fuel() {
	let handle = spawn(FUEL_BUDGET).await;
	let err = handle
		.get_manga_list(listing(), BURN_FATAL)
		.await
		.expect_err("fatal burn must trap");
	assert_out_of_fuel(err);
}

#[tokio::test]
async fn budget_resets_per_call_and_instance_survives_trap() {
	let handle = spawn(FUEL_BUDGET).await;
	// A single burn is about 1/3 of the budget: without a per-call reset it would accumulate past the cap within a few rounds (8 rounds total ~24M > 10M)
	for round in 0..8 {
		handle
			.get_manga_list(listing(), BURN_WITHIN)
			.await
			.unwrap_or_else(|e| panic!("round {round}: {e}"));
	}
	let err = handle
		.get_manga_list(listing(), BURN_FATAL)
		.await
		.expect_err("fatal burn must trap");
	assert_out_of_fuel(err);
	// The instance is not dead after the trap: the next call gets a fresh full budget and runs as usual
	let result = handle
		.get_manga_list(listing(), 1000)
		.await
		.expect("reused after trap");
	assert_eq!(result.entries.len(), 1);
}

#[tokio::test]
async fn fuel_exhaustion_traps_across_host_calls() {
	// A small budget speeds up the trap: the host call itself is not billed, but the call/br instructions in the loop are charged one by one
	let handle = spawn(100_000).await;
	let err = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect_err("host-call loop must trap");
	assert_out_of_fuel(err);
}
