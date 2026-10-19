//! memory.grow 语义对齐：生产 LimitedTunables（声明上限钳到 memory_max）对
//! wasmi StoreLimits。限额内可长、超帽失败、失败后实例照常可用
use wasmi_spike::wasmer_host::WasmerHost;
use wasmi_spike::wasmi_host::WasmiHost;
use wasmi_spike::{Host, Limits};

const GROW_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "start"))
  (func (export "grow") (param i32) (result i32)
    (memory.grow (local.get 0)))
  (func (export "size") (result i32)
    (memory.size)))
"#;

const ONE_MIB: u64 = 1024 * 1024;

fn grow_flow(host: &mut impl Host) {
	assert_eq!(host.call_raw("size", &[]).expect("size"), 1);
	// 帽内：+15 页到 16 页
	assert_eq!(host.call_raw("grow", &[15]).expect("grow 15"), 1);
	assert_eq!(host.call_raw("size", &[]).expect("size"), 16);
	// 超帽：memory_max 16 页，再要 1 页必须失败（ABI 返回 -1），实例不死
	assert_eq!(host.call_raw("grow", &[1]).expect("grow over cap"), -1);
	assert_eq!(host.call_raw("size", &[]).expect("size after failed grow"), 16);
	assert_eq!(host.call_raw("grow", &[0]).expect("grow 0 keeps working"), 16);
}

#[test]
fn memory_growth_over_cap_fails_under_wasmi() {
	let bytes = wat::parse_str(GROW_WAT).expect("valid wat");
	let limits = Limits {
		memory_max: ONE_MIB,
		..Limits::default()
	};
	let mut host = WasmiHost::build(&bytes, limits).expect("build wasmi host");
	grow_flow(&mut host);
}

#[test]
fn memory_growth_over_cap_fails_under_wasmer_baseline() {
	let bytes = wat::parse_str(GROW_WAT).expect("valid wat");
	let limits = Limits {
		memory_max: ONE_MIB,
		..Limits::default()
	};
	let mut host = WasmerHost::build(&bytes, limits).expect("build wasmer host");
	grow_flow(&mut host);
}
