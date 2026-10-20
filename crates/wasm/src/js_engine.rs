//! Boa engine wrapper (same engine as upstream aidoku-rs, pure Rust, no cross-compile
//! risk): one ABI context maps to one independent Context, so contexts are naturally
//! isolated. A bare Context only has ECMAScript built-ins: no console/fetch/setTimeout/
//! loader — JS's host capabilities are only the ones the wasm ABI exposes.
//!
//! Where limits land (Boa 0.21 has no memory-cap API, so a hard memory ceiling cannot be
//! implemented — see the PR discussion):
//! - Wall-clock timeout at the script entry: evaluate_async_with_budget yields once every
//!   budget VM instructions and checks the deadline between polls; on timeout the whole
//!   Context is dropped and rebuilt — no public API can recover the half-frame state of an
//!   interrupted VM, so continuing to use it is unsafe
//! - Infinite loops inside promise reaction jobs (then callbacks run via synchronous
//!   job.call, which the budget cannot reach) are backstopped by RuntimeLimits'
//!   loop_iteration_limit: the VM checks on every loop back-edge, independent of who drives

use boa_engine::{Context, JsString, JsValue, Script, Source};
use std::task::{Context as TaskContext, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

// Instruction quota the VM runs between deadline checks: too big and the timeout error grows, too small and the poll overhead dominates
const YIELD_BUDGET: u32 = 8192;

pub struct JsEngine {
	context: Context,
	timeout: Duration,
	loop_max: u64,
}

impl JsEngine {
	pub fn new(timeout: Duration, loop_max: u64) -> Self {
		Self {
			context: Self::new_context(loop_max),
			timeout,
			loop_max,
		}
	}

	fn new_context(loop_max: u64) -> Context {
		let mut context = Context::default();
		context.runtime_limits_mut().set_loop_iteration_limit(loop_max);
		context
	}

	// webview's document-replacement semantics require dropping all global state and starting over — same path as the timeout rebuild
	pub fn reset(&mut self) {
		self.context = Self::new_context(self.loop_max);
	}

	pub fn eval(&mut self, source: &str) -> Result<String, String> {
		let value = self.run_script(source)?;
		to_js_string(&mut self.context, &value)
	}

	// Aligned with AidokuRunner(iOS)'s evaluateAsyncScript: if the script result is a
	// Promise, drive the job queue until it settles and take the resolve value, otherwise
	// take the raw value; rejection counts as failure. A Promise still Pending after the
	// job queue drains will never advance again (the engine has no host async sources
	// registered), so fail immediately instead of idling until the timeout
	pub fn eval_async(&mut self, source: &str) -> Result<String, String> {
		let value = self.run_script(source)?;
		let Some(promise) = value.as_promise() else {
			return to_js_string(&mut self.context, &value);
		};
		let deadline = Instant::now() + self.timeout;
		loop {
			use boa_engine::builtins::promise::PromiseState;
			match promise.state() {
				PromiseState::Fulfilled(value) => return to_js_string(&mut self.context, &value),
				PromiseState::Rejected(reason) => {
					return Err(format!("promise rejected: {}", reason.display()));
				}
				PromiseState::Pending => {
					if Instant::now() >= deadline {
						return Err("js execution timed out".into());
					}
					if let Err(e) = self.context.run_jobs() {
						return Err(e.to_string());
					}
					if matches!(promise.state(), PromiseState::Pending) {
						return Err("promise never settles".into());
					}
				}
			}
		}
	}

	pub fn get(&mut self, name: &str) -> Result<String, String> {
		let value = self
			.context
			.global_object()
			.get(JsString::from(name), &mut self.context)
			.map_err(|e| e.to_string())?;
		to_js_string(&mut self.context, &value)
	}

	fn run_script(&mut self, source: &str) -> Result<JsValue, String> {
		let script = Script::parse(Source::from_bytes(source), None, &mut self.context)
			.map_err(|e| e.to_string())?;
		let deadline = Instant::now() + self.timeout;
		let waker = noop_waker();
		let mut task_cx = TaskContext::from_waker(&waker);
		let mut fut = Box::pin(script.evaluate_async_with_budget(&mut self.context, YIELD_BUDGET));
		loop {
			if Instant::now() >= deadline {
				drop(fut);
				self.context = Self::new_context(self.loop_max);
				return Err("js execution timed out".into());
			}
			match fut.as_mut().poll(&mut task_cx) {
				Poll::Ready(Ok(value)) => return Ok(value),
				Poll::Ready(Err(e)) => return Err(e.to_string()),
				Poll::Pending => {}
			}
		}
	}
}

fn to_js_string(context: &mut Context, value: &JsValue) -> Result<String, String> {
	value
		.to_string(context)
		.map_err(|e| e.to_string())?
		.to_std_string()
		.map_err(|e| e.to_string())
}

fn noop_waker() -> Waker {
	unsafe fn clone(_: *const ()) -> RawWaker {
		RawWaker::new(std::ptr::null(), &VTABLE)
	}
	unsafe fn noop(_: *const ()) {}
	static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
	unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}
