use crate::{WasmEnv, store::Ptr};
use wasmer::FunctionEnvMut;

pub fn abort(env: FunctionEnvMut<WasmEnv>) {
	env.data().log("error: abort called");
}

pub fn print(env: FunctionEnvMut<WasmEnv>, ptr: Ptr, len: u32) {
	match env.data().read_string(&env, ptr, len) {
		Ok(s) => env.data().log(&s),
		Err(_) => env.data().log("error: failed to read string for printing"),
	}
}

pub fn sleep(env: FunctionEnvMut<WasmEnv>, seconds: i32) {
	let cap = env.data().limits.sleep_max;
	let duration = std::time::Duration::from_secs(seconds.max(0) as u64).min(cap);
	std::thread::sleep(duration);
}

pub fn send_partial_result(_env: FunctionEnvMut<WasmEnv>, _value: i32) {
	// 部分结果流式加载是 M4 home 交互能力，M1 忽略
}
