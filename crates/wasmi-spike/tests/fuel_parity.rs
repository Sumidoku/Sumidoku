//! fuel 等价性：wasmi `Store::set_fuel`（每导出调用前重置）对生产
//! `Metering + set_remaining_points` 的语义映射。验证面：
//! 预算内结果不受影响、花光即 trap（含穿越 host 调用）、逐调用重置不累计、
//! trap 后实例可复用。两引擎跑同一批 WAT guest
use wasmi_spike::wasmer_host::WasmerHost;
use wasmi_spike::wasmi_host::WasmiHost;
use wasmi_spike::{CallError, Host, Limits};

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
    (local.get $i))
  (func (export "spin") (loop (br 0))))
"#;

const SPIN_HOST_WAT: &str = r#"
(module
  (import "std" "current_date" (func $now (result f64)))
  (memory (export "memory") 1)
  (func (export "start"))
  (func (export "spin_host")
    (loop
      (drop (call $now))
      (br 0))))
"#;

fn build_pair(wat_text: &str, fuel: u64) -> (WasmerHost, WasmiHost) {
	let bytes = wat::parse_str(wat_text).expect("valid wat");
	let limits = Limits {
		fuel,
		..Limits::default()
	};
	let wasmer = WasmerHost::build(&bytes, limits.clone()).expect("build wasmer");
	let wasmi = WasmiHost::build(&bytes, limits).expect("build wasmi");
	(wasmer, wasmi)
}

fn consumed(host: &mut (impl Host + ?Sized)) -> u64 {
	let budget = host.fuel_budget();
	budget - host.fuel_remaining()
}

#[test]
fn within_budget_result_is_unaffected() {
	let (mut wasmer, mut wasmi) = build_pair(BURN_WAT, 10_000_000);
	assert_eq!(wasmer.call_raw("burn", &[1000]).expect("wasmer burn"), 1000);
	assert_eq!(wasmi.call_raw("burn", &[1000]).expect("wasmi burn"), 1000);
}

#[test]
fn budget_exhaustion_traps_on_both_engines() {
	let (mut wasmer, mut wasmi) = build_pair(BURN_WAT, 10_000_000);
	for host in [&mut wasmer as &mut dyn Host, &mut wasmi] {
		assert_eq!(host.call_raw("burn", &[20000]).expect("burn"), 20000);
		let cost = consumed(host);
		assert!(cost > 0, "fuel metering must be live");
		// 预算砍到实际消耗的一半：必须 trap 且归类为 out of fuel
		host.set_fuel_budget(cost / 2);
		let err = host.call_raw("burn", &[20000]).expect_err("half budget must trap");
		assert!(matches!(err, CallError::OutOfFuel), "expected OutOfFuel, got {err}");
	}
}

#[test]
fn infinite_loop_traps_out_of_fuel() {
	let (mut wasmer, mut wasmi) = build_pair(BURN_WAT, 1_000_000);
	for host in [&mut wasmer as &mut dyn Host, &mut wasmi] {
		let err = host.call_raw("spin", &[]).expect_err("spin must trap");
		assert!(matches!(err, CallError::OutOfFuel), "expected OutOfFuel, got {err}");
	}
}

#[test]
fn budget_resets_per_call_and_instance_survives_trap() {
	let (mut wasmer, mut wasmi) = build_pair(BURN_WAT, 10_000_000);
	for host in [&mut wasmer as &mut dyn Host, &mut wasmi] {
		assert_eq!(host.call_raw("burn", &[20000]).expect("burn"), 20000);
		let cost = consumed(host);
		host.set_fuel_budget(cost + cost / 2);
		// 1.5 倍单次预算：不逐调用重置的话第二次必然累计爆掉
		for round in 0..3 {
			assert_eq!(
				host.call_raw("burn", &[20000]).unwrap_or_else(|e| panic!("round {round}: {e}")),
				20000
			);
		}
		// trap 之后实例不废：重置预算照常跑
		host.set_fuel_budget(cost / 2);
		assert!(matches!(host.call_raw("burn", &[20000]), Err(CallError::OutOfFuel)));
		host.set_fuel_budget(cost + cost / 2);
		assert_eq!(host.call_raw("burn", &[20000]).expect("reused after trap"), 20000);
	}
}

#[test]
fn fuel_exhaustion_traps_across_host_calls() {
	let (mut wasmer, mut wasmi) = build_pair(SPIN_HOST_WAT, 1_000_000);
	for host in [&mut wasmer as &mut dyn Host, &mut wasmi] {
		let err = host.call_raw("spin_host", &[]).expect_err("host-call loop must trap");
		assert!(matches!(err, CallError::OutOfFuel), "expected OutOfFuel, got {err}");
	}
	// trap 穿越 host 调用后宿主状态不坏：换 BURN guest 的实例继续可用
	let (mut wasmer, mut wasmi) = build_pair(BURN_WAT, 10_000_000);
	assert_eq!(wasmer.call_raw("burn", &[10]).expect("wasmer still fine"), 10);
	assert_eq!(wasmi.call_raw("burn", &[10]).expect("wasmi still fine"), 10);
}
