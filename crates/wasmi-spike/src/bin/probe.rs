//! wasmi 暖实例增量分解（排障用探针，非交付物）：实例化 → free_result(0)
//! （最小翻译路径）→ get_manga_list（完整流程）三段读 RSS，
//! 拆出「首调翻译」与「流程数据触碰」各占多少

use wasmi_spike::wasmi_host::WasmiPrepared;
use wasmi_spike::{Host, HostExt, Limits, fixture_wasm};

#[cfg(target_os = "macos")]
fn current_rss() -> u64 {
	unsafe {
		let mut info: libc::mach_task_basic_info = std::mem::zeroed();
		let mut count = (size_of::<libc::mach_task_basic_info>() / size_of::<libc::integer_t>())
			as libc::mach_msg_type_number_t;
		let kr = libc::task_info(
			// libc 把 mach_task_self 标 deprecated 推 mach2；spike bin 不为一个符号加依赖
			#[allow(deprecated)]
			libc::mach_task_self(),
			libc::MACH_TASK_BASIC_INFO,
			&mut info as *mut _ as libc::task_info_t,
			&mut count,
		);
		if kr == libc::KERN_SUCCESS { info.resident_size } else { 0 }
	}
}

#[cfg(target_os = "linux")]
fn current_rss() -> u64 {
	let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
	for line in status.lines() {
		if let Some(rest) = line.strip_prefix("VmRSS:") {
			let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
			return kb * 1024;
		}
	}
	0
}

fn main() {
	let mode = std::env::args().nth(1).unwrap_or_else(|| "lazy".to_string());
	let pages: u32 = std::env::args()
		.nth(2)
		.and_then(|a| a.parse().ok())
		.unwrap_or(0);
	let compilation = match mode.as_str() {
		"eager" => wasmi::CompilationMode::Eager,
		_ => wasmi::CompilationMode::LazyTranslation,
	};
	let bytes = if pages > 0 {
		wat::parse_str(format!(
			"(module (memory (export \"memory\") {pages}) (func (export \"start\")) (func (export \"free_result\") (param i32)))"
		))
		.expect("valid wat")
	} else if mode == "hostcall" {
		wat::parse_str(
			r#"(module
  (import "std" "current_date" (func $now (result f64)))
  (memory (export "memory") 17)
  (func (export "start"))
  (func (export "free_result") (param i32))
  (func (export "hostcall") (param i32) (result f64)
    (local $acc f64)
    (block $done
      (loop $l
        (br_if $done (i32.eqz (local.get 0)))
        (local.set $acc (f64.add (local.get $acc) (call $now)))
        (local.set 0 (i32.sub (local.get 0) (i32.const 1)))
        (br $l)))
    (local.get $acc)))"#,
		)
		.expect("valid wat")
	} else {
		fixture_wasm()
	};
	let prepared = {
		let mut config = wasmi::Config::default();
		config.consume_fuel(true);
		config.compilation_mode(compilation);
		if let Ok(v) = std::env::var("WASMI_MAX_CACHED_STACKS") {
			config.set_max_cached_stacks(v.parse().expect("usize"));
		}
		if let Ok(v) = std::env::var("WASMI_MIN_STACK_HEIGHT") {
			config.set_min_stack_height(v.parse().expect("usize"));
		}
		WasmiPrepared::from_engine(wasmi::Engine::new(&config), Limits::default())
	};
	let module = prepared.translate(&bytes).expect("translate");
	let kib = |b: u64| b as f64 / 1024.0;

	let base = current_rss();
	let mut host = prepared.instantiate(&module).expect("instantiate");
	let rss_inst = current_rss();
	host.call_raw("free_result", &[0]).expect("free_result(0)");
	let rss_free = current_rss();
	if pages > 0 {
		println!("mode: {mode}, tiny module with {pages} initial pages");
		println!("instance (incl. start):       +{:>8.1} KiB", kib(rss_inst - base));
		println!("free_result(0):               +{:>8.1} KiB", kib(rss_free - rss_inst));
		return;
	}
	if mode == "hostcall" {
		println!("hostcall module (17 pages, current_date loop)");
		println!("instance (incl. start):       +{:>8.1} KiB", kib(rss_inst - base));
		println!("free_result(0):               +{:>8.1} KiB", kib(rss_free - rss_inst));
		let acc = host.call_raw("hostcall", &[100]).expect("hostcall");
		println!("first hostcall(100):          +{:>8.1} KiB (acc {acc})", kib(current_rss() - rss_free));
		let rss_one = current_rss();
		host.call_raw("hostcall", &[100]).expect("hostcall 2");
		println!("second hostcall(100):         +{:>8.1} KiB", kib(current_rss() - rss_one));
		return;
	}
	let manga = aidoku::Manga {
		key: "echo".into(),
		..Default::default()
	};
	let chapter = aidoku::Chapter {
		key: "c1".into(),
		..Default::default()
	};
	let pages_out = host.get_page_list(&manga, &chapter).expect("page list");
	assert!(pages_out.is_empty());
	let rss_page = current_rss();
	println!("get_page_list first:          +{:>8.1} KiB", kib(rss_page - rss_free));
	let listing = aidoku::Listing {
		id: "hot".into(),
		name: "Hot Updates".into(),
		kind: aidoku::ListingKind::List,
	};
	let result = host.get_manga_list(&listing, 1).expect("manga list");
	assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
	let rss_first = current_rss();
	let result = host.get_manga_list(&listing, 2).expect("manga list 2");
	assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
	let rss_second = current_rss();

	println!("mode: {mode}");
	println!("instance (incl. start):       +{:>8.1} KiB", kib(rss_inst - base));
	println!("free_result(0):               +{:>8.1} KiB", kib(rss_free - rss_inst));
	println!("first get_manga_list:         +{:>8.1} KiB", kib(rss_first - rss_free));
	println!("second get_manga_list:        +{:>8.1} KiB", kib(rss_second - rss_first));
	println!("total:                        +{:>8.1} KiB", kib(rss_second - base));
}
