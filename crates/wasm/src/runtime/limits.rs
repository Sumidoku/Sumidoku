use std::ptr::NonNull;
use std::time::Duration;
use wasmer::sys::vm::{MemoryError, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};
use wasmer::sys::{BaseTunables, Tunables};
use wasmer::{MemoryStyle, MemoryType, Pages, TableStyle, TableType};

#[derive(Debug, Clone)]
pub struct Limits {
	/// 单次导出调用的指令数上限（wasmer metering fuel）
	pub fuel: u64,
	/// 调用级 wall-clock 超时，超时熔断重启实例
	pub call_timeout: Duration,
	/// env.sleep 单次上限
	pub sleep_max: Duration,
	pub net_timeout_default: Duration,
	pub net_timeout_max: Duration,
	/// 实例线性内存上限，memory.grow 超过即失败
	pub memory_max: u64,
	/// 单次 js eval 的执行上限（脚本入口 wall-clock，budget 让出点检查）
	pub js_timeout: Duration,
	/// js 单循环迭代上限：promise reaction job 内死循环的最后兜底（VM 回边检查）
	pub js_loop_max: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			fuel: 10_000_000_000,
			call_timeout: Duration::from_secs(60),
			sleep_max: Duration::from_secs(10),
			net_timeout_default: Duration::from_secs(30),
			net_timeout_max: Duration::from_secs(120),
			memory_max: 256 * 1024 * 1024,
			js_timeout: Duration::from_secs(10),
			js_loop_max: 1_000_000_000,
		}
	}
}

/// 按官方 tunables-limit-memory 示例的模式包装 BaseTunables，
/// 把模块声明的内存上限钳制到 limit_pages。
#[derive(Clone)]
pub struct LimitedTunables {
	base: BaseTunables,
	limit: Pages,
}

impl LimitedTunables {
	pub fn new(memory_max_bytes: u64) -> Self {
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
