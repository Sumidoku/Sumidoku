//! Boa 引擎封装（与上游 aidoku-rs 同引擎，纯 Rust 无交叉编译风险）：一个 ABI
//! context 对应一个独立 Context，context 间天然隔离。
//! 裸 Context 只有 ECMAScript 内置对象：无 console/fetch/setTimeout/loader，
//! JS 的宿主能力只有 wasm ABI 暴露的那些。
//!
//! 限制落点（Boa 0.21 无内存上限 API，硬内存顶不可落地，见 PR 讨论）：
//! - 脚本入口 wall-clock 超时：evaluate_async_with_budget 每 budget 条 VM 指令
//!   让出一次，poll 间隙查 deadline；超时即丢弃整个 Context 重建——被中断的 VM
//!   半帧状态没有公开 API 能恢复，继续用不安全
//! - promise reaction job（then 回调走同步 job.call，budget 够不着）里的死循环
//!   靠 RuntimeLimits 的 loop_iteration_limit 兜底：VM 在每次循环回边检查，
//!   与谁驱动无关

use boa_engine::{Context, JsString, JsValue, Script, Source};
use std::task::{Context as TaskContext, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

// 每次 deadline 检查之间让 VM 跑的指令配额：大了超时误差大，小了 poll 开销占比高
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

	// webview 的 document 替换语义要求丢弃全部全局状态重来，与超时重建同一条路径
	pub fn reset(&mut self) {
		self.context = Self::new_context(self.loop_max);
	}

	pub fn eval(&mut self, source: &str) -> Result<String, String> {
		let value = self.run_script(source)?;
		to_js_string(&mut self.context, &value)
	}

	// 与 AidokuRunner(iOS) 的 evaluateAsyncScript 对齐：脚本结果若是 Promise 就驱动
	// job 队列等它 settle 取 resolve 值，否则直接取原值；拒绝算失败。job 队列排空后
	// 仍 Pending 的 Promise 永远不会再推进（引擎没注册任何 host 异步源），立即失败
	// 而不是空转等超时
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
