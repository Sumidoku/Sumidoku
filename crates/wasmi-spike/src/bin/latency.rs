//! listing/detail/page 流程的 guest 可见延迟（issue #169 scope 4）：
//! 同一 fixture、同一 ABI 数据流，两引擎各跑 N 轮取中位/p95/min。
//! 另测实例冷启动（引擎+编译/翻译+实例化+start）中位数

use std::time::Instant;
use wasmi_spike::wasmer_host::WasmerHost;
use wasmi_spike::wasmi_host::WasmiHost;
use wasmi_spike::{Host, HostExt, Limits, fixture_wasm};

struct Stats {
	median: f64,
	p95: f64,
	min: f64,
}

fn stats(mut samples: Vec<f64>) -> Stats {
	samples.sort_by(f64::total_cmp);
	let n = samples.len();
	assert!(n > 0, "stats needs samples");
	Stats {
		median: samples[n / 2],
		p95: samples[(n * 95 / 100).min(n - 1)],
		min: samples[0],
	}
}

fn listing() -> aidoku::Listing {
	aidoku::Listing {
		id: "hot".into(),
		name: "Hot Updates".into(),
		kind: aidoku::ListingKind::List,
	}
}

fn manga() -> aidoku::Manga {
	aidoku::Manga {
		key: "echo".into(),
		title: "hot|Hot Updates|list".into(),
		..Default::default()
	}
}

fn chapter() -> aidoku::Chapter {
	aidoku::Chapter {
		key: "c1".into(),
		..Default::default()
	}
}

struct FlowRow {
	list: Stats,
	detail: Stats,
	page: Stats,
	flow: Stats,
}

fn measure_flow(host: &mut impl Host, iters: usize) -> FlowRow {
	let (mut list, mut detail, mut page, mut flow) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
	for _ in 0..iters {
		let t0 = Instant::now();
		let result = host.get_manga_list(&listing(), 1).expect("manga list");
		let t1 = Instant::now();
		assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
		let updated = host.get_manga_update(&manga(), true, false).expect("manga update");
		let t2 = Instant::now();
		assert_eq!(updated.authors.as_deref(), Some(&["Author One".to_string()][..]));
		let pages = host.get_page_list(&manga(), &chapter()).expect("page list");
		let t3 = Instant::now();
		assert!(pages.is_empty());
		list.push(t1.duration_since(t0).as_secs_f64() * 1e3);
		detail.push(t2.duration_since(t1).as_secs_f64() * 1e3);
		page.push(t3.duration_since(t2).as_secs_f64() * 1e3);
		flow.push(t3.duration_since(t0).as_secs_f64() * 1e3);
	}
	FlowRow {
		list: stats(list),
		detail: stats(detail),
		page: stats(page),
		flow: stats(flow),
	}
}

fn measure_builds<F: Fn() -> R, R>(build: F) -> Stats {
	let mut samples = Vec::new();
	for _ in 0..12 {
		let t0 = Instant::now();
		let host = build();
		samples.push(t0.elapsed().as_secs_f64() * 1e3);
		drop(host);
	}
	stats(samples)
}

fn print_stats(label: &str, s: &Stats) {
	println!("{:<28} {:>10.3} {:>10.3} {:>10.3}", label, s.median, s.p95, s.min);
}

fn main() {
	let iters: usize = std::env::args()
		.nth(1)
		.and_then(|a| a.parse().ok())
		.unwrap_or(300);
	let bytes = fixture_wasm();
	println!(
		"platform: {} {}, iterations per engine: {iters} (warmup 30)",
		std::env::consts::OS,
		std::env::consts::ARCH
	);
	println!();
	println!("{:<28} {:>10} {:>10} {:>10}", "op", "median ms", "p95 ms", "min ms");

	let build_wasmer = measure_builds(|| WasmerHost::build(&bytes, Limits::default()).expect("build wasmer"));
	let mut wasmer = WasmerHost::build(&bytes, Limits::default()).expect("build wasmer");
	for _ in 0..30 {
		std::hint::black_box(host_get_list(&mut wasmer));
	}
	let row = measure_flow(&mut wasmer, iters);
	print_stats("wasmer get_manga_list", &row.list);
	print_stats("wasmer get_manga_update", &row.detail);
	print_stats("wasmer get_page_list", &row.page);
	print_stats("wasmer flow (3 ops)", &row.flow);
	print_stats("wasmer cold build", &build_wasmer);
	println!();

	let build_wasmi = measure_builds(|| WasmiHost::build(&bytes, Limits::default()).expect("build wasmi"));
	let mut wasmi = WasmiHost::build(&bytes, Limits::default()).expect("build wasmi");
	for _ in 0..30 {
		std::hint::black_box(host_get_list(&mut wasmi));
	}
	let row = measure_flow(&mut wasmi, iters);
	print_stats("wasmi get_manga_list", &row.list);
	print_stats("wasmi get_manga_update", &row.detail);
	print_stats("wasmi get_page_list", &row.page);
	print_stats("wasmi flow (3 ops)", &row.flow);
	print_stats("wasmi cold build", &build_wasmi);

	compute_microbench();
}

fn host_get_list(host: &mut impl Host) -> aidoku::MangaPageResult {
	host.get_manga_list(&listing(), 1).expect("manga list")
}

/// 纯 guest 计算微基准（无 host 调用、无 ABI 编解码）：
/// 界定解释器相对 JIT 的最坏情况比值。每轮约 8 个 wasm 指令
fn compute_microbench() {
	const BURN_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "start"))
  (func (export "burn") (param i32) (result i32)
    (local $i i32)
    (block $done
      (loop $l
        (br_if $done (i32.ge_s (local.get $i) (local.get 0)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (local.get $i)))
"#;
	let bytes = wat::parse_str(BURN_WAT).expect("valid wat");
	println!();
	println!("compute microbench: burn(5_000_000) x 9 rounds, median of rounds");
	for (name, mut host) in [
		("wasmer", Box::new(WasmerHost::build(&bytes, Limits::default()).expect("build wasmer")) as Box<dyn Host>),
		("wasmi", Box::new(WasmiHost::build(&bytes, Limits::default()).expect("build wasmi")) as Box<dyn Host>),
	] {
		assert_eq!(host.call_raw("burn", &[1_000]).expect("warm burn"), 1_000);
		let mut samples = Vec::new();
		for _ in 0..9 {
			let t0 = Instant::now();
			assert_eq!(host.call_raw("burn", &[5_000_000]).expect("burn"), 5_000_000);
			samples.push(t0.elapsed().as_secs_f64() * 1e3);
		}
		let s = stats(samples);
		println!("{name:<28} {:>10.2} {:>10.2} {:>10.2}", s.median, s.p95, s.min);
		// fuel 计量单位差异：wasmi 按 IR 指令扣、wasmer 按 wasm operator 扣，
		// 同一 burn 调用两边的消耗读数直接给出换算比
		let consumed = host.fuel_budget() - host.fuel_remaining();
		println!("{name:<28} fuel consumed for burn(5_000_000): {consumed}");
	}
}
