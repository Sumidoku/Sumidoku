use crate::{WasmEnv, runtime::store::Ptr};
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

pub fn send_partial_result(env: FunctionEnvMut<WasmEnv>, value: i32) {
	if value < 0 {
		return;
	}
	let data = env.data();
	// 无在途流式调用时 sink 为空，部分结果按 ABI 约定丢弃
	let Some(sink) = &data.partial_sink else {
		return;
	};
	match data.read_item_bytes(&env, value as u32) {
		Ok(bytes) => sink(&bytes),
		Err(_) => data.log("error: failed to read partial result"),
	}
}
