//! QuickJS 引擎封装：一个 ABI context 对应一套独立的 Runtime+Context，context 间天然隔离。
//! 源是不可信代码，而 wasm 侧的 fuel/call_timeout 管不到宿主函数内部的 JS 执行，
//! 限制必须落在引擎层：内存上限挂在 Runtime 上，执行超时走 interrupt handler。
//! 不注册任何 loader 与全局函数，JS 的宿主能力只有 wasm ABI 暴露的那些。

use rquickjs::{Context, Coerced, Ctx, Error, FromJs, Promise, Runtime, Value};
use std::time::{Duration, Instant};

pub struct JsEngine {
	context: Context,
	timeout: Duration,
}

impl JsEngine {
	pub fn new(memory_limit: usize, timeout: Duration) -> Result<Self, String> {
		let runtime = Runtime::new().map_err(|e| e.to_string())?;
		runtime.set_memory_limit(memory_limit);
		Context::full(&runtime)
			.map(|context| Self { context, timeout })
			.map_err(|e| e.to_string())
	}

	pub fn eval(&mut self, source: &str) -> Result<String, String> {
		self.run(|ctx| {
			let value: Value = ctx.eval(source).map_err(|e| catch_msg(&ctx, e))?;
			to_js_string(&ctx, value)
		})
	}

	// 与 AidokuRunner(iOS) 的 evaluateAsyncScript 对齐：脚本结果若是 Promise 就驱动
	// job 队列等它 settle 取 resolve 值，否则直接取原值；拒绝、或 job 队列排空仍不
	// settle（finish 的 WouldBlock）都算失败
	pub fn eval_async(&mut self, source: &str) -> Result<String, String> {
		self.run(|ctx| {
			let value: Value = ctx.eval(source).map_err(|e| catch_msg(&ctx, e))?;
			let value = if value.is_promise() {
				Promise::from_js(&ctx, value)
					.map_err(|e| catch_msg(&ctx, e))?
					.finish()
					.map_err(|e| catch_msg(&ctx, e))?
			} else {
				value
			};
			to_js_string(&ctx, value)
		})
	}

	pub fn get(&mut self, name: &str) -> Result<String, String> {
		self.run(|ctx| {
			let value: Value = ctx.globals().get(name).map_err(|e| catch_msg(&ctx, e))?;
			to_js_string(&ctx, value)
		})
	}

	fn run(
		&mut self,
		f: impl for<'js> FnOnce(Ctx<'js>) -> Result<String, String>,
	) -> Result<String, String> {
		let deadline = Instant::now() + self.timeout;
		self.context
			.runtime()
			.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
		let result = self.context.with(f);
		// handler 捕获的 deadline 过期后会永久触发，必须摘掉，否则后续 eval 全灭
		self.context.runtime().set_interrupt_handler(None);
		result
	}
}

fn to_js_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String, String> {
	Coerced::<String>::from_js(ctx, value)
		.map(|s| s.0)
		.map_err(|e| catch_msg(ctx, e))
}

// Error::Exception 本身不带 JS 侧消息，从 context 里把异常值捞出来转成字符串
fn catch_msg<'js>(ctx: &Ctx<'js>, e: Error) -> String {
	match e {
		Error::Exception => {
			Coerced::<String>::from_js(ctx, ctx.catch())
				.map(|s| s.0)
				.unwrap_or_else(|_| "unknown js exception".into())
		}
		other => other.to_string(),
	}
}
