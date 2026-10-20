//! fuel 预算的 DoS 语义：预算内结果正常、耗尽按 out-of-fuel trap、
//! 逐调用重置不累计、trap 后实例可复用。guest 是测试内现编的 WAT：
//! get_manga_list 的 page 参数即燃烧轮数，返回 data 段里的合法结果；
//! get_search_manga_list 在循环里反复调 host（std.current_date），
//! 钉 fuel 计费穿越 host 调用仍然生效
use aidoku_wasm::{
	DirectPolicy, Error, Limits, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle,
	StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

// 燃烧量按计费指令数估：wasmi 每 IR 指令约 1 单位，一轮循环 ~5-7 条。
// WITHIN 约 3M < BUDGET；FATAL 约 25M > BUDGET，两侧都留数倍余量，不依赖精确费率
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

/// 现编燃烧 guest：静态 data 段存一份 ABI 布局（len+cap+postcard）的合法
/// MangaPageResult，get_manga_list 烧完 page 轮就返回它
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
	// cap 字段宿主侧不读，填 0
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
		browser_ws: None,
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
	// 单次燃烧约为预算 1/3：不逐调用重置的话几轮内必累计爆掉（8 轮累计约 24M > 10M）
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
	// trap 后实例不废：下一次调用拿到新的满预算照常跑
	let result = handle
		.get_manga_list(listing(), 1000)
		.await
		.expect("reused after trap");
	assert_eq!(result.entries.len(), 1);
}

#[tokio::test]
async fn fuel_exhaustion_traps_across_host_calls() {
	// 小预算加速 trap：host 调用本身不计费，但循环里的 call/br 逐条扣
	let handle = spawn(100_000).await;
	let err = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect_err("host-call loop must trap");
	assert_out_of_fuel(err);
}
