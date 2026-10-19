//! wasmi 宿主：与 wasmer 基线共用全部 import 函数体，这里只有引擎接线。
//! fuel 映射：wasmi 按翻译出的 IR 指令逐基本块扣费（默认每指令 1 单位），
//! 与 wasmer metering（每 wasm operator 1 单位、块级扣费）语义同类——
//! 预算在每次导出调用前重置、耗尽即 trap、host 调用期间不扣。

use crate::host::{Ctx, HostState};
use crate::{CallError, Host as _, Limits};
use wasmi::{Caller, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode, Val};

struct WasmiData {
	host: HostState,
	limiter: StoreLimits,
}

/// Caller 的内存访问与宿主状态访问分时复用同一 &mut，Ctx 每次调用重借
struct WasmiCtx<'a, 'b> {
	caller: &'a mut Caller<'b, WasmiData>,
	memory: Memory,
}

impl Ctx for WasmiCtx<'_, '_> {
	fn host(&mut self) -> &mut HostState {
		&mut self.caller.data_mut().host
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
		let data = self.memory.data(&*self.caller);
		let end = ptr.checked_add(len)? as usize;
		Some(data.get(ptr as usize..end)?.to_vec())
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
		let Some(end) = ptr.checked_add(bytes.len() as u32) else {
			return false;
		};
		let data = self.memory.data_mut(&mut *self.caller);
		let Some(slot) = data.get_mut(ptr as usize..end as usize) else {
			return false;
		};
		slot.copy_from_slice(bytes);
		true
	}
}

/// host fn 适配宏：取 memory 句柄后包 Ctx 调共享函数体。
/// memory 必须在函数体内现取——实例化完成后才存在，没法闭包捕获
macro_rules! wrap {
	($caller:expr) => {{
		let memory = $caller
			.get_export("memory")
			.and_then(wasmi::Extern::into_memory)
			.expect("source exports memory");
		WasmiCtx {
			caller: &mut $caller,
			memory,
		}
	}};
}

pub struct WasmiHost {
	store: Store<WasmiData>,
	instance: Instance,
	memory: Memory,
	limits: Limits,
}

impl WasmiHost {
	pub fn build(wasm_bytes: &[u8], limits: Limits) -> Result<Self, CallError> {
		let prepared = WasmiPrepared::new(limits);
		let module = prepared.translate(wasm_bytes)?;
		prepared.instantiate(&module)
	}
}

/// 引擎 + 翻译产物与实例化分离：内存/冷启动测量要区分模块成本与每实例成本
pub struct WasmiPrepared {
	engine: Engine,
	limits: Limits,
}

impl WasmiPrepared {
	pub fn new(limits: Limits) -> Self {
		Self::with_mode(limits, wasmi::CompilationMode::default())
	}

	pub fn with_mode(limits: Limits, mode: wasmi::CompilationMode) -> Self {
		let mut config = wasmi::Config::default();
		config.consume_fuel(true);
		config.compilation_mode(mode);
		Self {
			engine: Engine::new(&config),
			limits,
		}
	}

	pub fn from_engine(engine: Engine, limits: Limits) -> Self {
		Self { engine, limits }
	}

	pub fn translate(&self, wasm_bytes: &[u8]) -> Result<Module, CallError> {
		Module::new(&self.engine, wasm_bytes).map_err(|e| CallError::Instance(format!("translate module: {e}")))
	}

	pub fn instantiate(&self, module: &Module) -> Result<WasmiHost, CallError> {
		let limits = self.limits.clone();
		let memory_max = usize::try_from(limits.memory_max).expect("memory_max fits usize");
		let data = WasmiData {
			host: HostState::new(limits.clone()),
			limiter: StoreLimitsBuilder::new().memory_size(memory_max).build(),
		};
		let mut store = Store::new(&self.engine, data);
		store
			.set_fuel(limits.fuel)
			.map_err(|e| CallError::Instance(format!("set fuel: {e}")))?;
		store.limiter(|data| &mut data.limiter);

		let mut linker = Linker::new(&self.engine);
		linker
			.func_wrap("env", "abort", |mut caller: Caller<'_, WasmiData>| {
				crate::host::env::abort(&mut wrap!(caller));
			})
			.and_then(|l| {
				l.func_wrap("env", "print", |mut caller: Caller<'_, WasmiData>, ptr: u32, len: u32| {
					crate::host::env::print(&mut wrap!(caller), ptr, len);
				})
			})
			.and_then(|l| {
				l.func_wrap("env", "sleep", |mut caller: Caller<'_, WasmiData>, seconds: i32| {
					crate::host::env::sleep(&mut wrap!(caller), seconds);
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"env",
					"send_partial_result",
					|mut caller: Caller<'_, WasmiData>, value: i32| {
						crate::host::env::send_partial_result(&mut wrap!(caller), value);
					},
				)
			})
			.and_then(|l| {
				l.func_wrap("std", "destroy", |mut caller: Caller<'_, WasmiData>, rid: i32| {
					crate::host::std::destroy(&mut wrap!(caller), rid);
				})
			})
			.and_then(|l| {
				l.func_wrap("std", "buffer_len", |mut caller: Caller<'_, WasmiData>, rid: i32| -> i32 {
					crate::host::std::buffer_len(&mut wrap!(caller), rid)
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"std",
					"read_buffer",
					|mut caller: Caller<'_, WasmiData>, rid: i32, ptr: u32, size: u32| -> i32 {
						crate::host::std::read_buffer(&mut wrap!(caller), rid, ptr, size)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap("std", "current_date", |mut caller: Caller<'_, WasmiData>| -> f64 {
					crate::host::std::current_date(&mut wrap!(caller))
				})
			})
			.and_then(|l| {
				l.func_wrap("std", "utc_offset", |mut caller: Caller<'_, WasmiData>| -> i64 {
					crate::host::std::utc_offset(&mut wrap!(caller))
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"std",
					"parse_date",
					|mut caller: Caller<'_, WasmiData>, a: u32, b: u32, c: u32, d: u32, e: u32, f: u32, g: u32, h: u32| -> f64 {
						crate::host::std::parse_date(&mut wrap!(caller), a, b, c, d, e, f, g, h)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap("net", "init", |mut caller: Caller<'_, WasmiData>, method: i32| -> i32 {
					crate::host::net::init(&mut wrap!(caller), method)
				})
			})
			.and_then(|l| {
				l.func_wrap("net", "send", |mut caller: Caller<'_, WasmiData>, rid: i32| -> i32 {
					crate::host::net::send(&mut wrap!(caller), rid)
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"send_all",
					|mut caller: Caller<'_, WasmiData>, ptr: u32, len: u32| -> i32 {
						crate::host::net::send_all(&mut wrap!(caller), ptr, len)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"set_url",
					|mut caller: Caller<'_, WasmiData>, rid: i32, ptr: u32, len: u32| -> i32 {
						crate::host::net::set_url(&mut wrap!(caller), rid, ptr, len)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"set_header",
					|mut caller: Caller<'_, WasmiData>, rid: i32, kp: u32, kl: u32, vp: u32, vl: u32| -> i32 {
						crate::host::net::set_header(&mut wrap!(caller), rid, kp, kl, vp, vl)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"set_body",
					|mut caller: Caller<'_, WasmiData>, rid: i32, ptr: u32, len: u32| -> i32 {
						crate::host::net::set_body(&mut wrap!(caller), rid, ptr, len)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"set_timeout",
					|mut caller: Caller<'_, WasmiData>, rid: i32, value: f64| -> i32 {
						crate::host::net::set_timeout(&mut wrap!(caller), rid, value)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap("net", "data_len", |mut caller: Caller<'_, WasmiData>, rid: i32| -> i32 {
					crate::host::net::data_len(&mut wrap!(caller), rid)
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"read_data",
					|mut caller: Caller<'_, WasmiData>, rid: i32, buffer: u32, size: u32| -> i32 {
						crate::host::net::read_data(&mut wrap!(caller), rid, buffer, size)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"get_status_code",
					|mut caller: Caller<'_, WasmiData>, rid: i32| -> i32 {
						crate::host::net::get_status_code(&mut wrap!(caller), rid)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap("net", "get_url", |mut caller: Caller<'_, WasmiData>, rid: i32| -> i32 {
					crate::host::net::get_url(&mut wrap!(caller), rid)
				})
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"get_header",
					|mut caller: Caller<'_, WasmiData>, rid: i32, kp: u32, kl: u32| -> i32 {
						crate::host::net::get_header(&mut wrap!(caller), rid, kp, kl)
					},
				)
			})
			.and_then(|l| {
				l.func_wrap(
					"net",
					"set_rate_limit",
					|mut caller: Caller<'_, WasmiData>, permits: i32, period: i32, unit: i32| {
						crate::host::net::set_rate_limit(&mut wrap!(caller), permits, period, unit);
					},
				)
			})
			.map_err(|e| CallError::Instance(format!("link imports: {e}")))?;

		let instance = linker
			.instantiate_and_start(&mut store, module)
			.map_err(|e| CallError::Instance(format!("instantiate: {e}")))?;
		let memory = instance
			.get_memory(&store, "memory")
			.ok_or_else(|| CallError::Instance("source exports no memory".into()))?;
		let mut host = WasmiHost {
			store,
			instance,
			memory,
			limits,
		};
		host.call_raw("start", &[])?;
		Ok(host)
	}
}

impl crate::Host for WasmiHost {
	fn host_state(&mut self) -> &mut HostState {
		&mut self.store.data_mut().host
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Result<Vec<u8>, CallError> {
		let data = self.memory.data(&self.store);
		let end = ptr
			.checked_add(len)
			.ok_or_else(|| CallError::Instance("memory read overflow".into()))?;
		data.get(ptr as usize..end as usize)
			.map(|s| s.to_vec())
			.ok_or_else(|| CallError::Instance(format!("memory read out of bounds at {ptr}")))
	}

	fn reset_fuel(&mut self) {
		self.store.set_fuel(self.limits.fuel).expect("fuel enabled");
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> Result<(), CallError> {
		let end = ptr
			.checked_add(bytes.len() as u32)
			.ok_or_else(|| CallError::Instance("memory write overflow".into()))?;
		let data = self.memory.data_mut(&mut self.store);
		let slot = data
			.get_mut(ptr as usize..end as usize)
			.ok_or_else(|| CallError::Instance(format!("memory write out of bounds at {ptr}")))?;
		slot.copy_from_slice(bytes);
		Ok(())
	}

	fn fuel_remaining(&mut self) -> u64 {
		self.store.get_fuel().unwrap_or(0)
	}

	fn fuel_budget(&self) -> u64 {
		self.limits.fuel
	}

	fn set_fuel_budget(&mut self, budget: u64) {
		self.limits.fuel = budget;
	}

	fn call_raw(&mut self, name: &str, args: &[i32]) -> Result<i32, CallError> {
		self.reset_fuel();
		let function = self
			.instance
			.get_func(&self.store, name)
			.ok_or_else(|| CallError::Instance(format!("missing export {name}")))?;
		let params: Vec<Val> = args.iter().map(|a| Val::I32(*a)).collect();
		let mut results = vec![Val::I32(0); function.ty(&self.store).results().len()];
		match function.call(&mut self.store, &params, &mut results) {
			Ok(()) => Ok(results.first().and_then(|v| v.i32()).unwrap_or(0)),
			Err(e) => {
				if e.as_trap_code() == Some(TrapCode::OutOfFuel) {
					Err(CallError::OutOfFuel)
				} else {
					Err(CallError::Trap(e.to_string()))
				}
			}
		}
	}
}
