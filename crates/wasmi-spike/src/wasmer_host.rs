//! wasmer 基线宿主：按生产 `SourceInstance::build` 的同款接线
//! （Cranelift + Metering middleware + 内存页数钳制），import 函数只做
//! `FunctionEnvMut` → `Ctx` 的薄适配，函数体与 wasmi 侧共用。

use crate::host::{Ctx, HostState};
use crate::{CallError, Host as _, Limits};
use std::ptr::NonNull;
use std::sync::Arc;
use wasmer::sys::vm::{MemoryError, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};
use wasmer::sys::{BaseTunables, CompilerConfig, Cranelift, EngineBuilder, Tunables};
use wasmer::{
	Function, FunctionEnv, FunctionEnvMut, Instance, MemoryStyle, MemoryType, Module, Pages, Store, TableStyle,
	TableType, Value,
};
use wasmer_middlewares::Metering;
use wasmer_middlewares::metering::{MeteringPoints, get_remaining_points, set_remaining_points};

type CostFn = fn(&wasmer::wasmparser::Operator) -> u64;

fn operator_cost(_operator: &wasmer::wasmparser::Operator) -> u64 {
	1
}

/// 生产 `LimitedTunables` 的同款钳制：模块声明的内存上限压到 limit_pages
#[derive(Clone)]
struct LimitedTunables {
	base: BaseTunables,
	limit: Pages,
}

impl LimitedTunables {
	fn new(memory_max_bytes: u64) -> Self {
		Self {
			base: BaseTunables::new(),
			limit: Pages(memory_max_bytes.div_ceil(64 * 1024) as u32),
		}
	}

	fn clamp(&self, ty: &MemoryType) -> MemoryType {
		let mut adjusted = *ty;
		adjusted.maximum = Some(adjusted.maximum.map_or(self.limit, |m| m.min(self.limit)));
		adjusted
	}
}

impl Tunables for LimitedTunables {
	fn memory_style(&self, memory: &MemoryType) -> MemoryStyle {
		self.base.memory_style(&self.clamp(memory))
	}

	fn table_style(&self, table: &TableType) -> TableStyle {
		self.base.table_style(table)
	}

	fn create_host_memory(&self, ty: &MemoryType, style: &MemoryStyle) -> Result<VMMemory, MemoryError> {
		self.base.create_host_memory(&self.clamp(ty), style)
	}

	unsafe fn create_vm_memory(
		&self,
		ty: &MemoryType,
		style: &MemoryStyle,
		vm_definition_location: NonNull<VMMemoryDefinition>,
	) -> Result<VMMemory, MemoryError> {
		unsafe { self.base.create_vm_memory(&self.clamp(ty), style, vm_definition_location) }
	}

	fn create_host_table(&self, ty: &TableType, style: &TableStyle) -> Result<VMTable, String> {
		self.base.create_host_table(ty, style)
	}

	unsafe fn create_vm_table(
		&self,
		ty: &TableType,
		style: &TableStyle,
		vm_definition_location: NonNull<VMTableDefinition>,
	) -> Result<VMTable, String> {
		unsafe { self.base.create_vm_table(ty, style, vm_definition_location) }
	}
}

struct WasmerData {
	host: HostState,
	memory: Option<wasmer::Memory>,
}

/// 引擎 shim：把 FunctionEnvMut 包成 Ctx。MemoryView 读写走共享内存的内部可变性，
/// 不可变借用就够，与 host() 的 &mut 不打架
struct WasmerCtx<'a, 'b> {
	env: &'a mut FunctionEnvMut<'b, WasmerData>,
}

impl Ctx for WasmerCtx<'_, '_> {
	fn host(&mut self) -> &mut HostState {
		&mut self.env.data_mut().host
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
		if len == 0 {
			return Some(Vec::new());
		}
		let memory = self.env.data().memory.as_ref()?;
		let view = memory.view(&*self.env);
		let mut data = vec![0u8; len as usize];
		view.read(ptr as u64, &mut data).ok()?;
		Some(data)
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
		let Some(memory) = self.env.data().memory.as_ref() else {
			return false;
		};
		memory.view(&*self.env).write(ptr as u64, bytes).is_ok()
	}
}

pub struct WasmerHost {
	store: Store,
	instance: Instance,
	env: FunctionEnv<WasmerData>,
	limits: Limits,
	_metering: Arc<Metering<CostFn>>,
}

impl WasmerHost {
	pub fn build(wasm_bytes: &[u8], limits: Limits) -> Result<Self, CallError> {
		let prepared = WasmerPrepared::new(limits);
		let module = prepared.compile(wasm_bytes)?;
		prepared.instantiate(&module)
	}
}

/// 引擎 + 编译产物与实例化分离：内存/冷启动测量要区分模块成本与每实例成本
pub struct WasmerPrepared {
	engine: wasmer_compiler::Engine,
	metering: Arc<Metering<CostFn>>,
	limits: Limits,
}

impl WasmerPrepared {
	pub fn new(limits: Limits) -> Self {
		let metering: Arc<Metering<CostFn>> = Arc::new(Metering::new(limits.fuel, operator_cost as CostFn));
		let mut compiler = Cranelift::default();
		compiler.push_middleware(metering.clone());
		let mut engine = EngineBuilder::new(compiler).engine();
		engine.set_tunables(LimitedTunables::new(limits.memory_max));
		Self {
			engine,
			metering,
			limits,
		}
	}

	pub fn compile(&self, wasm_bytes: &[u8]) -> Result<Module, CallError> {
		Module::new(&Store::new(self.engine.clone()), wasm_bytes)
			.map_err(|e| CallError::Instance(format!("compile module: {e}")))
	}

	pub fn instantiate(&self, module: &Module) -> Result<WasmerHost, CallError> {
		let limits = self.limits.clone();
		let mut store = Store::new(self.engine.clone());
		let env = FunctionEnv::new(
			&mut store,
			WasmerData {
				host: HostState::new(limits.clone()),
				memory: None,
			},
		);
		let imports = wasmer::imports! {
			"env" => {
				"abort" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>| {
					crate::host::env::abort(&mut WasmerCtx { env: &mut { env } })
				}),
				"print" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, ptr: u32, len: u32| {
					crate::host::env::print(&mut WasmerCtx { env: &mut { env } }, ptr, len)
				}),
				"sleep" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, seconds: i32| {
					crate::host::env::sleep(&mut WasmerCtx { env: &mut { env } }, seconds)
				}),
				"send_partial_result" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, value: i32| {
					crate::host::env::send_partial_result(&mut WasmerCtx { env: &mut { env } }, value)
				}),
			},
			"std" => {
				"destroy" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| {
					crate::host::std::destroy(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"buffer_len" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| -> i32 {
					crate::host::std::buffer_len(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"read_buffer" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, ptr: u32, size: u32| -> i32 {
					crate::host::std::read_buffer(&mut WasmerCtx { env: &mut { env } }, rid, ptr, size)
				}),
				"current_date" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>| -> f64 {
					crate::host::std::current_date(&mut WasmerCtx { env: &mut { env } })
				}),
				"utc_offset" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>| -> i64 {
					crate::host::std::utc_offset(&mut WasmerCtx { env: &mut { env } })
				}),
				"parse_date" => Function::new_typed_with_env(&mut store, &env,
					|env: FunctionEnvMut<WasmerData>, a: u32, b: u32, c: u32, d: u32, e: u32, f: u32, g: u32, h: u32| -> f64 {
						crate::host::std::parse_date(&mut WasmerCtx { env: &mut { env } }, a, b, c, d, e, f, g, h)
					}),
			},
			"net" => {
				"init" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, method: u8| -> i32 {
					crate::host::net::init(&mut WasmerCtx { env: &mut { env } }, i32::from(method))
				}),
				"send" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| -> i32 {
					crate::host::net::send(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"send_all" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, ptr: u32, len: u32| -> i32 {
					crate::host::net::send_all(&mut WasmerCtx { env: &mut { env } }, ptr, len)
				}),
				"set_url" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, ptr: u32, len: u32| -> i32 {
					crate::host::net::set_url(&mut WasmerCtx { env: &mut { env } }, rid, ptr, len)
				}),
				"set_header" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, kp: u32, kl: u32, vp: u32, vl: u32| -> i32 {
					crate::host::net::set_header(&mut WasmerCtx { env: &mut { env } }, rid, kp, kl, vp, vl)
				}),
				"set_body" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, ptr: u32, len: u32| -> i32 {
					crate::host::net::set_body(&mut WasmerCtx { env: &mut { env } }, rid, ptr, len)
				}),
				"set_timeout" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, value: f64| -> i32 {
					crate::host::net::set_timeout(&mut WasmerCtx { env: &mut { env } }, rid, value)
				}),
				"data_len" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| -> i32 {
					crate::host::net::data_len(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"read_data" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, buffer: u32, size: u32| -> i32 {
					crate::host::net::read_data(&mut WasmerCtx { env: &mut { env } }, rid, buffer, size)
				}),
				"get_status_code" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| -> i32 {
					crate::host::net::get_status_code(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"get_url" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32| -> i32 {
					crate::host::net::get_url(&mut WasmerCtx { env: &mut { env } }, rid)
				}),
				"get_header" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, rid: i32, kp: u32, kl: u32| -> i32 {
					crate::host::net::get_header(&mut WasmerCtx { env: &mut { env } }, rid, kp, kl)
				}),
				"set_rate_limit" => Function::new_typed_with_env(&mut store, &env, |env: FunctionEnvMut<WasmerData>, permits: i32, period: i32, unit: i32| {
					crate::host::net::set_rate_limit(&mut WasmerCtx { env: &mut { env } }, permits, period, unit)
				}),
			},
		};
		let instance = Instance::new(&mut store, module, &imports)
			.map_err(|e| CallError::Instance(format!("instantiate: {e}")))?;
		{
			let memory = instance
				.exports
				.get_memory("memory")
				.map_err(|e| CallError::Instance(format!("source exports no memory: {e}")))?;
			env.as_mut(&mut store).memory = Some(memory.clone());
		}
		let mut host = WasmerHost {
			store,
			instance,
			env,
			limits,
			_metering: self.metering.clone(),
		};
		host.call_raw("start", &[])?;
		Ok(host)
	}
}

impl crate::Host for WasmerHost {
	fn host_state(&mut self) -> &mut HostState {
		&mut self.env.as_mut(&mut self.store).host
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Result<Vec<u8>, CallError> {
		if len == 0 {
			return Ok(Vec::new());
		}
		let memory = self
			.instance
			.exports
			.get_memory("memory")
			.map_err(|e| CallError::Instance(format!("no memory: {e}")))?;
		let view = memory.view(&self.store);
		let mut data = vec![0u8; len as usize];
		view.read(ptr as u64, &mut data)
			.map_err(|e| CallError::Instance(e.to_string()))?;
		Ok(data)
	}

	fn reset_fuel(&mut self) {
		set_remaining_points(&mut self.store, &self.instance, self.limits.fuel);
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> Result<(), CallError> {
		let memory = self
			.instance
			.exports
			.get_memory("memory")
			.map_err(|e| CallError::Instance(format!("no memory: {e}")))?;
		memory
			.view(&self.store)
			.write(ptr as u64, bytes)
			.map_err(|e| CallError::Instance(e.to_string()))
	}

	fn fuel_remaining(&mut self) -> u64 {
		match get_remaining_points(&mut self.store, &self.instance) {
			MeteringPoints::Remaining(v) => v,
			MeteringPoints::Exhausted => 0,
		}
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
			.exports
			.get_function(name)
			.map_err(|_| CallError::Instance(format!("missing export {name}")))?
			.clone();
		let values: Vec<Value> = args.iter().map(|a| Value::I32(*a)).collect();
		match function.call(&mut self.store, &values) {
			Ok(values) => Ok(values.first().and_then(|v| v.i32()).unwrap_or(0)),
			Err(e) => {
				if let MeteringPoints::Exhausted = get_remaining_points(&mut self.store, &self.instance) {
					Err(CallError::OutOfFuel)
				} else {
					Err(CallError::Trap(e.to_string()))
				}
			}
		}
	}
}
