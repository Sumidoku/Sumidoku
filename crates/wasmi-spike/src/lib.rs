//! wasmi 评估宿主（issue #169 的 time-boxed spike，不迁移生产代码）。
//! 三个 host import 族（env/std/net）的函数体与生产实现同构且与引擎无关，
//! 经 [`Ctx`] 访问宿主状态与 guest 内存；wasmer 基线与 wasmi 各自只做引擎接线，
//! 以此直接量出「改写成本只在 shim 层」。

pub mod host;
pub mod net;
pub mod store;
pub mod wasmer_host;
pub mod wasmi_host;

use serde::Serialize;
use serde::de::DeserializeOwned;
use store::Rid;

/// 与生产 `Limits` 对齐的子集：只保留 spike 接线用到的字段与默认值
#[derive(Debug, Clone)]
pub struct Limits {
	pub fuel: u64,
	pub sleep_max: std::time::Duration,
	pub net_timeout_default: std::time::Duration,
	pub net_timeout_max: std::time::Duration,
	pub net_body_max: u64,
	pub memory_max: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			fuel: 10_000_000_000,
			sleep_max: std::time::Duration::from_secs(10),
			net_timeout_default: std::time::Duration::from_secs(30),
			net_timeout_max: std::time::Duration::from_secs(120),
			net_body_max: 32 * 1024 * 1024,
			memory_max: 128 * 1024 * 1024,
		}
	}
}

/// 导出调用失败的对外分类：与生产 `Error::Trap("out of fuel")` 的区分语义一致
#[derive(Debug, thiserror::Error)]
pub enum CallError {
	#[error("out of fuel")]
	OutOfFuel,
	#[error("trap: {0}")]
	Trap(String),
	#[error("instance: {0}")]
	Instance(String),
}

/// 引擎宿主实例的统一操作面：内存读取、导出调用、fuel 读数（dyn-safe）。
/// 泛型流程方法在 [`HostExt`]，经 blanket impl 落到每个 Host 上
pub trait Host {
	fn host_state(&mut self) -> &mut host::HostState;
	/// 从 guest 线性内存读 len 字节；越界按 FFI 语义报错
	fn mem_read(&self, ptr: u32, len: u32) -> Result<Vec<u8>, CallError>;
	/// 测试侧向 guest 线性内存写参数（URL 等裸 UTF-8）
	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> Result<(), CallError>;
	/// 每次导出调用前把 fuel 预算重置为 per-call 上限（生产 call_raw 的同款动作）
	fn reset_fuel(&mut self);
	/// 当前剩余 fuel；Exhausted 归一成 0
	fn fuel_remaining(&mut self) -> u64;
	/// per-call 预算上限（每次 call_raw 重置到这个值）
	fn fuel_budget(&self) -> u64;
	fn set_fuel_budget(&mut self, budget: u64);
	/// 调用导出函数，返回 ABI 的 i32（负数为错误码，非负为结果指针/数值）
	fn call_raw(&mut self, name: &str, args: &[i32]) -> Result<i32, CallError>;
}

/// ABI 辅助与源流程方法：与生产 `SourceInstance` 同一条数据流，
/// 保证两引擎测的是同一件事
pub trait HostExt: Host {
	fn store_encoded<T: Serialize>(&mut self, value: &T, transient: &mut Vec<Rid>) -> Result<Rid, CallError> {
		let rid = self
			.host_state()
			.store
			.store_encoded(value)
			.map_err(|e| CallError::Instance(format!("encode argument: {e}")))?;
		transient.push(rid);
		Ok(rid)
	}

	fn store_string(&mut self, value: Option<&str>, transient: &mut Vec<Rid>) -> Rid {
		let rid = self
			.host_state()
			.store
			.store(store::StoreItem::String(value.unwrap_or_default().to_string()));
		transient.push(rid);
		rid
	}

	fn cleanup(&mut self, transient: Vec<Rid>) {
		for rid in transient {
			self.host_state().store.remove(rid);
		}
	}

	fn read_u32(&self, ptr: u32) -> Result<u32, CallError> {
		let bytes = self.mem_read(ptr, 4)?;
		Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
	}

	/// 与生产 `SourceInstance::read_result` 同一条解码路径：
	/// head == u32::MAX 是源侧错误消息，否则 head-8 字节 postcard 载荷
	fn read_result<T: DeserializeOwned>(&mut self, ret: i32) -> Result<T, CallError> {
		if ret < 0 {
			return Err(CallError::Instance(format!("ffi error code {ret}")));
		}
		let ptr = ret as u32;
		let head = self.read_u32(ptr)?;
		let result = if head == u32::MAX {
			let real_len = self.read_u32(ptr + 8)?;
			let msg = self.mem_read(ptr + 12, real_len.saturating_sub(12))?;
			Err(CallError::Instance(String::from_utf8_lossy(&msg).into_owned()))
		} else {
			if head < 8 {
				return Err(CallError::Instance("invalid result header".into()));
			}
			let payload = self.mem_read(ptr + 8, head - 8)?;
			postcard::from_bytes(&payload).map_err(|_| CallError::Instance("deserialize result".into()))
		};
		// free_result 失败不值得让整调用失败，与生产一致只记日志
		if let Err(e) = self.call_raw("free_result", &[ret]) {
			self.host_state().logs.push(format!("free_result failed: {e}"));
		}
		result
	}

	fn get_manga_list(&mut self, listing: &aidoku::Listing, page: i32) -> Result<aidoku::MangaPageResult, CallError> {
		let mut t = Vec::new();
		let listing_rid = self.store_encoded(listing, &mut t)?;
		let ret = self.call_raw("get_manga_list", &[listing_rid, page]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	fn get_search_manga_list(
		&mut self,
		query: Option<String>,
		page: i32,
		filters: Vec<aidoku::FilterValue>,
	) -> Result<aidoku::MangaPageResult, CallError> {
		let mut t = Vec::new();
		let query_rid = self.store_string(query.as_deref(), &mut t);
		let filters_rid = self.store_encoded(&filters, &mut t)?;
		let ret = self.call_raw("get_search_manga_list", &[query_rid, page, filters_rid]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	fn get_manga_update(
		&mut self,
		manga: &aidoku::Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<aidoku::Manga, CallError> {
		let mut t = Vec::new();
		let manga_rid = self.store_encoded(manga, &mut t)?;
		let ret = self.call_raw(
			"get_manga_update",
			&[manga_rid, needs_details as i32, needs_chapters as i32],
		);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	fn get_page_list(&mut self, manga: &aidoku::Manga, chapter: &aidoku::Chapter) -> Result<Vec<aidoku::Page>, CallError> {
		let mut t = Vec::new();
		let manga_rid = self.store_encoded(manga, &mut t)?;
		let chapter_rid = self.store_encoded(chapter, &mut t)?;
		let ret = self.call_raw("get_page_list", &[manga_rid, chapter_rid]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	fn get_listings(&mut self) -> Result<Vec<aidoku::Listing>, CallError> {
		let ret = self.call_raw("get_listings", &[]);
		ret.and_then(|r| self.read_result(r))
	}
}

impl<H: Host + ?Sized> HostExt for H {}

/// listing fixture 的路径（tests 与测量 bin 共用）
pub fn fixture_wasm() -> Vec<u8> {
	let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/listing/main.wasm");
	std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}
