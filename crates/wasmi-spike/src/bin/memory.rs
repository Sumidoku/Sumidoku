//! per-instance RSS 测量（issue #169 scope 3）：同一 listing fixture。
//! 每种形态一个独立进程（`--engine/--shape/--mode` 选择，外层循环驱动），
//! 避免跨形态复用分配器缓存污染增量。冷实例 = 实例化 + start；
//! 暖实例 = 再跑一轮 get_manga_list。实例全部存活取平均

use wasmi_spike::wasmer_host::{WasmerHost, WasmerPrepared};
use wasmi_spike::wasmi_host::{WasmiHost, WasmiPrepared};
use wasmi_spike::{HostExt, Limits, fixture_wasm};

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

fn kib(bytes: u64) -> f64 {
	bytes as f64 / 1024.0
}

fn warm_up(host: &mut impl wasmi_spike::Host) {
	let listing = aidoku::Listing {
		id: "hot".into(),
		name: "Hot Updates".into(),
		kind: aidoku::ListingKind::List,
	};
	let result = host.get_manga_list(&listing, 1).expect("warm call");
	assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
}

fn print_row(engine: &str, shape: &str, module_kib: Option<f64>, before: u64, cold: u64, warm: u64, n: usize) {
	let n = n as f64;
	let module = module_kib
		.map(|k| format!("{k:.1}"))
		.unwrap_or_else(|| "(amortized)".to_string());
	println!(
		"{engine:<8} {shape:<22} {module:>14} {:>14.1} {:>14.1}   (totals: cold {:.1} MiB, warm +{:.1} MiB)",
		kib(cold.saturating_sub(before)) / n,
		kib(warm.saturating_sub(cold)) / n,
		kib(cold.saturating_sub(before)) / 1024.0,
		kib(warm.saturating_sub(cold)) / 1024.0,
	);
}

fn main() {
	let mut engine = String::from("wasmi");
	let mut shape = String::from("shared");
	let mut mode = String::from("lazy-translation");
	let mut n: usize = 48;
	let mut args = std::env::args().skip(1);
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"--engine" => engine = args.next().expect("--engine value"),
			"--shape" => shape = args.next().expect("--shape value"),
			"--mode" => mode = args.next().expect("--mode value"),
			"--instances" => {
				n = args.next().expect("--instances value").parse().expect("usize")
			}
			other => panic!("unknown arg {other}"),
		}
	}
	let bytes = fixture_wasm();
	println!(
		"platform: {} {}, engine: {engine}, shape: {shape}, mode: {mode}, instances: {n}",
		std::env::consts::OS,
		std::env::consts::ARCH
	);
	println!(
		"{:<8} {:<22} {:>14} {:>14} {:>14}",
		"engine", "shape", "module KiB", "cold KiB/inst", "warm +KiB/inst"
	);
	match (engine.as_str(), shape.as_str()) {
		("wasmer", "shared") => {
			// Metering middleware 一个实例只能编译一个模块：热身走独立完整构建
			drop(WasmerHost::build(&bytes, Limits::default()).expect("warmup"));
			let prepared = WasmerPrepared::new(Limits::default());
			let base = current_rss();
			let module = prepared.compile(&bytes).expect("compile");
			let module_kib = kib(current_rss().saturating_sub(base));
			let before = current_rss();
			let mut hosts: Vec<WasmerHost> = (0..n)
				.map(|_| prepared.instantiate(&module).expect("instantiate"))
				.collect();
			let cold = current_rss();
			for host in &mut hosts {
				warm_up(host);
			}
			print_row("wasmer", "shared-engine", Some(module_kib), before, cold, current_rss(), n);
		}
		("wasmer", "per-instance") => {
			drop(WasmerHost::build(&bytes, Limits::default()).expect("warmup"));
			let before = current_rss();
			let mut hosts: Vec<WasmerHost> = (0..n)
				.map(|_| WasmerHost::build(&bytes, Limits::default()).expect("build"))
				.collect();
			let cold = current_rss();
			for host in &mut hosts {
				warm_up(host);
			}
			print_row("wasmer", "per-instance-engine", None, before, cold, current_rss(), n);
		}
		("wasmi", "shared") => {
			let compilation = match mode.as_str() {
				"eager" => wasmi::CompilationMode::Eager,
				_ => wasmi::CompilationMode::LazyTranslation,
			};
			let prepared = WasmiPrepared::with_mode(Limits::default(), compilation);
			drop(prepared.instantiate(&prepared.translate(&bytes).expect("translate")).expect("warmup"));
			let base = current_rss();
			let module = prepared.translate(&bytes).expect("translate");
			let module_kib = kib(current_rss().saturating_sub(base));
			let before = current_rss();
			let mut hosts: Vec<WasmiHost> = (0..n)
				.map(|_| prepared.instantiate(&module).expect("instantiate"))
				.collect();
			let cold = current_rss();
			for host in &mut hosts {
				warm_up(host);
			}
			print_row("wasmi", "shared-engine", Some(module_kib), before, cold, current_rss(), n);
		}
		("wasmi", "per-instance") => {
			drop(WasmiHost::build(&bytes, Limits::default()).expect("warmup"));
			let before = current_rss();
			let mut hosts: Vec<WasmiHost> = (0..n)
				.map(|_| WasmiHost::build(&bytes, Limits::default()).expect("build"))
				.collect();
			let cold = current_rss();
			for host in &mut hosts {
				warm_up(host);
			}
			print_row("wasmi", "per-instance-engine", None, before, cold, current_rss(), n);
		}
		(e, s) => panic!("unknown engine/shape {e}/{s}"),
	}
}
