use crate::runtime::{env::Ctx, store::Ptr};

pub fn abort(ctx: &mut impl Ctx) {
	ctx.host().log("error: abort called");
}

pub fn print(ctx: &mut impl Ctx, ptr: Ptr, len: u32) {
	match ctx.read_string(ptr, len) {
		Ok(s) => ctx.host().log(&s),
		Err(_) => ctx.host().log("error: failed to read string for printing"),
	}
}

pub fn sleep(ctx: &mut impl Ctx, seconds: i32) {
	let cap = ctx.host().limits.sleep_max;
	let duration = std::time::Duration::from_secs(seconds.max(0) as u64).min(cap);
	std::thread::sleep(duration);
}

pub fn send_partial_result(ctx: &mut impl Ctx, value: i32) {
	if value < 0 {
		return;
	}
	// With no in-flight streaming call the sink is empty, and partial results are dropped per the ABI contract
	if ctx.host().partial_sink.is_none() {
		return;
	}
	match ctx.read_item_bytes(value as u32) {
		Ok(bytes) => {
			if let Some(sink) = &ctx.host().partial_sink {
				sink(&bytes);
			}
		}
		Err(_) => ctx.host().log("error: failed to read partial result"),
	}
}
