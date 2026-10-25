use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Limits {
	/// Instruction budget (wasmi fuel) for a single export call: reset before each call,
	/// traps when exhausted. 10B would kill pathological sources like godamanga on long
	/// chapter lists (thousands of chapters x a regex compiled per chapter — measured 3875
	/// chapters of pure compute at ~7s ≈ 50B); the wall-clock call_timeout is the main
	/// backstop, fuel only guards against instruction-level runaway within the timeout, so 100B
	pub fuel: u64,
	/// Per-call wall-clock timeout; on timeout the circuit breaker restarts the instance
	pub call_timeout: Duration,
	/// Cap for a single env.sleep
	pub sleep_max: Duration,
	pub net_timeout_default: Duration,
	pub net_timeout_max: Duration,
	/// net host cap on a single response body (decoded bytes); 0 = unlimited
	pub net_body_max: u64,
	/// Instance linear-memory cap (StoreLimits); memory.grow beyond it fails
	pub memory_max: u64,
	/// Execution cap for a single js eval (wall-clock at the script entry, checked at budget yield points)
	pub js_timeout: Duration,
	/// js single-loop iteration cap: the last backstop for infinite loops inside promise reaction jobs (VM back-edge check)
	pub js_loop_max: u64,
	/// webview browser navigation timeout; must be below call_timeout, otherwise the circuit breaker fires before the timeout
	pub browser_timeout: Duration,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			fuel: 100_000_000_000,
			call_timeout: Duration::from_secs(60),
			sleep_max: Duration::from_secs(10),
			net_timeout_default: Duration::from_secs(30),
			net_timeout_max: Duration::from_secs(120),
			// The largest legitimate single body in the ecosystem is a full-page long-strip manga image (commonly a few MiB, ~20 MiB at the extreme); 32 MiB leaves headroom
			net_body_max: 32 * 1024 * 1024,
			// Measured Aidoku sources rarely exceed 64 MiB; setting it too low would make memory.grow fail and kill sources outright
			memory_max: 128 * 1024 * 1024,
			js_timeout: Duration::from_secs(10),
			js_loop_max: 1_000_000_000,
			browser_timeout: Duration::from_secs(30),
		}
	}
}
